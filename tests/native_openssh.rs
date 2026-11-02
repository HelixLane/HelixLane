//! Explicit full-daemon integration. Run via scripts/native_openssh.py on Linux.
//! Ignored in the portable suite: invoking it without provisioning must fail.
#![cfg(target_os = "linux")]
use helixlane::{
    sftp::Endpoint,
    ssh::{
        Service, Session, Status, TerminalStatus,
        credentials::Secret,
        forward::{Destination, ForwardSpec, ForwardStatus},
        host_keys::HostKeys,
        route::RouteSnapshot,
    },
    workspace::{AuthMethod, ConnectionProfile, TransportPolicy, Workspace},
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
async fn connected(session: &Arc<Session>, password: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let (status, prompt) = {
            let s = session.state.lock().unwrap();
            (s.status.clone(), s.authentication.clone())
        };
        if let Some(prompt) = prompt {
            session.answer_auth(
                prompt
                    .prompts
                    .iter()
                    .map(|_| Secret::new(password.into()))
                    .collect(),
            );
        }
        match status {
            Status::Connected => return,
            Status::Failed(e) => panic!("sshd authentication failed: {e}"),
            Status::HostKey { .. } => panic!("Fixture host key was not explicitly pinned"),
            _ => {}
        }
        assert!(Instant::now() < deadline, "sshd authentication deadline");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
async fn listening(job: &helixlane::ssh::forward::ForwardJob) -> std::net::SocketAddr {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match job.status.lock().unwrap().clone() {
            ForwardStatus::Listening(a) => return a,
            ForwardStatus::Failed(e) => panic!("native forward: {e}"),
            _ => {}
        }
        assert!(Instant::now() < deadline, "forward deadline");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
#[test]
#[ignore = "requires provisioned full OpenSSH daemon; scripts/native_openssh.py"]
fn full_sshd_password_key_interactive_agent_sftp_only_jump_and_forwards() {
    let config: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            std::env::var_os("HELIXLANE_NATIVE_CONFIG").expect("run scripts/native_openssh.py"),
        )
        .unwrap(),
    )
    .unwrap();
    let text = |k| config[k].as_str().unwrap();
    let port = config["port"].as_u64().unwrap() as u16;
    let sftp_port = config["sftp_port"].as_u64().unwrap() as u16;
    let mut keys = HostKeys::default();
    for port in [port, sftp_port] {
        keys.trust("127.0.0.1", port, text("fingerprint").into());
    }
    let keys_path = std::path::Path::new(text("root")).join("client-known-keys.json");
    keys.save(&keys_path).unwrap();
    let service = Service::new(keys_path);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut profile: ConnectionProfile = Workspace::demo_fixture()
        .resources
        .remove(0)
        .profiles
        .remove(0);
    profile.host = "127.0.0.1".into();
    profile.port = port;
    profile.username = text("user").into();
    profile.transport_policy = TransportPolicy::Independent;
    runtime.block_on(async {
        let mut key_profile = profile.clone();
        key_profile.auth = AuthMethod::PrivateKey {
            path: text("key").into(),
        };
        for (index, auth) in [
            AuthMethod::Password,
            key_profile.auth.clone(),
            AuthMethod::KeyboardInteractive,
            AuthMethod::Agent,
        ]
        .into_iter()
        .enumerate()
        {
            let mut p = profile.clone();
            p.id = 100 + index as u64;
            p.auth = auth;
            let session = service.connect_transport(p, Secret::new(text("password").into()), false);
            connected(&session, text("password")).await;
            let result = service
                .exec(session.clone(), b"printf native-sshd-ok".to_vec())
                .result
                .await
                .unwrap()
                .unwrap();
            assert_eq!(result.bytes, b"native-sshd-ok");
            assert_eq!(result.exit_status, Some(0));
            let endpoint = Endpoint::connect(session.clone()).await.unwrap();
            let path = format!("{}/native-{index}.txt", text("home"));
            let mut writer = endpoint.create_new(&path).await.unwrap();
            writer.write_all(b"real OpenSSH SFTP").await.unwrap();
            writer.shutdown().await.unwrap();
            drop(writer);
            assert_eq!(endpoint.read(&path).await.unwrap(), b"real OpenSSH SFTP");
            endpoint.remove(&path).await.unwrap();
            session.disconnect();
        }
        let mut sftp_profile = key_profile.clone();
        sftp_profile.port = sftp_port;
        let session =
            service.connect_transport(sftp_profile.clone(), Secret::new(String::new()), false);
        connected(&session, text("password")).await;
        assert!(
            !Endpoint::connect(session.clone())
                .await
                .unwrap()
                .list(text("home"))
                .await
                .unwrap()
                .is_empty()
        );
        session.disconnect();
        let shell = service.connect(sftp_profile, Secret::new(String::new()), false);
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            {
                let state = shell.state.lock().unwrap();
                if state.terminal_status == TerminalStatus::Finished {
                    assert!(
                        state.terminal.text().to_lowercase().contains("sftp")
                            || state.exit_status == Some(1),
                        "SFTP-only shell must explicitly refuse shell use"
                    );
                    break;
                }
                if let Status::Failed(error) = &state.status {
                    assert!(
                        error == "PTY request failed"
                            || error == "Shell request failed (this may be an SFTP-only host)",
                        "Unexpected SFTP-only failure: {error}"
                    );
                    break;
                }
            }
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        shell.disconnect();
        let mut jumped = key_profile.clone();
        jumped.route = Some(RouteSnapshot {
            hops: vec![key_profile.clone()],
            error: None,
        });
        let session = service.connect_transport(jumped, Secret::new(String::new()), false);
        connected(&session, text("password")).await;
        assert_eq!(
            service
                .exec(session.clone(), b"printf jumped".to_vec())
                .result
                .await
                .unwrap()
                .unwrap()
                .bytes,
            b"jumped"
        );
        let echo = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let echo_port = echo.local_addr().unwrap().port();
        let echo_task = tokio::spawn(async move {
            loop {
                let (mut stream, _) = echo.accept().await.unwrap();
                tokio::spawn(async move {
                    let (mut r, mut w) = stream.split();
                    let _ = tokio::io::copy(&mut r, &mut w).await;
                });
            }
        });
        for spec in [
            ForwardSpec::Local {
                bind: ForwardSpec::loopback(0),
                target: Destination {
                    host: "127.0.0.1".into(),
                    port: echo_port,
                },
            },
            ForwardSpec::Remote {
                bind: "0.0.0.0:0".parse().unwrap(),
                target: Destination {
                    host: "127.0.0.1".into(),
                    port: echo_port,
                },
            },
        ] {
            let job = service.forward(session.clone(), spec);
            let address = listening(&job).await;
            let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", address.port()))
                .await
                .unwrap();
            stream.write_all(b"native-forward").await.unwrap();
            let mut bytes = [0; 14];
            tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut bytes))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(&bytes, b"native-forward");
            drop(stream);
            job.stop();
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                if *job.status.lock().unwrap() == ForwardStatus::Stopped {
                    break;
                }
                assert!(Instant::now() < deadline, "forward stop");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
        echo_task.abort();
        session.disconnect();
    });
    println!(
        "PASS full OpenSSH: password/key/keyboard-interactive/raw-key agent, SFTP read/write, SFTP-only, jump route, local and wildcard remote forward"
    );
}
