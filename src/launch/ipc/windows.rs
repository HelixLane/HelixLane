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
            let next = match pipe(&name, &sid, false) {
                Ok(next) => next,
                Err(_) => {
                    // Keep the owned instance as the listener if replacement
                    // creation fails. A transient OS limit must not kill IPC.
                    let _ = current.disconnect();
                    drop(slot);
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    continue;
                }
            };
            let mut connected = std::mem::replace(&mut current, next);
            let queue = queue.clone();
            tokio::spawn(async move {
                let _ = tokio::time::timeout(DEADLINE, receive(&mut connected, &queue)).await;
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
}
