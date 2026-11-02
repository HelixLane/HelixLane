use super::*;
use crate::workspace::AuthMethod;
use russh::{
    Channel, ChannelId,
    server::{self, Server as _, Session as ServerSession},
};
#[derive(Clone)]
struct Fixture {
    shell: bool,
}
impl server::Server for Fixture {
    type Handler = Self;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
        self.clone()
    }
}
impl server::Handler for Fixture {
    type Error = russh::Error;
    async fn auth_password(
        &mut self,
        _: &str,
        password: &str,
    ) -> Result<server::Auth, Self::Error> {
        Ok(if password == "correct" {
            server::Auth::Accept
        } else {
            server::Auth::reject()
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
        if self.shell {
            s.channel_success(id)?;
        } else {
            s.channel_failure(id)?;
        }
        Ok(())
    }
    async fn shell_request(
        &mut self,
        id: ChannelId,
        s: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        if !self.shell {
            s.channel_failure(id)?;
            return Ok(());
        }
        s.channel_success(id)?;
        s.data(id, b"\x1b[32mwire-ready\x1b[0m\r\n".as_slice())?;
        Ok(())
    }
    async fn data(
        &mut self,
        id: ChannelId,
        data: &[u8],
        s: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        s.data(id, data.to_vec())?;
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
        s.exit_status_request(id, 0)?;
        s.eof(id)?;
        s.close(id)?;
        Ok(())
    }
}
pub(super) fn profile(port: u16) -> ConnectionProfile {
    ConnectionProfile {
        ephemeral: false,
        id: 99,
        host: "127.0.0.1".into(),
        port,
        username: "tester".into(),
        credential_ref: None,
        jump_resource: None,
        route: None,
        transport_policy: Default::default(),
        auth: AuthMethod::Password,
    }
}
pub(super) async fn wait(session: &Session, predicate: impl Fn(&SessionState) -> bool) {
    tokio::time::timeout(std::time::Duration::from_secs(8), async {
        loop {
            if predicate(&session.state.lock().unwrap()) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("session state timed out");
}
#[test]
fn wire_session_requires_trust_then_streams_and_never_changes_target() {
    let path = std::env::temp_dir().join(format!(
        "helix-hostkeys-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let service = Service::new(path.clone());
    service.runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let key = fixture_key();
        let config = Arc::new(server::Config {
            keys: vec![key],
            auth_rejection_time: std::time::Duration::ZERO,
            ..Default::default()
        });
        let server = tokio::spawn(async move {
            Fixture { shell: true }
                .run_on_socket(config, &listener)
                .await
                .unwrap()
        });
        let session = service.connect(profile(port), Secret::new("correct".into()), false);
        wait(&session, |s| matches!(s.status, Status::HostKey { .. })).await;
        assert!(!path.exists(), "unapproved key must never be persisted");
        session.trust(true);
        wait(&session, |s| {
            s.status == Status::Connected && s.terminal.text().contains("wire-ready")
        })
        .await;
        let task = service.exec(session.clone(), b"captured-command".to_vec());
        let output = task.result.await.unwrap().unwrap();
        assert_eq!(output.bytes, b"captured-command");
        assert_eq!(output.exit_status, Some(0));
        session.input(b"immutable target".to_vec());
        wait(&session, |s| s.terminal.text().contains("immutable target")).await;
        assert_eq!(session.profile, profile(port));
        session.disconnect();
        wait(&session, |s| s.status == Status::Disconnected).await;
        server.abort();
    });
    let _ = std::fs::remove_file(path);
}

#[test]
fn sftp_only_transport_opens_channels_without_requesting_a_pty() {
    let path = std::env::temp_dir().join(format!(
        "helix-sftp-only-{}.json",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let service = Service::new(path.clone());
    service.runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let key = fixture_key();
        let config = Arc::new(server::Config {
            keys: vec![key],
            ..Default::default()
        });
        let server = tokio::spawn(async move {
            Fixture { shell: false }
                .run_on_socket(config, &listener)
                .await
                .unwrap()
        });
        let session =
            service.connect_transport(profile(port), Secret::new("correct".into()), false);
        wait(&session, |s| matches!(s.status, Status::HostKey { .. })).await;
        session.trust(true);
        wait(&session, |s| s.status == Status::Connected).await;
        let mut channel = session.open_channel().await.unwrap();
        channel
            .exec(true, b"independent-channel".as_slice())
            .await
            .unwrap();
        let mut bytes = Vec::new();
        while let Some(message) = channel.wait().await {
            if let russh::ChannelMsg::Data { data } = message {
                bytes.extend_from_slice(&data);
            }
        }
        assert_eq!(bytes, b"independent-channel");
        session.disconnect();
        server.abort();
    });
    let _ = std::fs::remove_file(path);
}
#[test]
fn changed_key_rejection_and_authentication_errors_fail_closed() {
    let path = std::env::temp_dir().join(format!(
        "helix-change-{}.json",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let service = Service::new(path.clone());
    service.runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let key = fixture_key();
        let config = Arc::new(server::Config {
            keys: vec![key],
            auth_rejection_time: std::time::Duration::ZERO,
            ..Default::default()
        });
        let server = tokio::spawn(async move {
            Fixture { shell: true }
                .run_on_socket(config, &listener)
                .await
                .unwrap()
        });
        let mut keys = HostKeys::default();
        keys.trust("127.0.0.1", port, "old-key".into());
        keys.save(&path).unwrap();
        let rejected = service.connect(profile(port), Secret::new("correct".into()), false);
        wait(
            &rejected,
            |s| matches!(&s.status,Status::HostKey {previous:Some(old),..} if old=="old-key"),
        )
        .await;
        rejected.trust(false);
        wait(&rejected, |s| matches!(s.status, Status::Failed(_))).await;
        assert_eq!(
            HostKeys::load(&path)
                .unwrap()
                .verify("127.0.0.1", port, "old-key"),
            Verification::Trusted
        );
        let wrong_auth = service.connect(profile(port), Secret::new("incorrect".into()), false);
        wait(&wrong_auth, |s| matches!(s.status, Status::HostKey { .. })).await;
        wrong_auth.trust(true);
        wait(
            &wrong_auth,
            |s| matches!(&s.status,Status::Failed(error) if error=="Authentication rejected"),
        )
        .await;
        // A separate attempt is a separate identity even for the identical profile.
        assert_ne!(rejected.id, wrong_auth.id);
        server.abort();
    });
    let _ = std::fs::remove_file(path);
}
#[test]
fn cancellation_before_host_approval_does_not_wait_for_network_timeout() {
    let service = Service::new(std::env::temp_dir().join("unused-helix-hosts.json"));
    service.runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let session = service.connect(profile(port), Secret::new(String::new()), false);
        // The remote never sends an SSH version; cancellation must interrupt handshake.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        session.disconnect();
        wait(&session, |s| s.status == Status::Disconnected).await;
    });
}
#[test]
fn premature_channel_request_cannot_cancel_a_connection_attempt() {
    let service = Service::new(std::env::temp_dir().join("unused-helix-hosts.json"));
    service.runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let session = service.connect(
            profile(listener.local_addr().unwrap().port()),
            Secret::new(String::new()),
            false,
        );
        assert!(session.open_channel().await.is_err());
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        assert_eq!(session.state.lock().unwrap().status, Status::Connecting);
        session.disconnect();
        wait(&session, |s| s.status == Status::Disconnected).await;
    });
}

pub(super) fn fixture_key() -> russh::keys::PrivateKey {
    russh::keys::PrivateKey::new(
        russh::keys::ssh_key::private::KeypairData::Ed25519(
            russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&[7; 32]),
        ),
        "synthetic fixture",
    )
    .unwrap()
}

#[test]
fn rsa_private_key_load_and_sha2_authentication_survive_shell_dependency_alignment() {
    use std::sync::Mutex;
    #[derive(Clone)]
    struct RsaPeer {
        expected: russh::keys::PublicKey,
        verified: Arc<Mutex<Option<russh::keys::Algorithm>>>,
    }
    impl server::Server for RsaPeer {
        type Handler = Self;
        fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
            self.clone()
        }
    }
    impl server::Handler for RsaPeer {
        type Error = russh::Error;
        async fn auth_publickey(
            &mut self,
            user: &str,
            key: server::VerifiedPublicKey<'_>,
        ) -> Result<server::Auth, Self::Error> {
            if user == "tester" && key.public_key.key_data() == self.expected.key_data() {
                *self.verified.lock().unwrap() = Some(key.algorithm.clone());
                Ok(server::Auth::Accept)
            } else {
                Ok(server::Auth::reject())
            }
        }
    }
    let key_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rsa_test_key");
    let key = russh::keys::load_secret_key(&key_path, None).expect("load RSA private key");
    let pkcs8_path = key_path.with_file_name("rsa_test_key_pkcs8");
    let pkcs8_key = russh::keys::load_secret_key(&pkcs8_path, None).expect("load PKCS#8 RSA key");
    assert_eq!(
        pkcs8_key.public_key().key_data(),
        key.public_key().key_data()
    );
    assert!(matches!(
        key.algorithm(),
        russh::keys::Algorithm::Rsa { .. }
    ));
    let path = std::env::temp_dir().join(format!(
        "helix-rsa-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let service = Service::new(path.clone());
    service.runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let verified = Arc::new(Mutex::new(None));
        let peer = RsaPeer {
            expected: key.public_key().clone(),
            verified: verified.clone(),
        };
        let config = Arc::new(server::Config {
            keys: vec![key],
            auth_rejection_time: Duration::ZERO,
            ..Default::default()
        });
        let server =
            tokio::spawn(
                async move { peer.clone().run_on_socket(config, &listener).await.unwrap() },
            );
        let mut target = profile(port);
        target.auth = AuthMethod::PrivateKey {
            path: key_path.to_string_lossy().into_owned(),
        };
        let session = service.connect_transport(target, Secret::new(String::new()), false);
        wait(&session, |state| {
            matches!(state.status, Status::HostKey { .. })
        })
        .await;
        session.trust(true);
        wait(&session, |state| {
            matches!(state.status, Status::Connected | Status::Failed(_))
        })
        .await;
        assert_eq!(session.state.lock().unwrap().status, Status::Connected);
        assert!(matches!(
            *verified.lock().unwrap(),
            Some(russh::keys::Algorithm::Rsa {
                hash: Some(HashAlg::Sha256 | HashAlg::Sha512)
            })
        ));
        session.disconnect();
        wait(&session, |state| state.status == Status::Disconnected).await;
        server.abort();
    });
    let _ = std::fs::remove_file(path);
}

#[test]
fn single_channel_rejects_exec_while_pty_remains_usable() {
    let path = std::env::temp_dir().join(format!("helix-single-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let service = Service::new(path.clone());
    service.runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let config = Arc::new(server::Config {
            keys: vec![fixture_key()],
            ..Default::default()
        });
        let server = tokio::spawn(async move {
            Fixture { shell: true }
                .run_on_socket(config, &listener)
                .await
                .unwrap()
        });
        let mut p = profile(port);
        p.transport_policy = crate::workspace::TransportPolicy::SingleChannel;
        let session = service.connect(p, Secret::new("correct".into()), false);
        wait(&session, |s| matches!(s.status, Status::HostKey { .. })).await;
        session.trust(true);
        wait(&session, |s| s.status == Status::Connected).await;
        assert!(
            session.open_channel().await.is_err(),
            "extra exec/SFTP bypassed single channel policy"
        );
        session.input(b"still-usable".to_vec());
        wait(&session, |s| s.terminal.text().contains("still-usable")).await;
        session.disconnect();
        server.abort();
    });
    let _ = std::fs::remove_file(path);
}
