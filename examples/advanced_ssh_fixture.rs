//! Development-only advanced SSH protocol fixture for native GUI QA.
//! No OS commands execute. Loopback listeners, deterministic public test key.
//! Password `mfa`, then responses `otp-secret`, `second-factor`; user `fixture`.
use russh::{
    Channel, ChannelId,
    server::{self, Server as _, Session as ServerSession},
};
use std::{
    collections::HashMap,
    io::Write,
    sync::{Arc, Mutex},
    time::Duration,
};
fn fixture_key() -> russh::keys::PrivateKey {
    russh::keys::PrivateKey::new(
        russh::keys::ssh_key::private::KeypairData::Ed25519(
            russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&[7; 32]),
        ),
        "synthetic advanced fixture",
    )
    .unwrap()
}
#[derive(Clone)]
struct Peer {
    password: &'static str,
    interactive: bool,
    shells: Vec<ChannelId>,
    rounds: u8,
    remote: Arc<Mutex<HashMap<u32, tokio::task::JoinHandle<()>>>>,
}
impl server::Server for Peer {
    type Handler = Self;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
        Self {
            password: self.password,
            interactive: self.interactive,
            shells: Vec::new(),
            rounds: 0,
            remote: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        for (_, task) in self.remote.lock().unwrap().drain() {
            task.abort();
        }
    }
}
impl server::Handler for Peer {
    type Error = russh::Error;
    async fn auth_publickey(
        &mut self,
        _: &str,
        key: server::VerifiedPublicKey<'_>,
    ) -> Result<server::Auth, Self::Error> {
        Ok(
            if key.public_key.key_data() == fixture_key().public_key().key_data() {
                if self.password == "mfa" {
                    server::Auth::Reject {
                        proceed_with_methods: Some(
                            [russh::MethodKind::KeyboardInteractive].as_slice().into(),
                        ),
                        partial_success: true,
                    }
                } else {
                    server::Auth::Accept
                }
            } else {
                server::Auth::reject()
            },
        )
    }
    async fn tcpip_forward(
        &mut self,
        address: &str,
        port: &mut u32,
        s: &mut ServerSession,
    ) -> Result<bool, Self::Error> {
        let listener = match tokio::net::TcpListener::bind((address, *port as u16)).await {
            Ok(l) => l,
            Err(_) => return Ok(false),
        };
        *port = listener.local_addr().unwrap().port().into();
        let assigned = *port;
        let address = address.to_owned();
        let handle = s.handle();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut tcp, origin)) = listener.accept().await else {
                    break;
                };
                let handle = handle.clone();
                let address = address.clone();
                tokio::spawn(async move {
                    if let Ok(channel) = handle
                        .channel_open_forwarded_tcpip(
                            address,
                            assigned,
                            origin.ip().to_string(),
                            origin.port().into(),
                        )
                        .await
                    {
                        let mut stream = channel.into_stream();
                        let _ = tokio::io::copy_bidirectional(&mut tcp, &mut stream).await;
                    }
                });
            }
        });
        self.remote.lock().unwrap().insert(*port, task);
        // Hold the global response so cancellation can race allocation deterministically.
        tokio::time::sleep(Duration::from_millis(150)).await;
        Ok(true)
    }
    async fn cancel_tcpip_forward(
        &mut self,
        _: &str,
        port: u32,
        _: &mut ServerSession,
    ) -> Result<bool, Self::Error> {
        if let Some(task) = self.remote.lock().unwrap().remove(&port) {
            task.abort();
            Ok(true)
        } else {
            Ok(false)
        }
    }
    async fn auth_password(
        &mut self,
        _: &str,
        password: &str,
    ) -> Result<server::Auth, Self::Error> {
        Ok(if password == self.password {
            if self.password == "mfa" {
                server::Auth::Reject {
                    proceed_with_methods: Some(
                        [russh::MethodKind::KeyboardInteractive].as_slice().into(),
                    ),
                    partial_success: true,
                }
            } else {
                server::Auth::Accept
            }
        } else {
            server::Auth::reject()
        })
    }
    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        _: &str,
        _: &str,
        response: Option<server::Response<'a>>,
    ) -> Result<server::Auth, Self::Error> {
        Ok(if !self.interactive {
            server::Auth::reject()
        } else if let Some(mut r) = response {
            if r.next().as_deref()
                == Some(if self.rounds == 0 {
                    b"otp-secret".as_slice()
                } else {
                    b"second-factor".as_slice()
                })
            {
                self.rounds += 1;
                if self.password == "mfa" && self.rounds == 1 {
                    server::Auth::Partial {
                        name: "Confirm".into(),
                        instructions: "Second prompt".into(),
                        prompts: vec![("Confirmation".into(), false)].into(),
                    }
                } else {
                    server::Auth::Accept
                }
            } else {
                server::Auth::reject()
            }
        } else {
            server::Auth::Partial {
                name: "Challenge".into(),
                instructions: "Per-host OTP".into(),
                prompts: vec![("OTP".into(), false)].into(),
            }
        })
    }
    async fn channel_open_session(
        &mut self,
        _: Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }
    async fn pty_request(
        &mut self,
        id: ChannelId,
        _: &str,
        _: u32,
        _: u32,
        _: u32,
        _: u32,
        _: &[(russh::Pty, u32)],
        s: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        s.channel_success(id)?;
        Ok(())
    }
    async fn shell_request(
        &mut self,
        id: ChannelId,
        s: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        self.shells.push(id);
        s.channel_success(id)?;
        s.data(id, b"ready\r\n".as_slice())?;
        Ok(())
    }
    async fn data(
        &mut self,
        id: ChannelId,
        data: &[u8],
        s: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        if self.shells.contains(&id) {
            if data == b"exit-fixture" {
                s.close(id)?;
                return Ok(());
            }
            s.data(id, data.to_vec())?;
        }
        Ok(())
    }
    async fn exec_request(
        &mut self,
        id: ChannelId,
        data: &[u8],
        s: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        s.channel_success(id)?;
        if let Some(port) = std::str::from_utf8(data)
            .ok()
            .and_then(|s| s.strip_prefix("forward:"))
            .and_then(|s| s.parse::<u32>().ok())
        {
            let handle = s.handle();
            tokio::spawn(async move {
                let result = handle
                    .channel_open_forwarded_tcpip("127.0.0.1", port, "127.0.0.1", 1)
                    .await;
                let text = if result.is_err() {
                    b"rejected".as_slice()
                } else {
                    b"accepted".as_slice()
                };
                let _ = handle.data(id, text.to_vec()).await;
                let _ = handle.close(id).await;
            });
            return Ok(());
        }
        s.data(id, data.to_vec())?;
        s.eof(id)?;
        s.close(id)?;
        Ok(())
    }
    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<server::Msg>,
        host: &str,
        port: u32,
        _: &str,
        _: u32,
        reply: server::ChannelOpenHandle,
        _: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        let Ok(mut tcp) = tokio::net::TcpStream::connect((host, port as u16)).await else {
            return Ok(());
        };
        reply.accept().await;
        tokio::spawn(async move {
            let mut stream = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut tcp, &mut stream).await;
        });
        Ok(())
    }
}
#[tokio::main]
async fn main() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("SSH listener");
    let echo = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("echo listener");
    println!(
        "HELIXLANE_FIXTURE_PORT={}",
        listener.local_addr().unwrap().port()
    );
    println!("HELIXLANE_ECHO_PORT={}", echo.local_addr().unwrap().port());
    println!(
        "HELIXLANE_FIXTURE_FINGERPRINT={}",
        fixture_key()
            .public_key()
            .fingerprint(russh::keys::HashAlg::Sha256)
    );
    println!(
        "User fixture; password mfa; interactive responses otp-secret then second-factor. Selecting Interactive also starts those prompts directly."
    );
    std::io::stdout().flush().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = echo.accept().await.unwrap();
            tokio::spawn(async move {
                let (mut r, mut w) = stream.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    let config = Arc::new(server::Config {
        keys: vec![fixture_key()],
        auth_rejection_time: Duration::ZERO,
        ..Default::default()
    });
    Peer {
        password: "mfa",
        interactive: true,
        shells: Vec::new(),
        rounds: 0,
        remote: Arc::new(Mutex::new(HashMap::new())),
    }
    .run_on_socket(config, &listener)
    .await
    .expect("SSH fixture");
}
