//! Per-user named pipe, owner-only protected DACL, and server SID verification.
use super::*;
use std::{os::windows::io::AsRawHandle, ptr};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, LocalFree},
    Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    },
    Security::{GetTokenInformation, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser},
    System::{
        Pipes::GetNamedPipeServerProcessId,
        Threading::{
            GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
        },
    },
};
struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn process_sid(process: HANDLE) -> io::Result<String> {
    unsafe {
        let mut token = ptr::null_mut();
        if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = Handle(token);
        let mut needed = 0;
        GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut needed);
        if needed == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buffer = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
        if GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let mut text = ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut len = 0;
        while *text.add(len) != 0 {
            len += 1
        }
        let sid = String::from_utf16(std::slice::from_raw_parts(text, len))
            .map_err(|_| io::Error::other("Invalid SID"));
        LocalFree(text.cast());
        sid
    }
}
fn identity() -> io::Result<(String, String)> {
    let sid = process_sid(unsafe { GetCurrentProcess() })?;
    let name = format!(r"\\.\pipe\HelixLane-{sid}");
    Ok((sid, name))
}
fn verify_server(client: &impl AsRawHandle, sid: &str) -> io::Result<()> {
    unsafe {
        let mut pid = 0;
        if GetNamedPipeServerProcessId(client.as_raw_handle().cast(), &mut pid) == 0 {
            return Err(io::Error::last_os_error());
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return Err(io::Error::last_os_error());
        }
        let process = Handle(process);
        if process_sid(process.0)? != sid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Server SID mismatch",
            ));
        }
        Ok(())
    }
}
fn pipe(name: &str, sid: &str, first: bool) -> io::Result<NamedPipeServer> {
    unsafe {
        let descriptor: Vec<u16> = format!("O:{sid}D:P(A;;GA;;;{sid})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut security = ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            descriptor.as_ptr(),
            1,
            &mut security,
            ptr::null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: security,
            bInheritHandle: 0,
        };
        let result = ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .max_instances(QUEUE_CAPACITY + 1)
            .create_with_security_attributes_raw(
                name,
                (&attributes as *const SECURITY_ATTRIBUTES)
                    .cast_mut()
                    .cast(),
            );
        LocalFree(security);
        result
    }
}
pub(super) fn forward(request: &OpenRequest) -> Result<bool, String> {
    let (sid, name) = identity().map_err(|_| failure())?;
    let deadline = std::time::Instant::now() + DEADLINE;
    crate::tasks::runtime().block_on(async {
        tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), async {
            let Some(mut client) =
                retry::open_until(deadline, || ClientOptions::new().open(&name)).await?
            else {
                return Ok(false);
            };
            verify_server(&client, &sid)?;
            if std::time::Instant::now() >= deadline {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "Launch deadline"));
            }
            write_request(&mut client, request).await?;
            Ok(true)
        })
        .await
        .map_err(|_| failure())?
        .map_err(|_: io::Error| failure())
    })
}
pub(super) fn start(
    queue: SyncSender<OpenRequest>,
    requests: Receiver<OpenRequest>,
    sender: SyncSender<OpenRequest>,
) -> Result<Server, String> {
    let (sid, name) = identity().map_err(|_| failure())?;
    let _entered = crate::tasks::runtime().enter();
    let first = pipe(&name, &sid, true).map_err(|_| failure())?;
    let task = serve(first, name, sid, queue);
    Ok(Server {
        requests,
        sender,
        task,
    })
}
fn serve(
    first: NamedPipeServer,
    name: String,
    sid: String,
    queue: SyncSender<OpenRequest>,
) -> tokio::task::JoinHandle<()> {
    crate::tasks::runtime().spawn(async move {
        let slots = std::sync::Arc::new(tokio::sync::Semaphore::new(QUEUE_CAPACITY));
        let mut current = first;
        loop {
            // Reserve before accepting: 32 active handles plus this listener are
            // the entire 33-instance budget. Saturation never creates a 34th.
            let Ok(slot) = admission::reserve(&slots).await else {
                break;
            };
            if current.connect().await.is_err() {
                let _ = current.disconnect();
                drop(slot);
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            }
            let deadline = std::time::Instant::now() + DEADLINE;
            // Dropping Tokio's pipe owner can leave its OS instance alive until
            // cancelled overlapped I/O completes. Retain this accepted client
            // while ERROR_PIPE_BUSY clears; logical permits alone do not prove
            // that CreateNamedPipe has room. Other errors never enter a retry.
            let next = match retry::open_until(deadline, || pipe(&name, &sid, false)).await {
                Ok(Some(next)) => next,
                _ => {
                    // Keep the owned instance as the listener if replacement
                    // creation fails or this transaction's deadline expires.
                    let _ = current.disconnect();
                    drop(slot);
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    continue;
                }
            };
            let mut connected = std::mem::replace(&mut current, next);
            let queue = queue.clone();
            tokio::spawn(async move {
                // Replacement wait and frame/ack share one absolute budget.
                let _ = tokio::time::timeout_at(
                    tokio::time::Instant::from_std(deadline),
                    receive(&mut connected, &queue),
                )
                .await;
                admission::release(connected, slot);
            });
        }
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn held_busy_and_simultaneous_actual_pipe_clients() {
        let (sid, name) = identity().unwrap();
        let name = format!("{name}-test-{}", std::process::id());
        crate::tasks::runtime().block_on(async {
            let _server = pipe(&name, &sid, true).unwrap();
            let mut clients = vec![];
            for _ in 0..8 {
                let name = name.clone();
                clients.push(tokio::spawn(async move {
                    let deadline = std::time::Instant::now() + Duration::from_millis(100);
                    retry::open_until(deadline, || ClientOptions::new().open(&name)).await
                }));
            }
            let mut opened = 0;
            let mut timedout = 0;
            for client in clients {
                match client.await.unwrap() {
                    Ok(Some(_)) => opened += 1,
                    Err(e) if e.kind() == io::ErrorKind::TimedOut => timedout += 1,
                    _ => panic!("busy pipe misclassified"),
                }
            }
            assert_eq!(opened, 1);
            assert_eq!(timedout, 7);
        });
    }
    #[test]
    fn full_transaction_set_keeps_primary_listener_alive() {
        let (sid, name) = identity().unwrap();
        let name = format!("{name}-saturation-{}", std::process::id());
        let (queue, requests) = sync_channel(QUEUE_CAPACITY);
        crate::tasks::runtime().block_on(async {
            let first = pipe(&name, &sid, true).unwrap();
            let task = serve(first, name.clone(), sid.clone(), queue);
            let mut held = vec![];
            for _ in 0..QUEUE_CAPACITY + 1 {
                held.push(
                    retry::open_until(std::time::Instant::now() + DEADLINE, || {
                        ClientOptions::new().open(&name)
                    })
                    .await
                    .unwrap()
                    .unwrap(),
                );
            }
            // All transaction slots and the extra owned listener are held. More
            // clients time out; they cannot terminate the primary accept loop.
            for _ in 0..4 {
                let result = retry::open_until(
                    std::time::Instant::now() + Duration::from_millis(30),
                    || ClientOptions::new().open(&name),
                )
                .await;
                assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
            }
            assert!(!task.is_finished());
            drop(held);
            let mut client = retry::open_until(std::time::Instant::now() + DEADLINE, || {
                ClientOptions::new().open(&name)
            })
            .await
            .unwrap()
            .unwrap();
            verify_server(&client, &sid).unwrap();
            tokio::time::timeout(DEADLINE, write_request(&mut client, &OpenRequest::focus()))
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(
                requests.try_recv().unwrap().target,
                super::super::super::Target::Focus
            ));
            assert!(!task.is_finished());
            task.abort();
        });
    }

    #[test]
    fn accepted_request_survives_delayed_pipe_instance_release() {
        let (sid, name) = identity().unwrap();
        let name = format!("{name}-delayed-release-{}", std::process::id());
        let (queue, requests) = sync_channel(QUEUE_CAPACITY);
        crate::tasks::runtime().block_on(async {
            let first = pipe(&name, &sid, true).unwrap();
            // Validate ownership before full capacity can mask the guard error.
            assert_eq!(pipe(&name, &sid, true).unwrap_err().raw_os_error(), Some(5));
            // Connect to the primary before occupying the remaining instances.
            // These handles model OS instances retained by pending I/O after a
            // transaction's Rust value and logical permit have been dropped.
            let mut client = ClientOptions::new().open(&name).unwrap();
            verify_server(&client, &sid).unwrap();
            let held: Vec<_> = (0..QUEUE_CAPACITY)
                .map(|_| pipe(&name, &sid, false).unwrap())
                .collect();
            // Separately verify that all physical pipe instances are occupied.
            assert_eq!(
                pipe(&name, &sid, false).unwrap_err().raw_os_error(),
                Some(231)
            );
            let task = serve(first, name.clone(), sid.clone(), queue);
            let request = OpenRequest::focus();
            let mut sending = Box::pin(write_request(&mut client, &request));
            assert!(
                tokio::time::timeout(Duration::from_millis(50), &mut sending)
                    .await
                    .is_err(),
                "a busy replacement must retain the accepted request"
            );
            assert!(requests.try_recv().is_err());
            assert!(!task.is_finished());
            drop(held);
            tokio::time::timeout(DEADLINE, sending)
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(
                requests.try_recv().unwrap().target,
                super::super::super::Target::Focus
            ));
            assert!(!task.is_finished());
            task.abort();
        });
    }

    #[test]
    fn replacement_wait_and_frame_share_the_original_deadline() {
        let (sid, name) = identity().unwrap();
        let name = format!("{name}-replacement-deadline-{}", std::process::id());
        let (queue, requests) = sync_channel(QUEUE_CAPACITY);
        crate::tasks::runtime().block_on(async {
            let first = pipe(&name, &sid, true).unwrap();
            let mut client = ClientOptions::new().open(&name).unwrap();
            verify_server(&client, &sid).unwrap();
            let held: Vec<_> = (0..QUEUE_CAPACITY)
                .map(|_| pipe(&name, &sid, false).unwrap())
                .collect();
            let start = std::time::Instant::now();
            let task = serve(first, name.clone(), sid.clone(), queue);
            tokio::time::sleep(Duration::from_millis(1500)).await;
            drop(held);
            // No request bytes follow. A new two-second timeout after creating
            // the replacement would incorrectly extend this connection to 3.5s.
            let closed = tokio::time::timeout_at(
                tokio::time::Instant::from_std(start + DEADLINE + Duration::from_millis(900)),
                client.read_u8(),
            )
            .await;
            assert!(start.elapsed() >= DEADLINE - Duration::from_millis(200));
            let error = closed
                .expect("replacement reset the transaction deadline")
                .unwrap_err();
            assert!(matches!(
                error.kind(),
                io::ErrorKind::BrokenPipe | io::ErrorKind::UnexpectedEof
            ));
            assert!(requests.try_recv().is_err());
            assert!(!task.is_finished());
            let mut next = retry::open_until(std::time::Instant::now() + DEADLINE, || {
                ClientOptions::new().open(&name)
            })
            .await
            .unwrap()
            .unwrap();
            verify_server(&next, &sid).unwrap();
            tokio::time::timeout(DEADLINE, write_request(&mut next, &OpenRequest::focus()))
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(
                requests.try_recv().unwrap().target,
                super::super::super::Target::Focus
            ));
            task.abort();
        });
    }
}
