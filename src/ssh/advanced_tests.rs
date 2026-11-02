use super::tests::{fixture_key, profile, wait};
use super::*;
use russh::{
    Channel, ChannelId,
    server::{self, Server as _, Session as ServerSession},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
async fn peer(password: &'static str, interactive: bool) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = Arc::new(server::Config {
        keys: vec![fixture_key()],
        auth_rejection_time: Duration::ZERO,
        ..Default::default()
    });
    let task = tokio::spawn(async move {
        Peer {
            password,
            interactive,
            shells: Vec::new(),
            rounds: 0,
            remote: Arc::new(Mutex::new(HashMap::new())),
        }
        .run_on_socket(config, &listener)
        .await
        .unwrap();
    });
    (port, task)
}
fn path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "helix-5b-{name}-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}
#[test]
fn nested_jump_trust_and_secrets_are_per_hop_and_data_reaches_final_host() {
    let path = path("jump");
    let service = Service::new(path.clone());
    service.runtime.block_on(async {
        let (first, a) = peer("jump-password", false).await;
        let (second, b) = peer("middle-password", false).await;
        let (last, c) = peer("final-password", false).await;
        let mut p = profile(last);
        p.jump_resource = Some(1);
        p.route = Some(route::RouteSnapshot {
            hops: vec![profile(first), profile(second)],
            error: None,
        });
        let session = service.connect_transport(p, Secret::new("final-password".into()), false);
        for (port, password) in [
            (first, "jump-password"),
            (second, "middle-password"),
            (last, "final-password"),
        ] {
            wait(&session, |s| {
                matches!(s.status, Status::HostKey { .. })
                    && s.hop.as_ref().is_some_and(|h| h.port == port)
            })
            .await;
            session.trust(true);
            if port != last {
                wait(&session, |s| s.authentication.is_some()).await;
                let prompt = session
                    .state
                    .lock()
                    .unwrap()
                    .authentication
                    .clone()
                    .unwrap();
                assert_eq!(prompt.hop.port, port);
                assert!(!format!("{prompt:?}").contains("final-password"));
                session.answer_auth(vec![Secret::new(password.into())]);
            }
        }
        wait(&session, |s| s.status == Status::Connected).await;
        assert_eq!(
            service
                .exec(session.clone(), b"nested-data".to_vec())
                .result
                .await
                .unwrap()
                .unwrap()
                .bytes,
            b"nested-data"
        );
        session.disconnect();
        wait(&session, |s| s.status == Status::Disconnected).await;
        a.abort();
        b.abort();
        c.abort();
    });
    let _ = std::fs::remove_file(path);
}
#[test]
fn interactive_prompt_targets_host_rejects_wrong_count_and_cancels() {
    let path = path("interactive");
    let service = Service::new(path.clone());
    service.runtime.block_on(async {
        let (port, server) = peer("unused", true).await;
        let mut p = profile(port);
        p.auth = AuthMethod::KeyboardInteractive;
        let session = service.connect_transport(p.clone(), Secret::new(String::new()), false);
        wait(&session, |s| matches!(s.status, Status::HostKey { .. })).await;
        session.trust(true);
        wait(&session, |s| s.authentication.is_some()).await;
        let prompt = session
            .state
            .lock()
            .unwrap()
            .authentication
            .clone()
            .unwrap();
        assert_eq!(prompt.hop.port, port);
        assert_eq!(prompt.prompts, vec![("OTP".into(), false)]);
        session.answer_auth(vec![Secret::new("otp-secret".into())]);
        wait(&session, |s| s.status == Status::Connected).await;
        session.disconnect();
        let session = service.connect_transport(p.clone(), Secret::new(String::new()), false);
        wait(&session, |s| s.authentication.is_some()).await;
        session.answer_auth(vec![]);
        wait(
            &session,
            |s| matches!(&s.status,Status::Failed(e) if e.contains("count mismatch")),
        )
        .await;
        let session = service.connect_transport(p, Secret::new(String::new()), false);
        wait(&session, |s| s.authentication.is_some()).await;
        session.disconnect();
        wait(&session, |s| {
            s.status == Status::Disconnected && s.authentication.is_none()
        })
        .await;
        server.abort();
    });
    let _ = std::fs::remove_file(path);
}
#[test]
fn held_stream_retains_single_channel_capacity_until_drop() {
    let path = path("held");
    let service = Service::new(path.clone());
    service.runtime.block_on(async {
        let (port, server) = peer("correct", false).await;
        let mut p = profile(port);
        p.transport_policy = crate::workspace::TransportPolicy::SingleChannel;
        let session = service.connect_transport(p, Secret::new("correct".into()), false);
        wait(&session, |s| matches!(s.status, Status::HostKey { .. })).await;
        session.trust(true);
        wait(&session, |s| s.status == Status::Connected).await;
        let stream = session.open_channel().await.unwrap().into_stream();
        assert!(session.open_channel().await.is_err());
        drop(stream);
        assert!(session.open_channel().await.is_ok());
        session.disconnect();
        server.abort();
    });
    let _ = std::fs::remove_file(path);
}

async fn ready(service: &Service, port: u16) -> Arc<Session> {
    let session = service.connect_transport(profile(port), Secret::new("correct".into()), false);
    wait(&session, |s| matches!(s.status, Status::HostKey { .. })).await;
    session.trust(true);
    wait(&session, |s| s.status == Status::Connected).await;
    session
}
async fn listening(job: &forward::ForwardJob) -> std::net::SocketAddr {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match &*job.status.lock().unwrap() {
                forward::ForwardStatus::Listening(address) => return *address,
                forward::ForwardStatus::Failed(error) => panic!("forward failed: {error}"),
                _ => {}
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}
async fn stopped(job: &forward::ForwardJob) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if *job.status.lock().unwrap() == forward::ForwardStatus::Stopped {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
async fn echo() -> (u16, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        loop {
            let (mut tcp, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let (mut r, mut w) = tcp.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    (port, task)
}
async fn roundtrip(tcp: &mut tokio::net::TcpStream) {
    tcp.write_all(b"forward-data").await.unwrap();
    let mut data = [0; 12];
    tokio::time::timeout(Duration::from_secs(3), tcp.read_exact(&mut data))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&data, b"forward-data");
}
#[test]
fn local_remote_and_socks_forward_actual_data_and_stop_owned_streams() {
    let path = path("forward");
    let service = Service::new(path.clone());
    service.runtime.block_on(async {
        let (port, server) = peer("correct", false).await;
        let session = ready(&service, port).await;
        let (echo_port, echo) = echo().await;
        let target = forward::Destination {
            host: "127.0.0.1".into(),
            port: echo_port,
        };
        let bind = forward::ForwardSpec::loopback(0);
        for spec in [
            forward::ForwardSpec::Local {
                bind,
                target: target.clone(),
            },
            forward::ForwardSpec::Remote {
                bind,
                target: target.clone(),
            },
            forward::ForwardSpec::Socks { bind },
        ] {
            let socks = matches!(spec, forward::ForwardSpec::Socks { .. });
            let job = service.forward(session.clone(), spec);
            let address = listening(&job).await;
            assert!(address.ip().is_loopback());
            assert_ne!(address.port(), 0);
            let mut tcp = tokio::net::TcpStream::connect(address).await.unwrap();
            if socks {
                tcp.write_all(&[5, 1, 0]).await.unwrap();
                let mut reply = [0; 2];
                tcp.read_exact(&mut reply).await.unwrap();
                assert_eq!(reply, [5, 0]);
                let mut request = vec![5, 1, 0, 1, 127, 0, 0, 1];
                request.extend(echo_port.to_be_bytes());
                tcp.write_all(&request).await.unwrap();
                let mut reply = [0; 10];
                tcp.read_exact(&mut reply).await.unwrap();
                assert_eq!(reply[1], 0);
            }
            roundtrip(&mut tcp).await;
            job.stop();
            stopped(&job).await;
            let mut byte = [0];
            assert!(matches!(
                tokio::time::timeout(Duration::from_secs(2), tcp.read(&mut byte))
                    .await
                    .unwrap(),
                Ok(0) | Err(_)
            ));
            assert!(tokio::net::TcpStream::connect(address).await.is_err());
            assert_eq!(
                service
                    .exec(session.clone(), b"still-connected".to_vec())
                    .result
                    .await
                    .unwrap()
                    .unwrap()
                    .bytes,
                b"still-connected"
            );
        }
        session.disconnect();
        server.abort();
        echo.abort();
    });
    let _ = std::fs::remove_file(path);
}
#[test]
fn remote_stop_during_request_cancels_allocated_listener() {
    let path = path("remote-cancel");
    let service = Service::new(path.clone());
    service.runtime.block_on(async {
        let (port, server) = peer("correct", false).await;
        let session = ready(&service, port).await;
        let reserve = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = reserve.local_addr().unwrap();
        drop(reserve);
        let job = service.forward(
            session.clone(),
            forward::ForwardSpec::Remote {
                bind: address,
                target: forward::Destination {
                    host: "127.0.0.1".into(),
                    port: 9,
                },
            },
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        job.stop();
        stopped(&job).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            tokio::net::TcpStream::connect(address).await.is_err(),
            "cancelled remote request leaked a server listener"
        );
        session.disconnect();
        server.abort();
    });
    let _ = std::fs::remove_file(path);
}
#[test]
fn agent_protocol_lists_and_signs_without_exporting_private_key() {
    use russh::keys::{
        signature::Signer,
        ssh_encoding::{Decode, Encode},
    };
    let path = path("agent");
    let service = Service::new(path.clone());
    service.runtime.block_on(async {
        let (port, server) = peer("correct", false).await;
        let mut keys = HostKeys::default();
        keys.trust(
            "127.0.0.1",
            port,
            fixture_key()
                .public_key()
                .fingerprint(HashAlg::Sha256)
                .to_string(),
        );
        keys.save(&path).unwrap();
        // A detached presentation owner; this direct transport injects an agent protocol stream.
        let owner = service.connect_transport(profile(1), Secret::new(String::new()), false);
        wait(&owner, |s| matches!(s.status, Status::Failed(_))).await;
        for (fail_sign, mfa) in [(false, false), (true, false), (false, true)] {
            let extra = if mfa {
                Some(peer("mfa", true).await)
            } else {
                None
            };
            let port = extra.as_ref().map(|(port, _)| *port).unwrap_or(port);
            keys.trust(
                "127.0.0.1",
                port,
                fixture_key()
                    .public_key()
                    .fingerprint(HashAlg::Sha256)
                    .to_string(),
            );
            keys.save(&path).unwrap();
            let handler = Handler {
                session: owner.clone(),
                keys: path.clone(),
                profile: profile(port),
                destination: true,
            };
            let mut ssh = client::connect(
                Arc::new(client::Config::default()),
                ("127.0.0.1", port),
                handler,
            )
            .await
            .unwrap();
            let (transport, mut peer) = tokio::io::duplex(8192);
            let signer = tokio::spawn(async move {
                let key = fixture_key();
                let mut seen = Vec::new();
                for _ in 0..2 {
                    let size = peer.read_u32().await.unwrap();
                    assert!(size < 8192);
                    let mut packet = vec![0; size as usize];
                    peer.read_exact(&mut packet).await.unwrap();
                    seen.push(packet[0]);
                    let mut response = Vec::new();
                    match packet[0] {
                        11 => {
                            response.push(12);
                            response.extend(1u32.to_be_bytes());
                            key.public_key()
                                .to_bytes()
                                .unwrap()
                                .encode(&mut response)
                                .unwrap();
                            "fixture-agent".encode(&mut response).unwrap();
                        }
                        13 => {
                            if fail_sign {
                                response.push(5);
                            } else {
                                let mut input = &packet[1..];
                                let _key = Vec::<u8>::decode(&mut input).unwrap();
                                let data = Vec::<u8>::decode(&mut input).unwrap();
                                let signature: russh::keys::ssh_key::Signature =
                                    key.try_sign(&data).unwrap();
                                let mut encoded = Vec::new();
                                signature.encode(&mut encoded).unwrap();
                                response.push(14);
                                encoded.encode(&mut response).unwrap();
                            }
                        }
                        _ => panic!("unexpected agent message"),
                    }
                    peer.write_u32(response.len() as u32).await.unwrap();
                    peer.write_all(&response).await.unwrap();
                }
                seen
            });
            let mut agent = russh::keys::agent::client::AgentClient::connect(transport);
            let result = auth::authenticate_agent(&mut ssh, "tester", &mut agent).await;
            if fail_sign {
                assert_eq!(result.unwrap_err(), "SSH agent signing failed");
            } else if mfa {
                let result = result.unwrap();
                assert!(matches!(
                    result,
                    client::AuthResult::Failure {
                        partial_success: true,
                        ..
                    }
                ));
                let hop = auth::HopIdentity {
                    host: "127.0.0.1".into(),
                    port,
                    username: "tester".into(),
                    index: 0,
                    total: 1,
                };
                let answers = async {
                    wait(&owner, |s| s.authentication.is_some()).await;
                    assert_eq!(
                        owner
                            .state
                            .lock()
                            .unwrap()
                            .authentication
                            .as_ref()
                            .unwrap()
                            .hop
                            .port,
                        port
                    );
                    owner.answer_auth(vec![Secret::new("otp-secret".into())]);
                    wait(&owner, |s| {
                        s.authentication
                            .as_ref()
                            .is_some_and(|p| p.prompts[0].0 == "Confirmation")
                    })
                    .await;
                    owner.answer_auth(vec![Secret::new("second-factor".into())]);
                };
                let (completed, ()) = tokio::join!(
                    auth::finish_auth(&owner, &mut ssh, "tester", &hop, result),
                    answers
                );
                assert!(completed.unwrap());
            } else {
                assert!(result.unwrap().success());
            }
            assert_eq!(signer.await.unwrap(), vec![11, 13]);
            if let Some((_, server)) = extra {
                server.abort();
            }
            let _ = ssh
                .disconnect(russh::Disconnect::ByApplication, "done", "en")
                .await;
        }
        server.abort();
    });
    let _ = std::fs::remove_file(path);
}

#[test]
fn intermediate_hop_cannot_inject_into_final_remote_forward() {
    let path = path("forward-origin");
    let service = Service::new(path.clone());
    service.runtime.block_on(async {
        let (first, a) = peer("correct", false).await;
        let (last, b) = peer("correct", false).await;
        let mut p = profile(last);
        p.jump_resource = Some(1);
        p.route = Some(route::RouteSnapshot {
            hops: vec![profile(first)],
            error: None,
        });
        let session = service.connect_transport(p, Secret::new("correct".into()), false);
        wait(&session, |s| matches!(s.status, Status::HostKey { .. })).await;
        session.trust(true);
        wait(&session, |s| s.authentication.is_some()).await;
        session.answer_auth(vec![Secret::new("correct".into())]);
        wait(&session, |s| {
            matches!(s.status, Status::HostKey { .. })
                && s.hop.as_ref().is_some_and(|h| h.port == last)
        })
        .await;
        session.trust(true);
        wait(&session, |s| s.status == Status::Connected).await;
        let (echo_port, echo) = echo().await;
        let job = service.forward(
            session.clone(),
            forward::ForwardSpec::Remote {
                bind: forward::ForwardSpec::loopback(0),
                target: forward::Destination {
                    host: "127.0.0.1".into(),
                    port: echo_port,
                },
            },
        );
        let address = listening(&job).await;
        let hop = session.hops.lock().unwrap()[0].clone();
        let mut channel = hop.channel_open_session().await.unwrap();
        channel
            .exec(true, format!("forward:{}", address.port()))
            .await
            .unwrap();
        let mut bytes = Vec::new();
        while let Some(event) = channel.wait().await {
            if let ChannelMsg::Data { data } = event {
                bytes.extend_from_slice(&data);
            }
        }
        assert_eq!(
            bytes, b"rejected",
            "an intermediate SSH hop injected a final-host forwarded channel"
        );
        let mut tcp = tokio::net::TcpStream::connect(address).await.unwrap();
        roundtrip(&mut tcp).await;
        job.stop();
        stopped(&job).await;
        session.disconnect();
        a.abort();
        b.abort();
        echo.abort();
    });
    let _ = std::fs::remove_file(path);
}

#[test]
fn shared_terminal_channels_keep_distinct_ids_screens_and_independent_policy() {
    let path = path("shared");
    let service = Service::new(path.clone());
    service.runtime.block_on(async {
        let (port, server) = peer("correct", false).await;
        let transport = ready(&service, port).await;
        let first = service.connect(profile(port), Secret::new(String::new()), false);
        let second = service.connect(profile(port), Secret::new(String::new()), false);
        wait(&first, |s| s.terminal_status == TerminalStatus::Running).await;
        wait(&second, |s| s.terminal_status == TerminalStatus::Running).await;
        assert_ne!(first.id, second.id);
        assert!(Arc::ptr_eq(
            first.transport.lock().unwrap().as_ref().unwrap(),
            second.transport.lock().unwrap().as_ref().unwrap()
        ));
        first.input(b"first-screen".to_vec());
        second.input(b"second-screen".to_vec());
        wait(&first, |s| s.terminal.text().contains("first-screen")).await;
        wait(&second, |s| s.terminal.text().contains("second-screen")).await;
        assert!(
            !first
                .state
                .lock()
                .unwrap()
                .terminal
                .text()
                .contains("second-screen")
        );
        first.input(b"exit-fixture".to_vec());
        wait(&first, |s| s.terminal_status == TerminalStatus::Finished).await;
        assert_eq!(
            service
                .exec(second.clone(), b"survivor".to_vec())
                .result
                .await
                .unwrap()
                .unwrap()
                .bytes,
            b"survivor"
        );
        let mut p = profile(port);
        p.transport_policy = crate::workspace::TransportPolicy::Independent;
        let independent = service.connect_transport(p, Secret::new("correct".into()), false);
        wait(&independent, |s| s.status == Status::Connected).await;
        assert!(!Arc::ptr_eq(
            transport.transport.lock().unwrap().as_ref().unwrap(),
            independent.transport.lock().unwrap().as_ref().unwrap()
        ));
        transport.disconnect();
        wait(&second, |s| s.status == Status::Disconnected).await;
        assert_eq!(independent.state.lock().unwrap().status, Status::Connected);
        independent.disconnect();
        server.abort();
    });
    let _ = std::fs::remove_file(path);
}
#[test]
fn socks_rejects_bind_auth_and_occupied_channel_without_disrupting_stream() {
    let path = path("socks-policy");
    let service = Service::new(path.clone());
    service.runtime.block_on(async {
        let (port, server) = peer("correct", false).await;
        let mut p = profile(port);
        p.transport_policy = crate::workspace::TransportPolicy::SingleChannel;
        let session = service.connect_transport(p, Secret::new("correct".into()), false);
        wait(&session, |s| matches!(s.status, Status::HostKey { .. })).await;
        session.trust(true);
        wait(&session, |s| s.status == Status::Connected).await;
        let job = service.forward(
            session.clone(),
            forward::ForwardSpec::Socks {
                bind: forward::ForwardSpec::loopback(0),
            },
        );
        let address = listening(&job).await;
        let mut unsupported = tokio::net::TcpStream::connect(address).await.unwrap();
        unsupported.write_all(&[5, 1, 2]).await.unwrap();
        let mut auth_reply = [0; 2];
        unsupported.read_exact(&mut auth_reply).await.unwrap();
        assert_eq!(auth_reply, [5, 255]);
        let (echo_port, echo) = echo().await;
        let mut held = None;
        for (command, expected) in [(2, 7), (1, 0), (1, 5)] {
            let mut tcp = tokio::net::TcpStream::connect(address).await.unwrap();
            tcp.write_all(&[5, 1, 0]).await.unwrap();
            tcp.read_exact(&mut auth_reply).await.unwrap();
            let mut request = vec![5, command, 0, 1, 127, 0, 0, 1];
            request.extend(echo_port.to_be_bytes());
            tcp.write_all(&request).await.unwrap();
            let mut reply = [0; 10];
            tcp.read_exact(&mut reply).await.unwrap();
            assert_eq!(reply[1], expected);
            if expected == 0 {
                roundtrip(&mut tcp).await;
                held = Some(tcp);
                assert!(session.open_channel().await.is_err());
            }
        }
        roundtrip(held.as_mut().unwrap()).await;
        session.disconnect();
        stopped(&job).await;
        assert!(tokio::net::TcpStream::connect(address).await.is_err());
        server.abort();
        echo.abort();
    });
    let _ = std::fs::remove_file(path);
}

#[test]
fn partial_primary_auth_continues_with_two_distinct_interactive_rounds() {
    let path = path("mfa");
    let service = Service::new(path.clone());
    service.runtime.block_on(async {
        let (port, server) = peer("mfa", true).await;
        let session = service.connect_transport(profile(port), Secret::new("mfa".into()), false);
        wait(&session, |s| matches!(s.status, Status::HostKey { .. })).await;
        session.trust(true);
        wait(&session, |s| {
            s.authentication.is_some() || matches!(s.status, Status::Failed(_))
        })
        .await;
        {
            let state = session.state.lock().unwrap();
            assert!(
                state.authentication.is_some(),
                "partial primary success did not continue to MFA: {:?}",
                state.status
            );
        }
        session.answer_auth(vec![Secret::new("otp-secret".into())]);
        wait(&session, |s| {
            s.authentication
                .as_ref()
                .is_some_and(|p| p.prompts[0].0 == "Confirmation")
        })
        .await;
        session.answer_auth(vec![Secret::new("second-factor".into())]);
        wait(&session, |s| s.status == Status::Connected).await;
        session.disconnect();
        server.abort();
    });
    let _ = std::fs::remove_file(path);
}
#[test]
fn explicit_disconnect_from_reused_pty_tears_down_shared_transport() {
    let path = path("shared-disconnect");
    let service = Service::new(path.clone());
    service.runtime.block_on(async {
        let (port, server) = peer("correct", false).await;
        let root = ready(&service, port).await;
        let child = service.connect(profile(port), Secret::new(String::new()), false);
        wait(&child, |s| s.terminal_status == TerminalStatus::Running).await;
        let mut channel = root.open_channel().await.unwrap();
        child.disconnect();
        tokio::time::timeout(Duration::from_millis(700), async {
            loop {
                if root.state.lock().unwrap().status == Status::Disconnected {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("explicit Disconnect left the shared transport and channels alive");
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), channel.wait())
                .await
                .unwrap(),
            None | Some(ChannelMsg::Close)
        ));
        server.abort();
    });
    let _ = std::fs::remove_file(path);
}
