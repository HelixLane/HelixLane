//! Forward jobs own listeners and all active streams independently of views.
use super::*;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::watch,
};
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Destination {
    pub host: String,
    pub port: u16,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ForwardSpec {
    Local {
        bind: SocketAddr,
        target: Destination,
    },
    Remote {
        bind: SocketAddr,
        target: Destination,
    },
    Socks {
        bind: SocketAddr,
    },
}
impl ForwardSpec {
    pub fn loopback(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
    }
    fn validate(&self) -> Result<(), String> {
        let target = match self {
            Self::Local { target, .. } | Self::Remote { target, .. } => Some(target),
            Self::Socks { .. } => None,
        };
        if target.is_some_and(|t| {
            t.host.trim().is_empty()
                || t.host.len() > 253
                || t.host.chars().any(char::is_control)
                || t.port == 0
        }) {
            return Err("Invalid forwarding destination".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForwardStatus {
    Starting,
    Listening(SocketAddr),
    Stopped,
    Failed(String),
}
pub struct ForwardJob {
    pub id: u64,
    pub profile: ConnectionProfile,
    pub spec: ForwardSpec,
    pub status: Mutex<ForwardStatus>,
    stop: watch::Sender<bool>,
}
impl ForwardJob {
    pub fn stop(&self) {
        let _ = self.stop.send(true);
    }
}
#[derive(Clone)]
pub(super) struct RemoteRegistration {
    pub sender: mpsc::Sender<channel::ManagedChannel>,
    pub capacity: Arc<channel::Capacity>,
    pub policy: crate::workspace::TransportPolicy,
}
pub(super) type RemoteRegistry = Arc<Mutex<HashMap<(String, u32), RemoteRegistration>>>;
impl Session {
    pub fn forwards(&self) -> Vec<Arc<ForwardJob>> {
        let mut jobs = self
            .forwards
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        jobs.sort_by_key(|j| j.id);
        jobs
    }
}
impl Service {
    pub fn forward(&self, session: Arc<Session>, spec: ForwardSpec) -> Arc<ForwardJob> {
        let (stop, stopped) = watch::channel(false);
        let job = Arc::new(ForwardJob {
            id: self.next.fetch_add(1, Ordering::Relaxed),
            profile: session.profile.clone(),
            spec,
            status: Mutex::new(ForwardStatus::Starting),
            stop,
        });
        session.forwards.lock().unwrap().insert(job.id, job.clone());
        let running = job.clone();
        self.runtime.spawn(async move {
            let result = run_forward(&session, &running, stopped).await;
            *running.status.lock().unwrap() = match result {
                Ok(()) => ForwardStatus::Stopped,
                Err(error) => ForwardStatus::Failed(error),
            };
            session.state.lock().unwrap().revision += 1;
        });
        job
    }
}
async fn direct(
    session: &Session,
    target: &Destination,
    origin: SocketAddr,
) -> Result<channel::ManagedStream, String> {
    let permit = session.capacity.acquire(session.profile.transport_policy)?;
    let ssh = session
        .transport
        .lock()
        .unwrap()
        .clone()
        .ok_or("Connection not ready")?;
    let channel = tokio::time::timeout(
        Duration::from_secs(15),
        ssh.channel_open_direct_tcpip(
            &target.host,
            target.port.into(),
            origin.ip().to_string(),
            origin.port().into(),
        ),
    )
    .await
    .map_err(|_| "Forward channel timed out")?
    .map_err(|_| "Forward destination refused")?;
    Ok(channel::ManagedChannel::new(channel, permit).into_stream())
}
async fn run_forward(
    session: &Arc<Session>,
    job: &ForwardJob,
    mut stopped: watch::Receiver<bool>,
) -> Result<(), String> {
    job.spec.validate()?;
    if session.state.lock().unwrap().status != Status::Connected {
        return Err("Connection not ready".into());
    }
    let ssh = session
        .transport
        .lock()
        .unwrap()
        .clone()
        .ok_or("Connection not ready")?;
    let mut streams = tokio::task::JoinSet::new();
    match &job.spec {
        ForwardSpec::Local { bind, .. } | ForwardSpec::Socks { bind } => {
            let listener = TcpListener::bind(bind)
                .await
                .map_err(|_| "Cannot bind forwarding listener")?;
            *job.status.lock().unwrap() = ForwardStatus::Listening(
                listener
                    .local_addr()
                    .map_err(|_| "Cannot read listener address")?,
            );
            session.state.lock().unwrap().revision += 1;
            loop {
                if *stopped.borrow() {
                    break;
                }
                tokio::select! {
                    _=stopped.changed()=>break,
                    _=tokio::time::sleep(Duration::from_millis(100)),if ssh.is_closed()=>break,
                    _=streams.join_next(),if !streams.is_empty()=>{},
                    result=listener.accept()=>{
                        if streams.len() >= 128 { let _ = result; continue; }
                        let (mut tcp,origin)=result.map_err(|_|"Forward listener failed")?;let session=session.clone();let spec=job.spec.clone();
                        streams.spawn(async move {
                            let socks=matches!(spec,ForwardSpec::Socks {..});
                            let target=match spec {ForwardSpec::Local {target,..}=>target,ForwardSpec::Socks {..}=>match tokio::time::timeout(Duration::from_secs(15),socks_request(&mut tcp)).await {Ok(Ok(target))=>target,_=>return},_=>unreachable!()};
                            let mut stream=match direct(&session,&target,origin).await {Ok(stream)=>stream,Err(error)=>{{let mut state=session.state.lock().unwrap();state.input_notice=Some(error);state.revision+=1;}if socks {let _=socks_reply(&mut tcp,5).await;}return}};
                            if socks && socks_reply(&mut tcp,0).await.is_err() {return;}
                            let _=tokio::io::copy_bidirectional(&mut tcp,&mut stream).await;
                        });
                    }
                }
            }
        }
        ForwardSpec::Remote { bind, target } => {
            let address = bind.ip().to_string();
            let requested = tokio::time::timeout(
                Duration::from_secs(15),
                ssh.tcpip_forward(address.clone(), bind.port().into()),
            );
            // A global request cannot be recalled before its reply: learn the
            // allocated port, then cancel it, even if Stop arrived meanwhile.
            let assigned = match requested.await {
                Ok(result) => result.map_err(|_| "Remote forwarding refused")?,
                Err(_) => {
                    session.disconnect();
                    return Err("Remote forward request timed out; connection closed to remove unknown listener".into());
                }
            };
            let port = if bind.port() == 0 {
                u16::try_from(assigned)
                    .ok()
                    .filter(|p| *p > 0)
                    .ok_or("Invalid remote forwarding port")?
            } else {
                bind.port()
            };
            let key = (address.clone(), u32::from(port));
            let (tx, mut incoming) = mpsc::channel(32);
            {
                let mut registry = session.remote_forwards.lock().unwrap();
                if registry.contains_key(&key) {
                    return Err("Remote listener is already registered".into());
                }
                registry.insert(
                    key.clone(),
                    RemoteRegistration {
                        sender: tx,
                        capacity: session.capacity.clone(),
                        policy: session.profile.transport_policy,
                    },
                );
            }
            *job.status.lock().unwrap() =
                ForwardStatus::Listening(SocketAddr::new(bind.ip(), port));
            session.state.lock().unwrap().revision += 1;
            loop {
                if *stopped.borrow() {
                    break;
                }
                tokio::select! {
                    _=stopped.changed()=>break,
                    _=tokio::time::sleep(Duration::from_millis(100)),if ssh.is_closed()=>break,
                    _=streams.join_next(),if !streams.is_empty()=>{},
                    channel=incoming.recv()=>{let Some(channel)=channel else {break};if streams.len()>=128 {drop(channel);continue;}let target=target.clone();
                        streams.spawn(async move {let mut stream=channel.into_stream();if let Ok(Ok(mut tcp))=tokio::time::timeout(Duration::from_secs(15),TcpStream::connect((target.host.as_str(),target.port))).await {let _=tokio::io::copy_bidirectional(&mut tcp,&mut stream).await;}});
                    }
                }
            }
            // Stop admission before draining; any already reserved sender carries
            // a close-on-drop channel and cannot strand a raw accepted channel.
            incoming.close();
            session.remote_forwards.lock().unwrap().remove(&key);
            while let Ok(channel) = incoming.try_recv() {
                drop(channel);
            }
            drop(incoming);
            streams.abort_all();
            while streams.join_next().await.is_some() {}
            let cancellation = tokio::time::timeout(
                Duration::from_secs(5),
                ssh.cancel_tcpip_forward(address, port.into()),
            )
            .await;
            let failure = match cancellation {
                Ok(Ok(())) => None,
                Ok(Err(_)) => Some(
                    "Remote forward cancellation failed; shared connection closed to remove listener",
                ),
                Err(_) => Some(
                    "Remote forward cancellation timed out; shared connection closed to remove uncertain listener",
                ),
            };
            if let Some(failure) = failure {
                // A failed Stop must not relinquish a possibly live listener.
                // Disconnect the owner, including sibling shared work, while
                // retaining this specific failure on the job for the user.
                session.disconnect();
                return Err(failure.into());
            }
        }
    }
    streams.abort_all();
    while streams.join_next().await.is_some() {}
    Ok(())
}
async fn socks_reply(stream: &mut TcpStream, code: u8) -> std::io::Result<()> {
    stream.write_all(&[5, code, 0, 1, 0, 0, 0, 0, 0, 0]).await
}
async fn socks_request(stream: &mut TcpStream) -> Result<Destination, String> {
    let mut greeting = [0; 2];
    stream
        .read_exact(&mut greeting)
        .await
        .map_err(|_| "Incomplete SOCKS greeting")?;
    if greeting[0] != 5 || greeting[1] == 0 {
        return Err("SOCKS5 required".into());
    }
    let mut methods = vec![0; greeting[1] as usize];
    stream
        .read_exact(&mut methods)
        .await
        .map_err(|_| "Incomplete SOCKS methods")?;
    if !methods.contains(&0) {
        let _ = stream.write_all(&[5, 255]).await;
        return Err("SOCKS authentication method unsupported".into());
    }
    stream
        .write_all(&[5, 0])
        .await
        .map_err(|_| "SOCKS greeting write failed")?;
    let mut request = [0; 4];
    stream
        .read_exact(&mut request)
        .await
        .map_err(|_| "Incomplete SOCKS request")?;
    if request[0] != 5 || request[2] != 0 {
        let _ = socks_reply(stream, 1).await;
        return Err("Invalid SOCKS request".into());
    }
    if request[1] != 1 {
        let _ = socks_reply(stream, 7).await;
        return Err("Only SOCKS CONNECT is supported".into());
    }
    let host = match request[3] {
        1 => {
            let mut bytes = [0; 4];
            stream
                .read_exact(&mut bytes)
                .await
                .map_err(|_| "Incomplete SOCKS address")?;
            std::net::Ipv4Addr::from(bytes).to_string()
        }
        4 => {
            let mut bytes = [0; 16];
            stream
                .read_exact(&mut bytes)
                .await
                .map_err(|_| "Incomplete SOCKS address")?;
            std::net::Ipv6Addr::from(bytes).to_string()
        }
        3 => {
            let size = stream
                .read_u8()
                .await
                .map_err(|_| "Incomplete SOCKS address")?;
            let mut bytes = vec![0; size as usize];
            stream
                .read_exact(&mut bytes)
                .await
                .map_err(|_| "Incomplete SOCKS address")?;
            String::from_utf8(bytes).map_err(|_| "Invalid SOCKS hostname")?
        }
        _ => {
            let _ = socks_reply(stream, 8).await;
            return Err("SOCKS address type unsupported".into());
        }
    };
    let port = stream
        .read_u16()
        .await
        .map_err(|_| "Incomplete SOCKS port")?;
    if host.is_empty() || host.chars().any(char::is_control) || port == 0 {
        let _ = socks_reply(stream, 1).await;
        return Err("Invalid SOCKS destination".into());
    }
    Ok(Destination { host, port })
}
