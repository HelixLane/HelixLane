//! Private, bounded single-instance transport. Credentials follow peer verification.
use super::{MAX_FRAME, OpenRequest, decode, encode};
use std::{
    io,
    sync::mpsc::{Receiver, SyncSender, sync_channel},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use zeroize::Zeroizing;
#[cfg(any(windows, test))]
mod admission;
pub mod retry;
#[cfg(windows)]
mod windows;
pub const QUEUE_CAPACITY: usize = 32;
const DEADLINE: Duration = Duration::from_secs(2);
fn failure() -> String {
    "Private launch IPC unavailable / 私有启动通道不可用".into()
}
pub struct Server {
    pub requests: Receiver<OpenRequest>,
    sender: SyncSender<OpenRequest>,
    task: tokio::task::JoinHandle<()>,
    #[cfg(unix)]
    _lock: std::fs::File,
    #[cfg(unix)]
    endpoint: std::path::PathBuf,
}
impl Server {
    pub fn sender(&self) -> SyncSender<OpenRequest> {
        self.sender.clone()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(&self.endpoint);
        }
    }
}
async fn write_request<S: AsyncRead + AsyncWrite + Unpin>(
    s: &mut S,
    r: &OpenRequest,
) -> io::Result<()> {
    let data = encode(r).map_err(|_| io::Error::other("Invalid request"))?;
    s.write_u32(data.len() as u32).await?;
    s.write_all(&data).await?;
    if s.read_u8().await? != 1 {
        return Err(io::Error::other("Launch queue unavailable"));
    }
    Ok(())
}
async fn receive<S: AsyncRead + AsyncWrite + Unpin>(
    s: &mut S,
    queue: &SyncSender<OpenRequest>,
) -> io::Result<()> {
    let len = s.read_u32().await? as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(io::Error::other("Invalid frame"));
    }
    let mut data = Zeroizing::new(vec![0; len]);
    s.read_exact(&mut data).await?;
    let r = decode(&data).map_err(|_| io::Error::other("Invalid request"))?;
    let accepted = queue.try_send(r).is_ok();
    s.write_u8(u8::from(accepted)).await?;
    Ok(())
}
#[cfg(unix)]
fn directory() -> Result<std::path::PathBuf, String> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    let path = std::path::PathBuf::from(format!("/tmp/helixlane-{}", unsafe { libc::geteuid() }));
    match std::fs::DirBuilder::new().mode(0o700).create(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(failure()),
    }
    let m = std::fs::symlink_metadata(&path).map_err(|_| failure())?;
    if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o777 != 0o700 {
        return Err(failure());
    }
    Ok(path)
}
#[cfg(unix)]
fn lock(dir: &std::path::Path) -> Result<std::fs::File, String> {
    use std::os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    };
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(dir.join("instance.lock"))
        .map_err(|_| failure())?;
    let m = file.metadata().map_err(|_| failure())?;
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o777 != 0o600
        || m.nlink() != 1
    {
        return Err(failure());
    }
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(failure());
    }
    Ok(file)
}
#[cfg(unix)]
fn socket_available(dir: &std::path::Path) -> Result<bool, String> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let m = match std::fs::symlink_metadata(dir.join("instance.sock")) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(failure()),
    };
    if !m.file_type().is_socket()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o777 != 0o600
        || m.nlink() != 1
    {
        return Err(failure());
    }
    Ok(true)
}
#[cfg(unix)]
fn peer(s: &tokio::net::UnixStream) -> io::Result<()> {
    if s.peer_cred()?.uid() != unsafe { libc::geteuid() } {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Peer mismatch",
        ));
    }
    Ok(())
}
#[cfg(all(unix, debug_assertions))]
fn test_port() -> Result<Option<u16>, String> {
    std::env::var("HELIXLANE_TEST_IPC_TCP")
        .ok()
        .map(|s| s.parse::<u16>().map_err(|_| failure()))
        .transpose()
}
#[cfg(all(unix, debug_assertions))]
fn private_read(
    dir: &std::path::Path,
    name: &str,
    max: u64,
) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(dir.join(name))
    {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(failure()),
    };
    let m = file.metadata().map_err(|_| failure())?;
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o777 != 0o600
        || m.nlink() != 1
        || m.len() > max
    {
        return Err(failure());
    }
    let mut bytes = Zeroizing::new(Vec::new());
    file.read_to_end(&mut bytes).map_err(|_| failure())?;
    Ok(Some(bytes))
}
#[cfg(all(unix, debug_assertions))]
fn private_write(dir: &std::path::Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let path = dir.join(name);
    match std::fs::symlink_metadata(&path) {
        Ok(m) if m.file_type().is_symlink() => return Err(failure()),
        Ok(_) => std::fs::remove_file(&path).map_err(|_| failure())?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(_) => return Err(failure()),
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| failure())?;
    f.write_all(bytes).map_err(|_| failure())
}
#[cfg(all(unix, debug_assertions))]
fn nonce() -> io::Result<Zeroizing<Vec<u8>>> {
    use std::io::Read;
    let mut v = Zeroizing::new(vec![0; 32]);
    std::fs::File::open("/dev/urandom")?.read_exact(&mut v)?;
    Ok(v)
}
#[cfg(all(unix, debug_assertions))]
fn proof(token: &[u8], role: &[u8], client: &[u8], server: &[u8]) -> hmac::Hmac<sha2::Sha256> {
    use hmac::Mac;
    let mut m = hmac::Hmac::<sha2::Sha256>::new_from_slice(token).expect("HMAC key");
    m.update(role);
    m.update(client);
    m.update(server);
    m
}
#[cfg(all(unix, debug_assertions))]
async fn test_client_auth<S: AsyncRead + AsyncWrite + Unpin>(
    s: &mut S,
    token: &[u8],
) -> io::Result<()> {
    use hmac::Mac;
    let c = nonce()?;
    s.write_all(&c).await?;
    let mut server = [0; 32];
    s.read_exact(&mut server).await?;
    let mut supplied = Zeroizing::new(vec![0; 32]);
    s.read_exact(&mut supplied).await?;
    proof(token, b"server", &c, &server)
        .verify_slice(&supplied)
        .map_err(|_| io::Error::new(io::ErrorKind::PermissionDenied, "Server proof"))?;
    s.write_all(&proof(token, b"client", &c, &server).finalize().into_bytes())
        .await?;
    Ok(())
}
#[cfg(all(unix, debug_assertions))]
async fn test_server_auth<S: AsyncRead + AsyncWrite + Unpin>(
    s: &mut S,
    token: &[u8],
) -> io::Result<()> {
    use hmac::Mac;
    let mut c = [0; 32];
    s.read_exact(&mut c).await?;
    let server = nonce()?;
    s.write_all(&server).await?;
    s.write_all(&proof(token, b"server", &c, &server).finalize().into_bytes())
        .await?;
    let mut supplied = Zeroizing::new(vec![0; 32]);
    s.read_exact(&mut supplied).await?;
    proof(token, b"client", &c, &server)
        .verify_slice(&supplied)
        .map_err(|_| io::Error::new(io::ErrorKind::PermissionDenied, "Client proof"))?;
    Ok(())
}
/// An absent primary returns false; authentication, timeout and security errors never do.
pub fn forward(r: &OpenRequest) -> Result<bool, String> {
    r.validate()?;
    #[cfg(windows)]
    {
        windows::forward(r)
    }
    #[cfg(unix)]
    {
        let dir = directory()?;
        crate::tasks::runtime().block_on(async {
            tokio::time::timeout(DEADLINE, async {
                #[cfg(debug_assertions)]
                if test_port()?.is_some() {
                    let Some(port) = private_read(&dir, "test-ipc-port", 5)? else {
                        return Ok(false);
                    };
                    let port = std::str::from_utf8(&port)
                        .ok()
                        .and_then(|s| s.parse::<u16>().ok())
                        .filter(|p| *p > 0)
                        .ok_or_else(failure)?;
                    let token = private_read(&dir, "test-ipc-token", 32)?
                        .filter(|v| v.len() == 32)
                        .ok_or_else(failure)?;
                    let mut s =
                        match tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
                            .await
                        {
                            Ok(s) => s,
                            Err(e)
                                if matches!(
                                    e.kind(),
                                    io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
                                ) =>
                            {
                                return Ok(false);
                            }
                            Err(_) => return Err(failure()),
                        };
                    test_client_auth(&mut s, &token)
                        .await
                        .map_err(|_| failure())?;
                    write_request(&mut s, r).await.map_err(|_| failure())?;
                    return Ok(true);
                }
                if !socket_available(&dir)? {
                    return Ok(false);
                }
                let mut s = match tokio::net::UnixStream::connect(dir.join("instance.sock")).await {
                    Ok(s) => s,
                    Err(e)
                        if matches!(
                            e.kind(),
                            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                        ) =>
                    {
                        return Ok(false);
                    }
                    Err(_) => return Err(failure()),
                };
                peer(&s).map_err(|_| failure())?;
                write_request(&mut s, r).await.map_err(|_| failure())?;
                Ok(true)
            })
            .await
            .map_err(|_| failure())?
        })
    }
}
pub fn start() -> Result<Server, String> {
    let (queue, requests) = sync_channel(QUEUE_CAPACITY);
    let sender = queue.clone();
    #[cfg(windows)]
    {
        windows::start(queue, requests, sender)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
        let dir = directory()?;
        let lock = lock(&dir)?;
        #[cfg(debug_assertions)]
        if let Some(port) = test_port()? {
            let _entered = crate::tasks::runtime().enter();
            let listener = crate::tasks::runtime()
                .block_on(tokio::net::TcpListener::bind((
                    std::net::Ipv4Addr::LOCALHOST,
                    port,
                )))
                .map_err(|_| failure())?;
            let actual = listener.local_addr().map_err(|_| failure())?.port();
            let token = nonce().map_err(|_| failure())?;
            private_write(&dir, "test-ipc-token", &token)?;
            private_write(&dir, "test-ipc-port", actual.to_string().as_bytes())?;
            let task = crate::tasks::runtime().spawn(async move {
                let slots = std::sync::Arc::new(tokio::sync::Semaphore::new(QUEUE_CAPACITY));
                loop {
                    let Ok((mut stream, _)) = listener.accept().await else {
                        break;
                    };
                    let Ok(slot) = slots.clone().try_acquire_owned() else {
                        continue;
                    };
                    let queue = queue.clone();
                    let token = token.clone();
                    tokio::spawn(async move {
                        let _slot = slot;
                        let _ = tokio::time::timeout(DEADLINE, async {
                            test_server_auth(&mut stream, &token).await?;
                            receive(&mut stream, &queue).await
                        })
                        .await;
                    });
                }
            });
            return Ok(Server {
                requests,
                sender,
                task,
                _lock: lock,
                endpoint: dir.join("test-ipc-port"),
            });
        }
        let endpoint = dir.join("instance.sock");
        match std::fs::symlink_metadata(&endpoint) {
            Ok(m) if m.file_type().is_socket() && m.uid() == unsafe { libc::geteuid() } => {
                std::fs::remove_file(&endpoint).map_err(|_| failure())?
            }
            Ok(_) => return Err(failure()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return Err(failure()),
        }
        let listener = std::os::unix::net::UnixListener::bind(&endpoint).map_err(|_| failure())?;
        std::fs::set_permissions(&endpoint, std::fs::Permissions::from_mode(0o600))
            .map_err(|_| failure())?;
        listener.set_nonblocking(true).map_err(|_| failure())?;
        let _entered = crate::tasks::runtime().enter();
        let listener = tokio::net::UnixListener::from_std(listener).map_err(|_| failure())?;
        let task = crate::tasks::runtime().spawn(async move {
            let slots = std::sync::Arc::new(tokio::sync::Semaphore::new(QUEUE_CAPACITY));
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let Ok(slot) = slots.clone().try_acquire_owned() else {
                    continue;
                };
                if peer(&stream).is_err() {
                    continue;
                }
                let queue = queue.clone();
                tokio::spawn(async move {
                    let _slot = slot;
                    let _ = tokio::time::timeout(DEADLINE, receive(&mut stream, &queue)).await;
                });
            }
        });
        Ok(Server {
            requests,
            sender,
            task,
            _lock: lock,
            endpoint,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn frame_roundtrip_bounds_and_queue_backpressure() {
        let (tx, rx) = sync_channel(QUEUE_CAPACITY);
        let (mut client, mut server) = tokio::io::duplex(MAX_FRAME + 16);
        let queue = tx.clone();
        let receiving = tokio::spawn(async move { receive(&mut server, &queue).await });
        write_request(&mut client, &OpenRequest::focus())
            .await
            .unwrap();
        receiving.await.unwrap().unwrap();
        assert!(matches!(
            rx.try_recv().unwrap().target,
            super::super::Target::Focus
        ));
        for _ in 0..QUEUE_CAPACITY {
            tx.try_send(OpenRequest::focus()).unwrap();
        }
        let (mut client, mut server) = tokio::io::duplex(2048);
        let queue = tx.clone();
        let receiving = tokio::spawn(async move { receive(&mut server, &queue).await });
        assert!(
            write_request(&mut client, &OpenRequest::focus())
                .await
                .is_err()
        );
        receiving.await.unwrap().unwrap();
        let (mut client, mut server) = tokio::io::duplex(16);
        client.write_u32((MAX_FRAME + 1) as u32).await.unwrap();
        assert!(receive(&mut server, &tx).await.is_err());
    }
    #[cfg(all(unix, debug_assertions))]
    #[tokio::test]
    async fn role_separated_mutual_auth_precedes_credentials() {
        let token = nonce().unwrap();
        let wrong = nonce().unwrap();
        let (mut client, mut server) = tokio::io::duplex(2048);
        let key = token.clone();
        let task = tokio::spawn(async move { test_server_auth(&mut server, &key).await });
        test_client_auth(&mut client, &token).await.unwrap();
        task.await.unwrap().unwrap();
        let (mut client, mut server) = tokio::io::duplex(2048);
        let task = tokio::spawn(async move { test_server_auth(&mut server, &token).await });
        assert!(test_client_auth(&mut client, &wrong).await.is_err());
        drop(client);
        assert!(task.await.unwrap().is_err());
    }
    #[cfg(unix)]
    #[test]
    fn owned_directory_lock_and_actual_unix_transport() {
        use std::os::unix::fs::MetadataExt;
        let dir = directory().unwrap();
        assert_eq!(
            std::fs::symlink_metadata(&dir).unwrap().mode() & 0o777,
            0o700
        );
        let first = lock(&dir).unwrap();
        assert!(lock(&dir).is_err());
        drop(first);
        match std::os::unix::net::UnixListener::bind(dir.join("capability-probe.sock")) {
            Ok(probe) => {
                drop(probe);
                std::fs::remove_file(dir.join("capability-probe.sock")).unwrap();
                let server = start().unwrap();
                assert!(forward(&OpenRequest::focus()).unwrap());
                assert!(matches!(
                    server.requests.recv_timeout(DEADLINE).unwrap().target,
                    super::super::Target::Focus
                ));
            }
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                eprintln!("AF_UNIX denied by runtime; native Unix endpoint test unavailable")
            }
            Err(e) => panic!("Unix probe: {e}"),
        }
    }
}

#[cfg(all(test, unix))]
mod socket_tests {
    use super::*;
    #[test]
    fn absent_socket_is_absent_without_connecting_and_symlinks_are_errors() {
        let dir = std::env::temp_dir().join(format!("helix-ipc-metadata-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).unwrap();
        assert!(!socket_available(&dir).unwrap());
        std::fs::write(dir.join("instance.sock"), b"not a socket").unwrap();
        assert!(socket_available(&dir).is_err());
        std::fs::remove_file(dir.join("instance.sock")).unwrap();
        std::os::unix::fs::symlink(dir.join("missing"), dir.join("instance.sock")).unwrap();
        assert!(socket_available(&dir).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
