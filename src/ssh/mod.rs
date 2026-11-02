pub mod auth;
mod channel;
pub mod credentials;
pub mod forward;
pub mod host_keys;
pub mod route;
use crate::workspace::AuthMethod;
use crate::{terminal::Terminal, workspace::ConnectionProfile};
use credentials::Secret;
use host_keys::{HostKeys, Verification};
use russh::{
    ChannelMsg, client,
    keys::{HashAlg, PrivateKeyWithHashAlg},
};
use std::time::Duration;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::{mpsc, oneshot, watch};
pub type SessionId = u64;
const MAX_INPUT_BYTES: usize = 64 * 1024;
const INPUT_QUEUE_CAPACITY: usize = 32;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Connecting,
    HostKey {
        fingerprint: String,
        previous: Option<String>,
    },
    Authenticating,
    Connected,
    Disconnected,
    Failed(String),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalStatus {
    NotRequested,
    Running,
    Finished,
}
pub struct SessionState {
    pub status: Status,
    pub terminal: Terminal,
    pub revision: u64,
    pub input_notice: Option<String>,
    pub exit_status: Option<u32>,
    pub remote_eof: bool,
    pub terminal_status: TerminalStatus,
    pub hop: Option<auth::HopIdentity>,
    pub authentication: Option<auth::AuthPrompt>,
}
pub struct Session {
    pub id: SessionId,
    pub profile: ConnectionProfile,
    pub state: Mutex<SessionState>,
    commands: mpsc::Sender<Command>,
    terminal_stop: watch::Sender<bool>,
    outbound: mpsc::Sender<Vec<u8>>,
    transport: Mutex<Option<Arc<client::Handle<Handler>>>>,
    socket: Mutex<Option<std::net::TcpStream>>,
    trust: Mutex<Option<oneshot::Sender<bool>>>,
    auth_reply: Mutex<Option<oneshot::Sender<Vec<Secret>>>>,
    capacity: Arc<channel::Capacity>,
    hops: Mutex<Vec<Arc<client::Handle<Handler>>>>,
    forwards: Mutex<HashMap<u64, Arc<forward::ForwardJob>>>,
    remote_forwards: forward::RemoteRegistry,
    owner: Option<Arc<Session>>,
    stop: Mutex<Option<oneshot::Sender<()>>>,
}
enum Command {
    Resize(u32, u32),
    Open(oneshot::Sender<Result<channel::ManagedChannel, String>>),
}
pub struct Service {
    runtime: tokio::runtime::Runtime,
    sessions: Mutex<HashMap<SessionId, Arc<Session>>>,
    next: AtomicU64,
    host_keys: PathBuf,
}
impl Service {
    pub fn new(host_keys: PathBuf) -> Self {
        Self {
            runtime: tokio::runtime::Runtime::new().expect("SSH runtime"),
            sessions: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            host_keys,
        }
    }
    pub fn global() -> &'static Self {
        static SERVICE: OnceLock<Service> = OnceLock::new();
        SERVICE.get_or_init(|| {
            Self::new(crate::settings::Settings::path().with_file_name("known-host-keys.json"))
        })
    }
    pub fn session(&self, id: SessionId) -> Option<Arc<Session>> {
        self.sessions.lock().unwrap().get(&id).cloned()
    }
    /// Session discovery supports explicit reattachment after a view is closed.
    pub fn resumable(&self, profile: &ConnectionProfile) -> Vec<Arc<Session>> {
        let mut sessions = self
            .sessions
            .lock()
            .unwrap()
            .values()
            .filter(|session| {
                session.profile == *profile
                    && session.state.lock().unwrap().status == Status::Connected
                    && session.stop.lock().unwrap().is_some()
            })
            .cloned()
            .collect::<Vec<_>>();
        sessions.sort_by_key(|session| session.id);
        sessions
    }
    pub fn connect_transport(
        &self,
        profile: ConnectionProfile,
        secret: Secret,
        remember: bool,
    ) -> Arc<Session> {
        self.start(profile, secret, remember, false)
    }
    pub fn connect(
        &self,
        profile: ConnectionProfile,
        secret: Secret,
        remember: bool,
    ) -> Arc<Session> {
        self.start(profile, secret, remember, true)
    }
    fn start(
        &self,
        profile: ConnectionProfile,
        secret: Secret,
        remember: bool,
        terminal: bool,
    ) -> Arc<Session> {
        let owner = if profile.transport_policy != crate::workspace::TransportPolicy::Independent {
            self.resumable(&profile).into_iter().find(|s| {
                s.transport
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|ssh| !ssh.is_closed())
            })
        } else {
            None
        };
        if !terminal && let Some(owner) = &owner {
            return owner.clone();
        }
        let (commands, receiver) = mpsc::channel(32);
        let (outbound, outgoing) = mpsc::channel(INPUT_QUEUE_CAPACITY);
        let (stop, stopped) = oneshot::channel();
        let session = Arc::new(Session {
            id: self.next.fetch_add(1, Ordering::Relaxed),
            profile,
            state: Mutex::new(SessionState {
                status: Status::Connecting,
                terminal: Terminal::new(80, 24),
                revision: 0,
                input_notice: None,
                exit_status: None,
                remote_eof: false,
                terminal_status: TerminalStatus::NotRequested,
                hop: None,
                authentication: None,
            }),
            commands,
            terminal_stop: watch::channel(false).0,
            outbound,
            transport: Mutex::new(None),
            socket: Mutex::new(None),
            trust: Mutex::new(None),
            auth_reply: Mutex::new(None),
            capacity: owner
                .as_ref()
                .map(|s| s.capacity.clone())
                .unwrap_or_default(),
            hops: Mutex::new(Vec::new()),
            forwards: Mutex::new(HashMap::new()),
            remote_forwards: owner
                .as_ref()
                .map(|s| s.remote_forwards.clone())
                .unwrap_or_default(),
            owner,
            stop: Mutex::new(Some(stop)),
        });
        self.sessions
            .lock()
            .unwrap()
            .insert(session.id, session.clone());
        let running = session.clone();
        let path = self.host_keys.clone();
        self.runtime.spawn(async move {
            tokio::select! {
                biased;
                _ = stopped => running.status(Status::Disconnected),
                result = run(running.clone(), secret, remember, path, receiver, outgoing, terminal) => {
                    if let Err(error) = result
                        && !matches!(running.state.lock().unwrap().status,Status::Failed(_)) {running.status(Status::Failed(error));}
                }
            }
            for job in running.forwards() {job.stop();}
            // Disconnect is a transport-level action even when independently
            // opened SFTP/exec channels still hold network sender references.
            let transport=running.transport.lock().unwrap().take();
            if running.owner.is_none() && let Some(ssh)=transport {let _=tokio::time::timeout(Duration::from_millis(500),ssh.disconnect(russh::Disconnect::ByApplication,"Session ended","en")).await;}
            let hops = std::mem::take(&mut *running.hops.lock().unwrap());
            for hop in hops.into_iter().rev() { let _ = tokio::time::timeout(Duration::from_millis(500), hop.disconnect(russh::Disconnect::ByApplication, "Route ended", "en")).await; }
            running.auth_reply.lock().unwrap().take();
            running.state.lock().unwrap().authentication = None;
            // Hard-close the shared TCP stream even if the transport control
            // queue is itself blocked or another channel retains a sender.
            if let Some(socket)=running.socket.lock().unwrap().take() {let _=socket.shutdown(std::net::Shutdown::Both);}
        });
        session
    }
}
impl Session {
    fn status(&self, status: Status) {
        let mut state = self.state.lock().unwrap();
        state.status = status;
        state.revision += 1;
    }
    pub fn trust(&self, accept: bool) {
        if let Some(reply) = self.trust.lock().unwrap().take() {
            let _ = reply.send(accept);
        }
    }
    pub fn terminal_running(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.status == Status::Connected && state.terminal_status == TerminalStatus::Running
    }
    fn terminal_finished(&self, notice: Option<String>) {
        let mut state = self.state.lock().unwrap();
        state.terminal_status = TerminalStatus::Finished;
        if let Some(notice) = notice {
            state.input_notice = Some(notice);
        }
        state.revision += 1;
    }
    /// Reject an entire oversized/full-queue input without truncating commands.
    pub fn input(&self, bytes: Vec<u8>) {
        {
            let mut state = self.state.lock().unwrap();
            if state.status != Status::Connected {
                return;
            }
            if state.terminal_status != TerminalStatus::Running {
                state.input_notice =
                    Some("Terminal input not sent: shell is finished or unavailable".into());
                state.revision += 1;
                return;
            }
        }
        let error = if bytes.len() > MAX_INPUT_BYTES {
            Some("Terminal input not sent: paste exceeds 64 KiB")
        } else if self.outbound.try_send(bytes).is_err() {
            Some("Terminal input not sent: outbound queue is full or closed")
        } else {
            None
        };
        if let Some(error) = error {
            let mut state = self.state.lock().unwrap();
            state.input_notice = Some(error.into());
            state.revision += 1;
        }
    }
    pub fn resize(&self, cols: u32, rows: u32) {
        if self.terminal_running() {
            let _ = self
                .commands
                .try_send(Command::Resize(cols.max(2), rows.max(1)));
        }
    }
    /// Close only this presentation's PTY; shared SFTP/exec/forward channels stay alive.
    pub fn stop_terminal(&self) {
        // Retained independently of ordinary resize/channel-open queue capacity.
        // send_replace also retains a request made before connected() subscribes.
        self.terminal_stop.send_replace(true);
    }
    pub fn disconnect(&self) {
        // Disconnect is explicitly transport-level, including reused PTYs.
        if let Some(owner) = &self.owner {
            owner.disconnect();
        }
        if let Some(stop) = self.stop.lock().unwrap().take() {
            let _ = stop.send(());
        }
        for job in self.forwards() {
            job.stop();
        }
        self.trust(false);
        self.auth_reply.lock().unwrap().take();
    }
    /// Opens an independent channel on this immutable session, for SFTP/exec.
    pub async fn open_channel(&self) -> Result<channel::ManagedChannel, String> {
        if self.state.lock().unwrap().status != Status::Connected {
            return Err("Connection not ready".into());
        }
        let (tx, rx) = oneshot::channel();
        self.commands
            .send(Command::Open(tx))
            .await
            .map_err(|_| "Session disconnected".to_string())?;
        rx.await.map_err(|_| "Session disconnected".to_string())?
    }
}

struct Handler {
    session: Arc<Session>,
    keys: PathBuf,
    profile: ConnectionProfile,
    destination: bool,
}
impl client::Handler for Handler {
    type Error = russh::Error;
    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: russh::Channel<client::Msg>,
        address: &str,
        port: u32,
        _: &str,
        _: u32,
        reply: client::ChannelOpenHandle,
        _: &mut client::Session,
    ) -> Result<(), Self::Error> {
        if !self.destination {
            return Ok(());
        }
        let registration = self
            .session
            .remote_forwards
            .lock()
            .unwrap()
            .get(&(address.to_string(), port))
            .cloned();
        if let Some(registration) = registration
            && let Ok(queue_slot) = registration.sender.try_reserve()
            && let Ok(permit) = registration.capacity.acquire(registration.policy)
        {
            // Admission precedes SSH confirmation. The permit and close owner
            // accompany the channel even while it waits in the forward inbox.
            let channel = channel::ManagedChannel::new(channel, permit);
            reply.accept().await;
            if !registration.sender.is_closed() {
                queue_slot.send(channel);
            }
        }
        Ok(())
    }

    async fn check_server_key(
        &mut self,
        key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // Certificates are pinned by their underlying public-key fingerprint too;
        // this is explicit TOFU, not CA/certificate-principal validation.
        let fingerprint = key.public_key().fingerprint(HashAlg::Sha256).to_string();
        let keys = match HostKeys::load(&self.keys) {
            Ok(k) => k,
            Err(e) => {
                self.session.status(Status::Failed(e));
                return Ok(false);
            }
        };
        let previous = match keys.verify(&self.profile.host, self.profile.port, &fingerprint) {
            Verification::Trusted => return Ok(true),
            Verification::Unknown => None,
            Verification::Changed(old) => Some(old),
        };
        let (tx, rx) = oneshot::channel();
        *self.session.trust.lock().unwrap() = Some(tx);
        self.session.status(Status::HostKey {
            fingerprint: fingerprint.clone(),
            previous: previous.clone(),
        });
        let accepted = tokio::time::timeout(Duration::from_secs(120), rx)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or(false);
        if !accepted {
            return Ok(false);
        }
        // Serialize the read/modify/replace transaction across simultaneous prompts.
        static STORE_WRITE: Mutex<()> = Mutex::new(());
        let _guard = STORE_WRITE.lock().unwrap();
        // Reload after consent so another connection's accepted key is preserved.
        let mut keys = match HostKeys::load(&self.keys) {
            Ok(k) => k,
            Err(e) => {
                self.session.status(Status::Failed(e));
                return Ok(false);
            }
        };
        match (
            &previous,
            keys.verify(&self.profile.host, self.profile.port, &fingerprint),
        ) {
            (_, Verification::Trusted) => return Ok(true),
            (None, Verification::Unknown) => {}
            (Some(expected), Verification::Changed(actual)) if *expected == actual => {}
            _ => {
                self.session.status(Status::Failed(
                    "Host identity changed while approval was pending".into(),
                ));
                return Ok(false);
            }
        }
        keys.trust(&self.profile.host, self.profile.port, fingerprint);
        if let Err(e) = keys.save(&self.keys) {
            self.session.status(Status::Failed(e));
            return Ok(false);
        }
        Ok(true)
    }
}
async fn run(
    session: Arc<Session>,
    mut secret: Secret,
    remember: bool,
    keys: PathBuf,
    commands: mpsc::Receiver<Command>,
    outgoing: mpsc::Receiver<Vec<u8>>,
    terminal: bool,
) -> Result<(), String> {
    if let Some(owner) = &session.owner {
        let ssh = owner
            .transport
            .lock()
            .unwrap()
            .clone()
            .ok_or("Shared transport disconnected")?;
        return connected(session, ssh, commands, outgoing, terminal).await;
    }
    let profiles = session.profile.route_profiles()?;
    let total = profiles.len();
    let mut previous: Option<Arc<client::Handle<Handler>>> = None;
    for (index, profile) in profiles.into_iter().enumerate() {
        let hop = auth::HopIdentity {
            host: profile.host.clone(),
            port: profile.port,
            username: profile.username.clone(),
            index,
            total,
        };
        {
            let mut state = session.state.lock().unwrap();
            state.hop = Some(hop.clone());
            state.revision += 1;
        }
        let handler = Handler {
            session: session.clone(),
            keys: keys.clone(),
            profile: profile.clone(),
            destination: index + 1 == total,
        };
        let config = Arc::new(client::Config {
            keepalive_interval: Some(Duration::from_secs(30)),
            keepalive_max: 3,
            ..Default::default()
        });
        let connecting = async {
            if let Some(parent) = &previous {
                let channel = parent
                    .channel_open_direct_tcpip(&profile.host, profile.port.into(), "127.0.0.1", 0)
                    .await?;
                client::connect_stream(config, channel.into_stream(), handler).await
            } else {
                let socket =
                    tokio::net::TcpStream::connect((profile.host.as_str(), profile.port)).await?;
                if config.nodelay {
                    let _ = socket.set_nodelay(true);
                }
                let socket = socket.into_std()?;
                *session.socket.lock().unwrap() = Some(socket.try_clone()?);
                client::connect_stream(config, tokio::net::TcpStream::from_std(socket)?, handler)
                    .await
            }
        };
        let mut ssh = tokio::time::timeout(Duration::from_secs(150), connecting)
            .await
            .map_err(|_| "Connection timed out")?
            .map_err(|_| "SSH connection or host-key verification failed")?;
        let final_hop = index + 1 == total;
        let hop_secret = if final_hop {
            std::mem::replace(&mut secret, Secret::new(String::new()))
        } else {
            Secret::new(String::new())
        };
        tokio::time::timeout(
            Duration::from_secs(180),
            auth::authenticate(
                &session,
                &mut ssh,
                &profile,
                hop,
                hop_secret,
                remember && final_hop,
                !final_hop,
            ),
        )
        .await
        .map_err(|_| "Authentication timed out")??;
        if final_hop {
            return connected(session, Arc::new(ssh), commands, outgoing, terminal).await;
        }
        let ssh = Arc::new(ssh);
        session.hops.lock().unwrap().push(ssh.clone());
        previous = Some(ssh);
    }
    Err("Empty jump route".into())
}
async fn connected(
    session: Arc<Session>,
    ssh: Arc<client::Handle<Handler>>,
    mut commands: mpsc::Receiver<Command>,
    mut outgoing: mpsc::Receiver<Vec<u8>>,
    terminal: bool,
) -> Result<(), String> {
    *session.transport.lock().unwrap() = Some(ssh.clone());
    let mut terminal_stop = session.terminal_stop.subscribe();
    let mut terminal_cancelled = *terminal_stop.borrow();
    if terminal && terminal_cancelled {
        session.terminal_finished(None);
    }
    let terminal = terminal && !terminal_cancelled;
    let mut terminal_permit = if terminal {
        Some(session.capacity.acquire(session.profile.transport_policy)?)
    } else {
        None
    };
    let (mut reader, mut writer) = if terminal {
        let channel = ssh
            .channel_open_session()
            .await
            .map_err(|_| "Server refused a session channel".to_string())?;
        channel
            .request_pty(true, "xterm-256color", 80, 24, 0, 0, &[])
            .await
            .map_err(|_| "PTY request failed".to_string())?;
        channel
            .request_shell(true)
            .await
            .map_err(|_| "Shell request failed (this may be an SFTP-only host)".to_string())?;
        let (reader, writer) = channel.split();
        (Some(reader), Some(Arc::new(writer)))
    } else {
        (None, None)
    };
    let mut terminal_lease = writer.as_ref().map(|writer| channel::TerminalLease {
        writer: writer.clone(),
        permit: terminal_permit.take(),
    });
    // The writer is owned by the terminal channel, independently of the
    // transport owner/control loop. Dropping it cancels a blocked channel write.
    let mut writing = writer.as_ref().map(|writer| {
        let writer = writer.clone();
        Box::pin(async move {
            while let Some(bytes) = outgoing.recv().await {
                writer
                    .data(bytes.as_slice())
                    .await
                    .map_err(|_| "Terminal input failed".to_string())?;
            }
            Ok::<(), String>(())
        })
    });
    if terminal {
        session.state.lock().unwrap().terminal_status = TerminalStatus::Running;
    }
    let mut openings = tokio::task::JoinSet::new();
    session.status(Status::Connected);
    loop {
        if ssh.is_closed() && reader.is_none() {
            break;
        }
        tokio::select! {
            biased;
            _ = async { let _ = terminal_stop.wait_for(|stopped| *stopped).await; }, if !terminal_cancelled => {
                terminal_cancelled = true;
                // Release only this PTY; TerminalLease closes it independently.
                // Shared transports, channel-open work and forwards remain alive.
                reader=None;writer=None;writing=None;terminal_lease.take();session.terminal_finished(None);
            },
            result=async {writing.as_mut().expect("writer guarded by select").await},if writing.is_some()=>{
                writing=None;writer=None;session.terminal_finished(result.err());
            },
            _=openings.join_next(),if !openings.is_empty()=>{},
            _ = tokio::time::sleep(Duration::from_secs(1)), if reader.is_none() => {if ssh.is_closed() {break;}}
            command = commands.recv() => match command {
                Some(Command::Resize(cols,rows)) => {
                    session.state.lock().unwrap().terminal.resize(cols as usize,rows as usize);
                    if let Some(writer)=&writer {
                        tokio::select! {
                            biased;
                            _ = terminal_stop.wait_for(|stopped| *stopped) => {},
                            result = writer.window_change(cols,rows,0,0) => {
                                if result.is_err() {session.terminal_finished(Some("Terminal resize failed".into()));}
                            }
                        }
                    }
                }
                Some(Command::Open(reply)) => {
                    let permit = match session.capacity.acquire(session.profile.transport_policy) {Ok(permit)=>permit,Err(error)=>{let _=reply.send(Err(error));continue;}};
                    let ssh=ssh.clone();
                    openings.spawn(async move {
                        let result=tokio::time::timeout(Duration::from_secs(30),ssh.channel_open_session()).await.map_err(|_|"Channel open timed out".to_string()).and_then(|result|result.map_err(|_|"Server refused channel".to_string()));
                        let result = result.map(|channel| channel::ManagedChannel::new(channel,permit));
                        if let Err(Ok(channel))=reply.send(result) {let _=channel.close().await;}
                    });
                }
                None => break,
            },
            event = async { match &mut reader {Some(reader)=>reader.wait().await,None=>std::future::pending().await} } => match event {
                Some(ChannelMsg::Data {data}) | Some(ChannelMsg::ExtendedData {data,..}) => {
                    let replies = {let mut state=session.state.lock().unwrap();state.terminal.feed(&data);state.revision+=1;state.terminal.replies()};
                    for reply in replies {session.input(reply.into_bytes());}
                }
                Some(ChannelMsg::ExitStatus {exit_status})=>{let mut state=session.state.lock().unwrap();state.exit_status=Some(exit_status);state.revision+=1;},
                Some(ChannelMsg::Eof)=>{let mut state=session.state.lock().unwrap();state.remote_eof=true;state.revision+=1;},
                Some(ChannelMsg::Failure) => {
                    reader=None;writer=None;writing=None;terminal_lease.take();
                    session.terminal_finished(Some("Server refused PTY or shell (SFTP-only hosts need the SFTP view)".into()));
                },
                Some(ChannelMsg::Close) | None => {
                    reader=None;writer=None;writing=None;terminal_lease.take();session.terminal_finished(None);
                },
                _ => {}
            }
        }
    }
    session.status(Status::Disconnected);
    Ok(())
}
/// Command execution captures an immutable session/channel and supports cancellation.
pub struct ExecTask {
    pub result: oneshot::Receiver<Result<ExecOutput, String>>,
    cancel: Option<oneshot::Sender<()>>,
}
impl ExecTask {
    pub fn cancel(&mut self) {
        if let Some(tx) = self.cancel.take() {
            let _ = tx.send(());
        }
    }
}
pub struct ExecOutput {
    pub bytes: Vec<u8>,
    pub exit_status: Option<u32>,
}
impl Service {
    pub fn exec(&self, session: Arc<Session>, command: Vec<u8>) -> ExecTask {
        let (tx, result) = oneshot::channel();
        let (cancel, mut cancelled) = oneshot::channel();
        self.runtime.spawn(async move {
            let work=async {
                let mut channel=tokio::select! {
                    Ok(())=&mut cancelled=>return Err("Cancelled".into()),
                    channel=session.open_channel()=>channel?,
                };
                channel.exec(true,command).await.map_err(|_| "Exec request failed".to_string())?;
                channel.eof().await.map_err(|_|"Exec stdin EOF failed".to_string())?;
                let mut output=ExecOutput {bytes:Vec::new(),exit_status:None};
                loop {tokio::select! {
                    Ok(())=&mut cancelled=> {let _=channel.close().await;return Err("Cancelled".into());}
                    message=channel.wait()=>match message {
                        Some(ChannelMsg::Data {data}) | Some(ChannelMsg::ExtendedData {data,..}) => {if output.bytes.len()+data.len()>16*1024*1024 {let _=channel.close().await;return Err("Exec output exceeded 16 MiB".into());}output.bytes.extend_from_slice(&data);}
                        Some(ChannelMsg::ExitStatus {exit_status})=>output.exit_status=Some(exit_status),
                        Some(ChannelMsg::Close) | None=>break,
                        Some(ChannelMsg::Failure)=>return Err("Exec request rejected".into()),
                        _=>{}
                    }
                }}
                Ok(output)
            };
            let _=tx.send(work.await);
        });
        ExecTask {
            result,
            cancel: Some(cancel),
        }
    }
}
#[cfg(test)]
mod tests;

#[cfg(test)]
mod adverse_tests;

#[cfg(test)]
mod advanced_tests;
