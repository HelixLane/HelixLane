use super::*;
use crate::{
    ssh::{Service, Status, credentials::Secret},
    tasks::{Conflict, JobState, TransferManager, TransferSpec},
    workspace::{AuthMethod, ConnectionProfile},
};
use russh::server::Server as _;
#[path = "../../examples/support/sftp_server.rs"]
mod fixture;
#[test]
fn openssh_backend_operations_atomic_upload_external_conflict_and_sftp_only_transport() {
    assert!(
        fixture::server_binary().exists(),
        "OpenSSH sftp-server is required for this integration test"
    );
    let root = std::env::temp_dir().join(format!("helixlane-openssh-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    let service = Service::new(root.join("known.json"));
    crate::tasks::runtime().block_on(async {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let server_root = root.clone();
        let server = tokio::spawn(async move {
            let key = fixture::key();
            fixture::Fixture::new(server_root)
                .run_on_socket(
                    std::sync::Arc::new(russh::server::Config {
                        keys: vec![key],
                        auth_rejection_time: std::time::Duration::ZERO,
                        ..Default::default()
                    }),
                    &listener,
                )
                .await
                .unwrap();
        });
        let profile = ConnectionProfile {
            ephemeral: false,
            id: 700,
            host: "127.0.0.1".into(),
            port,
            username: "fixture".into(),
            auth: AuthMethod::None,
            credential_ref: None,
            jump_resource: None,
            route: None,
            transport_policy: Default::default(),
        };
        let session = service.connect_transport(profile.clone(), Secret::new(String::new()), false);
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let status = session.state.lock().unwrap().status.clone();
                match status {
                    Status::HostKey { .. } => session.trust(true),
                    Status::Connected => break,
                    Status::Failed(e) => panic!("{e}"),
                    _ => {}
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(service.resumable(&profile).len(), 1);
        let remote = Endpoint::connect(session.clone()).await.unwrap();
        remote.mkdir("folder").await.unwrap();
        let local = root.join("upload-source");
        std::fs::write(&local, b"first body").unwrap();
        let m = TransferManager::new(2);
        let spec = TransferSpec::capture(
            Endpoint::Local,
            local.display().to_string(),
            remote.clone(),
            "folder/data".into(),
            Conflict::Overwrite,
        )
        .await
        .unwrap();
        let id = m.submit_paused(spec);
        m.run(id).await;
        assert_eq!(m.jobs()[0].state, JobState::Complete);
        assert_eq!(remote.read("folder/data").await.unwrap(), b"first body");
        std::fs::write(&local, b"second body").unwrap();
        let spec = TransferSpec::capture(
            Endpoint::Local,
            local.display().to_string(),
            remote.clone(),
            "folder/data".into(),
            Conflict::Overwrite,
        )
        .await
        .unwrap();
        let id = m.submit_paused(spec.clone());
        m.cancel(id);
        m.run(id).await;
        assert_eq!(remote.read("folder/data").await.unwrap(), b"first body");
        let id = m.submit_paused(spec);
        m.run(id).await;
        assert_eq!(remote.read("folder/data").await.unwrap(), b"second body");
        assert!(
            !remote
                .list("folder")
                .await
                .unwrap()
                .iter()
                .any(|e| e.name.contains(".part"))
        );
        remote
            .rename("folder/data", "folder/renamed")
            .await
            .unwrap();
        remote.chmod("folder/renamed", 0o600).await.unwrap();
        assert_eq!(
            remote
                .stat("folder/renamed")
                .await
                .unwrap()
                .unwrap()
                .permissions
                & 0o777,
            0o600
        );
        let lease = super::editor::EditLease::download(
            remote.clone(),
            "folder/renamed".into(),
            &root.join("edits"),
        )
        .await
        .unwrap();
        std::fs::write(root.join("folder/renamed"), b"other content").unwrap();
        assert!(
            lease
                .upload_spec()
                .await
                .err()
                .unwrap()
                .contains("Remote file changed")
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc", root.join("folder/link")).unwrap();
            assert_eq!(
                remote.stat("folder/link").await.unwrap().unwrap().kind,
                Kind::Symlink
            );
            assert!(remote.reader("folder/link").await.is_err());
        }
        remote.remove("folder/renamed").await.unwrap();
        assert!(remote.stat("folder/renamed").await.unwrap().is_none());
        // External endpoints remain live and usable, but neither queued nor failed
        // transfer captures may disclose their temporary host in the disk journal.
        let mut temporary_profile = profile.clone();
        temporary_profile.id = 701;
        temporary_profile.ephemeral = true;
        let temporary =
            service.connect_transport(temporary_profile, Secret::new(String::new()), false);
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                match temporary.state.lock().unwrap().status.clone() {
                    Status::HostKey { .. } => temporary.trust(true),
                    Status::Connected => break,
                    Status::Failed(e) => panic!("{e}"),
                    _ => {}
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let temporary_remote = Endpoint::connect(temporary.clone()).await.unwrap();
        let journal = root.join("temporary-transfer-journal.json");
        let temporary_manager = TransferManager::with_journal(1, journal.clone()).unwrap();
        let queued = TransferSpec::capture(
            Endpoint::Local,
            local.display().to_string(),
            temporary_remote,
            "folder/temporary-target".into(),
            Conflict::Overwrite,
        )
        .await
        .unwrap();
        let queued_id = temporary_manager.submit_paused(queued);
        assert_eq!(temporary_manager.jobs()[0].state, JobState::Queued);
        assert_eq!(std::fs::read_to_string(&journal).unwrap(), "[]");
        std::fs::write(&local, b"changed after immutable submission").unwrap();
        temporary_manager.run(queued_id).await;
        assert!(matches!(
            temporary_manager.jobs()[0].state,
            JobState::Failed(_)
        ));
        assert_eq!(std::fs::read_to_string(&journal).unwrap(), "[]");
        assert_eq!(temporary.state.lock().unwrap().status, Status::Connected);
        temporary.disconnect();
        assert_eq!(session.state.lock().unwrap().status, Status::Connected);
        session.disconnect();
        server.abort();
    });
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn openssh_two_host_copy_uses_local_staging_and_cancellation_preserves_remote_original() {
    let base = std::env::temp_dir().join(format!("helixlane-two-openssh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let service = Service::new(base.join("keys.json"));
    crate::tasks::runtime().block_on(async {
        let mut endpoints = vec![];
        let mut servers = vec![];
        for n in 1..=2 {
            let root = base.join(format!("host{n}"));
            std::fs::create_dir(&root).unwrap();
            std::fs::write(root.join("source"), b"actual two-host OpenSSH relay").unwrap();
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .unwrap();
            let port = listener.local_addr().unwrap().port();
            servers.push(tokio::spawn(async move {
                fixture::Fixture::new(root)
                    .run_on_socket(
                        std::sync::Arc::new(russh::server::Config {
                            keys: vec![fixture::key()],
                            auth_rejection_time: std::time::Duration::ZERO,
                            ..Default::default()
                        }),
                        &listener,
                    )
                    .await
                    .unwrap();
            }));
            let profile = ConnectionProfile {
                ephemeral: false,
                id: 800 + n,
                host: "127.0.0.1".into(),
                port,
                username: "fixture".into(),
                auth: AuthMethod::None,
                credential_ref: None,
                jump_resource: None,
                route: None,
                transport_policy: Default::default(),
            };
            let session = service.connect_transport(profile, Secret::new(String::new()), false);
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    let status = session.state.lock().unwrap().status.clone();
                    match status {
                        Status::HostKey { .. } => session.trust(true),
                        Status::Connected => break,
                        Status::Failed(e) => panic!("{e}"),
                        _ => {}
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            endpoints.push(Endpoint::connect(session).await.unwrap());
        }
        let manager = TransferManager::new(1);
        let spec = TransferSpec::capture(
            endpoints[0].clone(),
            "source".into(),
            endpoints[1].clone(),
            "relayed".into(),
            Conflict::Overwrite,
        )
        .await
        .unwrap();
        let id = manager.submit_paused(spec);
        assert!(manager.jobs()[0].staging.is_some());
        manager.run(id).await;
        assert_eq!(manager.jobs()[0].state, JobState::Complete);
        assert_eq!(
            endpoints[1].read("relayed").await.unwrap(),
            b"actual two-host OpenSSH relay"
        );
        assert!(!std::path::Path::new(manager.jobs()[0].staging.as_ref().unwrap()).exists());
        let source = base.join("big-local");
        std::fs::write(&source, vec![17; 4 * 1024 * 1024]).unwrap();
        std::fs::write(base.join("host2/cancel-target"), b"preserved original").unwrap();
        let spec = TransferSpec::capture(
            Endpoint::Local,
            source.display().to_string(),
            endpoints[1].clone(),
            "cancel-target".into(),
            Conflict::Overwrite,
        )
        .await
        .unwrap();
        let id = manager.submit_paused(spec);
        let runner = manager.clone();
        let running = tokio::spawn(async move {
            runner.run(id).await;
        });
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if manager.jobs()[1].bytes > 0 {
                    manager.cancel(id);
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        running.await.unwrap();
        assert_eq!(manager.jobs()[1].state, JobState::Cancelled);
        assert_eq!(
            endpoints[1].read("cancel-target").await.unwrap(),
            b"preserved original"
        );
        assert!(
            !endpoints[1]
                .list(".")
                .await
                .unwrap()
                .iter()
                .any(|e| e.name.ends_with(".part"))
        );
        for endpoint in endpoints {
            if let Endpoint::Remote { transport, .. } = endpoint {
                transport.disconnect();
            }
        }
        for server in servers {
            server.abort();
        }
    });
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn openssh_regular_file_never_replaces_nonempty_directory() {
    let root = std::env::temp_dir().join(format!(
        "helixlane-openssh-type-conflict-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("dst")).unwrap();
    std::fs::write(root.join("dst/original"), b"original-directory-content").unwrap();
    std::fs::write(root.join("source"), b"new-file").unwrap();
    let service = Service::new(root.join("keys.json"));
    crate::tasks::runtime().block_on(async {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let remote_root = root.clone();
        let server = tokio::spawn(async move {
            fixture::Fixture::new(remote_root)
                .run_on_socket(
                    std::sync::Arc::new(russh::server::Config {
                        keys: vec![fixture::key()],
                        auth_rejection_time: std::time::Duration::ZERO,
                        ..Default::default()
                    }),
                    &listener,
                )
                .await
                .unwrap();
        });
        let session = service.connect_transport(
            ConnectionProfile {
                ephemeral: false,
                id: 990,
                host: "127.0.0.1".into(),
                port,
                username: "fixture".into(),
                auth: AuthMethod::None,
                credential_ref: None,
                jump_resource: None,
                route: None,
                transport_policy: Default::default(),
            },
            Secret::new(String::new()),
            false,
        );
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let status = session.state.lock().unwrap().status.clone();
                match status {
                    Status::HostKey { .. } => session.trust(true),
                    Status::Connected => break,
                    Status::Failed(e) => panic!("{e}"),
                    _ => {}
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let remote = Endpoint::connect(session.clone()).await.unwrap();
        let captured = TransferSpec::capture(
            Endpoint::Local,
            root.join("source").display().to_string(),
            remote.clone(),
            "dst".into(),
            Conflict::Overwrite,
        )
        .await;
        assert!(
            captured
                .err()
                .unwrap()
                .contains("Incompatible destination type")
        );
        assert_eq!(
            remote.stat("dst").await.unwrap().unwrap().kind,
            Kind::Directory
        );
        assert_eq!(
            remote.read("dst/original").await.unwrap(),
            b"original-directory-content"
        );
        assert!(
            !remote
                .list(".")
                .await
                .unwrap()
                .iter()
                .any(|e| e.name.contains(".part"))
        );
        session.disconnect();
        server.abort();
    });
    let _ = std::fs::remove_dir_all(root);
}
