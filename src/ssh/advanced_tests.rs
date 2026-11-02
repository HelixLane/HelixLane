use super::tests::{fixture_key, profile, wait};
use super::*;
use russh::{
    Channel, ChannelId,
    server::{self, Server as _, Session as ServerSession},
};

#[derive(Clone)]
struct Peer {
    password: &'static str,
    interactive: bool,
}
impl server::Server for Peer {
    type Handler = Self;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
        self.clone()
    }
}
impl server::Handler for Peer {
    type Error = russh::Error;
    async fn auth_password(
        &mut self,
        _: &str,
        password: &str,
    ) -> Result<server::Auth, Self::Error> {
        Ok(if password == self.password {
            server::Auth::Accept
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
            if r.next().as_deref() == Some(b"otp-secret") {
                server::Auth::Accept
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
    async fn exec_request(
        &mut self,
        id: ChannelId,
        data: &[u8],
        s: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        s.channel_success(id)?;
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
