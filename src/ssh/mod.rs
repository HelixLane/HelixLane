pub mod credentials;
pub mod host_keys;
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
use tokio::sync::{mpsc, oneshot};
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
}
pub struct Session {
    pub id: SessionId,
    pub profile: ConnectionProfile,
    pub state: Mutex<SessionState>,
    commands: mpsc::Sender<Command>,
    outbound: mpsc::Sender<Vec<u8>>,
    transport: Mutex<Option<Arc<client::Handle<Handler>>>>,
    socket: Mutex<Option<std::net::TcpStream>>,
    trust: Mutex<Option<oneshot::Sender<bool>>>,
    stop: Mutex<Option<oneshot::Sender<()>>>,
}
enum Command {
    Resize(u32, u32),
    Open(oneshot::Sender<Result<russh::Channel<russh::client::Msg>, String>>),
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
            }),
            commands,
            outbound,
            transport: Mutex::new(None),
            socket: Mutex::new(None),
            trust: Mutex::new(None),
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
            // Disconnect is a transport-level action even when independently
            // opened SFTP/exec channels still hold network sender references.
            let transport=running.transport.lock().unwrap().take();
            if let Some(ssh)=transport {let _=tokio::time::timeout(Duration::from_millis(500),ssh.disconnect(russh::Disconnect::ByApplication,"Session ended","en")).await;}
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
    pub fn disconnect(&self) {
        if let Some(stop) = self.stop.lock().unwrap().take() {
            let _ = stop.send(());
        }
        self.trust(false);
    }
    /// Opens an independent channel on this immutable session, for SFTP/exec.
    pub async fn open_channel(&self) -> Result<russh::Channel<russh::client::Msg>, String> {
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
}
impl client::Handler for Handler {
    type Error = russh::Error;
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
        let previous = match keys.verify(
            &self.session.profile.host,
            self.session.profile.port,
            &fingerprint,
        ) {
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
            keys.verify(
                &self.session.profile.host,
                self.session.profile.port,
                &fingerprint,
            ),
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
        keys.trust(
            &self.session.profile.host,
            self.session.profile.port,
            fingerprint,
        );
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
    if session.profile.jump_resource.is_some() {
        return Err("Jump hosts are not configured for this connection".into());
    }
    let handler = Handler {
        session: session.clone(),
        keys,
    };
    let config = Arc::new(client::Config {
        keepalive_interval: Some(Duration::from_secs(30)),
        keepalive_max: 3,
        ..Default::default()
    });
    let connecting = async {
        let socket =
            tokio::net::TcpStream::connect((session.profile.host.as_str(), session.profile.port))
                .await?;
        if config.nodelay {
            let _ = socket.set_nodelay(true);
        }
        let socket = socket.into_std()?;
        *session.socket.lock().unwrap() = Some(socket.try_clone()?);
        client::connect_stream(config, tokio::net::TcpStream::from_std(socket)?, handler).await
    };
    let mut ssh = tokio::time::timeout(Duration::from_secs(150), connecting)
        .await
        .map_err(|_| "Connection timed out".to_string())?
        .map_err(|_| "SSH connection or host-key verification failed".to_string())?;
    session.status(Status::Authenticating);
    if !matches!(session.profile.auth, AuthMethod::None)
        && secret.expose().is_empty()
        && let Some(reference) = session.profile.credential_ref.clone()
    {
        secret = tokio::task::spawn_blocking(move || credentials::load(&reference))
            .await
            .map_err(|_| "Keychain task failed".to_string())??;
    }
    let result = match &session.profile.auth {
        AuthMethod::Password => {
            ssh.authenticate_password(&session.profile.username, secret.expose())
                .await
        }
        AuthMethod::None => ssh.authenticate_none(&session.profile.username).await,
        AuthMethod::PrivateKey { path } => {
            let path = path.clone();
            let passphrase = Secret::new(secret.expose().to_string());
            let key = tokio::task::spawn_blocking(move || {
                russh::keys::load_secret_key(
                    path,
                    if passphrase.expose().is_empty() {
                        None
                    } else {
                        Some(passphrase.expose())
                    },
                )
            })
            .await
            .map_err(|_| "Private-key task failed".to_string())?
            .map_err(|_| "Cannot read private key or incorrect passphrase".to_string())?;
            let hash = ssh
                .best_supported_rsa_hash()
                .await
                .map_err(|_| "Key negotiation failed".to_string())?
                .flatten();
            ssh.authenticate_publickey(
                &session.profile.username,
                PrivateKeyWithHashAlg::new(Arc::new(key), hash),
            )
            .await
        }
    }
    .map_err(|_| "Authentication transport failed".to_string())?;
    if !result.success() {
        return Err("Authentication rejected".into());
    }
    if remember && !matches!(session.profile.auth, AuthMethod::None) {
        let reference = session
            .profile
            .credential_ref
            .clone()
            .ok_or("Missing keychain reference")?;
        tokio::task::spawn_blocking(move || credentials::save(&reference, &secret))
            .await
            .map_err(|_| "Keychain task failed".to_string())??;
    }
    connected(session, ssh, commands, outgoing, terminal).await
}
async fn connected(
    session: Arc<Session>,
    ssh: client::Handle<Handler>,
    mut commands: mpsc::Receiver<Command>,
    mut outgoing: mpsc::Receiver<Vec<u8>>,
    terminal: bool,
) -> Result<(), String> {
    let ssh = Arc::new(ssh);
    *session.transport.lock().unwrap() = Some(ssh.clone());
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
        if ssh.is_closed() {
            break;
        }
        tokio::select! {
            result=async {writing.as_mut().expect("writer guarded by select").await},if writing.is_some()=>{
                writing=None;writer=None;session.terminal_finished(result.err());
            },
            _=openings.join_next(),if !openings.is_empty()=>{},
            _ = tokio::time::sleep(Duration::from_secs(1)), if reader.is_none() => {if ssh.is_closed() {break;}}
            command = commands.recv() => match command {
                Some(Command::Resize(cols,rows)) => { session.state.lock().unwrap().terminal.resize(cols as usize,rows as usize); if let Some(writer)=&writer && writer.window_change(cols,rows,0,0).await.is_err() {session.terminal_finished(Some("Terminal resize failed".into()));} }
                Some(Command::Open(reply)) => {
                    let ssh=ssh.clone();
                    openings.spawn(async move {
                        let result=tokio::time::timeout(Duration::from_secs(30),ssh.channel_open_session()).await.map_err(|_|"Channel open timed out".to_string()).and_then(|result|result.map_err(|_|"Server refused channel".to_string()));
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
                    reader=None;writer=None;writing=None;
                    session.terminal_finished(Some("Server refused PTY or shell (SFTP-only hosts need the SFTP view)".into()));
                },
                Some(ChannelMsg::Close) | None => {
                    reader=None;writer=None;writing=None;session.terminal_finished(None);
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
