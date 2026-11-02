//! Development-only encrypted SSH fixture. No OS commands are executed.
//! `cargo run --example ssh_fixture --no-default-features`; user fixture, auth None or password correct.
use russh::{
    Channel, ChannelId,
    server::{self, Server as _, Session as ServerSession},
};
use std::{io::Write, sync::Arc};
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
    async fn auth_none(&mut self, user: &str) -> Result<server::Auth, Self::Error> {
        Ok(if user == "fixture" {
            server::Auth::Accept
        } else {
            server::Auth::reject()
        })
    }
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

#[tokio::main]
async fn main() {
    // Deliberately public deterministic fixture material, never a real host key.
    let key = russh::keys::PrivateKey::new(
        russh::keys::ssh_key::private::KeypairData::Ed25519(
            russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&[7; 32]),
        ),
        "synthetic fixture",
    )
    .unwrap();
    let public_key = key.public_key().to_openssh().unwrap();
    let config = Arc::new(server::Config {
        keys: vec![key],
        auth_rejection_time: std::time::Duration::ZERO,
        ..Default::default()
    });
    let requested = std::env::var("HELIXLANE_FIXTURE_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(0);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", requested))
        .await
        .expect("loopback listener");
    println!(
        "HELIXLANE_FIXTURE_PORT={}",
        listener.local_addr().unwrap().port()
    );
    println!("HELIXLANE_FIXTURE_PUBLIC_KEY={public_key}");
    std::io::stdout().flush().unwrap();
    Fixture { shell: true }
        .run_on_socket(config, &listener)
        .await
        .expect("SSH fixture");
}
