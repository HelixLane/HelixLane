use super::tests::{fixture_key, profile};
use super::*;
use russh::{
    Channel, ChannelId,
    server::{self, Server as _, Session as ServerSession},
};
use std::sync::atomic::AtomicUsize;
#[derive(Clone, Copy)]
enum Behavior {
    Backpressure,
    StatusBeforeTail,
    WaitForExecEof,
    ShellCloseDuringExec,
}
#[derive(Clone)]
struct Peer {
    behavior: Behavior,
    received: Arc<AtomicUsize>,
    shell: Option<ChannelId>,
    subsystem: Option<ChannelId>,
}
impl server::Server for Peer {
    type Handler = Self;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
        self.clone()
    }
}
impl server::Handler for Peer {
    type Error = russh::Error;
    async fn auth_password(&mut self, _: &str, _: &str) -> Result<server::Auth, Self::Error> {
        Ok(server::Auth::Accept)
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
        s.channel_success(id)?;
        self.shell = Some(id);
        if matches!(self.behavior, Behavior::StatusBeforeTail) {
            s.exit_status_request(id, 7)?;
            s.data(id, b"tail-after-status".as_slice())?;
            s.eof(id)?;
            s.close(id)?;
        } else {
            s.data(id, b"ready\r\n".as_slice())?;
        }
        Ok(())
    }
    // Russh replenishes once before this callback. A target of one disables
    // subsequent automatic adjustment (window < target/2 is never true).
    fn adjust_window(&mut self, _: ChannelId, current: u32) -> u32 {
        if matches!(
            self.behavior,
            Behavior::Backpressure | Behavior::ShellCloseDuringExec
        ) {
            1
        } else {
            current
        }
    }
    async fn data(
        &mut self,
        id: ChannelId,
        data: &[u8],
        s: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        let total = self.received.fetch_add(data.len(), Ordering::SeqCst) + data.len();
        if matches!(self.behavior, Behavior::Backpressure) && total >= 2048 {
            s.data(id, b"window-exhausted-output\x1b[6n".as_slice())?;
            let handle = s.handle();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let _ = handle.data(id, b"after-terminal-reply".to_vec()).await;
            });
        }
        Ok(())
    }
    async fn exec_request(
        &mut self,
        id: ChannelId,
        _: &[u8],
        s: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        s.channel_success(id)?;
        if matches!(self.behavior, Behavior::ShellCloseDuringExec) {
            if let Some(shell) = self.shell.take() {
                s.exit_status_request(shell, 0)?;
                s.eof(shell)?;
                s.close(shell)?;
            }
            let handle = s.handle();
            let subsystem = self.subsystem;
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(150)).await;
                let _ = handle.data(id, b"independent-work-complete".to_vec()).await;
                let _ = handle.exit_status_request(id, 0).await;
                let _ = handle.eof(id).await;
                let _ = handle.close(id).await;
                if let Some(subsystem) = subsystem {
                    let _ = handle
                        .data(subsystem, b"subsystem-still-open".to_vec())
                        .await;
                }
            });
        }
        Ok(())
    }
    async fn subsystem_request(
        &mut self,
        id: ChannelId,
        name: &str,
        s: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        assert_eq!(name, "sftp");
        self.subsystem = Some(id);
        s.channel_success(id)?;
        s.data(id, b"subsystem-ready".as_slice())?;
        Ok(())
    }
    async fn channel_eof(
        &mut self,
        id: ChannelId,
        s: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        if matches!(self.behavior, Behavior::WaitForExecEof) {
            s.exit_status_request(id, 0)?;
            s.data(id, b"stdin-complete".as_slice())?;
            s.eof(id)?;
            s.close(id)?;
        }
        Ok(())
    }
}
fn exercise(behavior: Behavior, work: impl AsyncFn(&Service, Arc<Session>, Arc<AtomicUsize>)) {
    let path = std::env::temp_dir().join(format!(
        "helix-adverse-{}.json",
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
        let mut keys = HostKeys::default();
        keys.trust(
            "127.0.0.1",
            port,
            key.public_key().fingerprint(HashAlg::Sha256).to_string(),
        );
        keys.save(&path).unwrap();
        let received = Arc::new(AtomicUsize::new(0));
        let peer = Peer {
            behavior,
            received: received.clone(),
            shell: None,
            subsystem: None,
        };
        let config = Arc::new(server::Config {
            keys: vec![key],
            window_size: 1024,
            maximum_packet_size: 1024,
            ..Default::default()
        });
        let server =
            tokio::spawn(
                async move { peer.clone().run_on_socket(config, &listener).await.unwrap() },
            );
        let session = if matches!(behavior, Behavior::WaitForExecEof) {
            service.connect_transport(profile(port), Secret::new("correct".into()), false)
        } else {
            service.connect(profile(port), Secret::new("correct".into()), false)
        };
        if !matches!(behavior, Behavior::StatusBeforeTail) {
            until(|| session.state.lock().unwrap().status == Status::Connected).await;
        }
        work(&service, session.clone(), received).await;
        session.disconnect();
        server.abort();
    });
    let _ = std::fs::remove_file(path);
}
async fn until(condition: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("adverse peer condition timed out");
}
#[test]
fn exhausted_remote_window_does_not_stop_inbound_drain_or_channel_open() {
    exercise(Behavior::Backpressure, async |_, session, received| {
        session.input(vec![b'x'; 64 * 1024]);
        until(|| received.load(Ordering::SeqCst) >= 2048).await;
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(
            received.load(Ordering::SeqCst),
            2048,
            "fixture must leave remote window exhausted with transport open"
        );
        until(|| {
            session
                .state
                .lock()
                .unwrap()
                .terminal
                .text()
                .contains("window-exhausted-output")
        })
        .await;
        until(|| {
            session
                .state
                .lock()
                .unwrap()
                .terminal
                .text()
                .contains("after-terminal-reply")
        })
        .await;
        let channel = tokio::time::timeout(Duration::from_millis(500), session.open_channel())
            .await
            .expect("channel open stalled behind outbound data")
            .unwrap();
        let _ = channel.close().await;
    });
}
#[test]
fn explicit_disconnect_interrupts_exhausted_remote_window() {
    exercise(Behavior::Backpressure, async |_, session, received| {
        let mut independent = session.open_channel().await.unwrap();
        session.input(vec![b'x'; 64 * 1024]);
        until(|| received.load(Ordering::SeqCst) >= 2048).await;
        session.disconnect();
        tokio::time::timeout(
            Duration::from_millis(500),
            until(|| session.state.lock().unwrap().status == Status::Disconnected),
        )
        .await
        .expect("disconnect trapped behind remote window");
        let event = tokio::time::timeout(Duration::from_millis(750), independent.wait())
            .await
            .expect("shared channel kept transport alive after explicit disconnect");
        assert!(matches!(event, None | Some(ChannelMsg::Close)));
    });
}
#[test]
fn terminal_drains_tail_after_exit_status_until_channel_close() {
    exercise(Behavior::StatusBeforeTail, async |_, session, _| {
        until(|| session.state.lock().unwrap().terminal_status == TerminalStatus::Finished).await;
        assert_eq!(session.state.lock().unwrap().status, Status::Connected);
        assert_eq!(session.state.lock().unwrap().exit_status, Some(7));
        assert!(session.state.lock().unwrap().remote_eof);
        assert!(
            session
                .state
                .lock()
                .unwrap()
                .terminal
                .text()
                .contains("tail-after-status")
        );
    });
}
#[test]
fn stdinless_exec_signals_eof_and_drains_status_before_tail() {
    exercise(Behavior::WaitForExecEof, async |service, session, _| {
        let task = service.exec(session, b"read-until-eof".to_vec());
        let output = tokio::time::timeout(Duration::from_millis(500), task.result)
            .await
            .expect("exec did not signal stdin EOF")
            .unwrap()
            .unwrap();
        assert_eq!(output.bytes, b"stdin-complete");
        assert_eq!(output.exit_status, Some(0));
    });
}

#[test]
fn oversized_paste_is_rejected_whole_and_queue_overload_is_visible() {
    exercise(Behavior::Backpressure, async |_, session, received| {
        session.input(vec![b'x'; MAX_INPUT_BYTES + 1]);
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(
            received.load(Ordering::SeqCst),
            0,
            "oversized command was partially transmitted"
        );
        assert_eq!(
            session.state.lock().unwrap().input_notice.as_deref(),
            Some("Terminal input not sent: paste exceeds 64 KiB")
        );
        for _ in 0..INPUT_QUEUE_CAPACITY + 8 {
            session.input(vec![b'x'; MAX_INPUT_BYTES]);
        }
        assert_eq!(
            session.state.lock().unwrap().input_notice.as_deref(),
            Some("Terminal input not sent: outbound queue is full or closed")
        );
        until(|| received.load(Ordering::SeqCst) >= 2048).await;
        assert_eq!(session.state.lock().unwrap().status, Status::Connected);
    });
}

#[test]
fn natural_shell_close_preserves_independent_work_and_channel_service() {
    exercise(
        Behavior::ShellCloseDuringExec,
        async |service, session, received| {
            let mut idle = session.open_channel().await.unwrap();
            idle.request_subsystem(true, "sftp").await.unwrap();
            assert_eq!(channel_data(&mut idle).await, b"subsystem-ready");
            session.input(vec![b'x'; MAX_INPUT_BYTES]);
            until(|| received.load(Ordering::SeqCst) >= 2048).await;
            let task = service.exec(session.clone(), b"long-running-independent-task".to_vec());
            let output = tokio::time::timeout(Duration::from_secs(1), task.result)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(
                output.bytes, b"independent-work-complete",
                "natural shell exit interrupted independent work"
            );
            assert_eq!(output.exit_status, Some(0));
            assert_eq!(
                session.state.lock().unwrap().status,
                Status::Connected,
                "shell completion must not end its shared transport"
            );
            assert_eq!(channel_data(&mut idle).await, b"subsystem-still-open");
            assert_eq!(
                session.state.lock().unwrap().terminal_status,
                TerminalStatus::Finished
            );
            assert!(!session.terminal_running());
            session.input(b"must-not-reach-closed-pty".to_vec());
            assert_eq!(
                session.state.lock().unwrap().input_notice.as_deref(),
                Some("Terminal input not sent: shell is finished or unavailable")
            );
            assert_eq!(received.load(Ordering::SeqCst), 2048);
            let mut fresh =
                tokio::time::timeout(Duration::from_millis(500), session.open_channel())
                    .await
                    .unwrap()
                    .unwrap();
            session.disconnect();
            until(|| session.state.lock().unwrap().status == Status::Disconnected).await;
            for channel in [&mut idle, &mut fresh] {
                assert!(matches!(
                    tokio::time::timeout(Duration::from_millis(750), channel.wait())
                        .await
                        .unwrap(),
                    None | Some(ChannelMsg::Close)
                ));
            }
        },
    );
}

async fn channel_data(channel: &mut Channel<client::Msg>) -> Vec<u8> {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            match channel.wait().await {
                Some(ChannelMsg::Data { data }) => return data.to_vec(),
                Some(ChannelMsg::Success) => {}
                other => panic!("subsystem unexpectedly completed: {other:?}"),
            }
        }
    })
    .await
    .expect("subsystem data stalled after terminal close")
}
