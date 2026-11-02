//! Development/test only. Bridges an encrypted russh subsystem to genuine OpenSSH sftp-server.
//! `-d` selects an initial working directory. This is NOT a chroot or full sshd fixture.
use russh::{
    Channel, ChannelId,
    server::{self, Session as ServerSession},
};
use std::{collections::HashMap, path::PathBuf, process::Stdio, sync::Arc};
#[derive(Clone)]
pub struct Fixture {
    pub root: PathBuf,
    pub channels: HashMap<ChannelId, Arc<tokio::sync::Mutex<Option<Channel<server::Msg>>>>>,
}
impl Fixture {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            channels: HashMap::new(),
        }
    }
}
impl server::Server for Fixture {
    type Handler = Self;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
        Self::new(self.root.clone())
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
    async fn channel_open_session(
        &mut self,
        ch: Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        self.channels
            .insert(ch.id(), Arc::new(tokio::sync::Mutex::new(Some(ch))));
        reply.accept().await;
        Ok(())
    }
    async fn subsystem_request(
        &mut self,
        id: ChannelId,
        name: &str,
        s: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        if name != "sftp" {
            s.channel_failure(id)?;
            return Ok(());
        }
        let Some(ch) = self.channels.remove(&id) else {
            s.channel_failure(id)?;
            return Ok(());
        };
        let child = tokio::process::Command::new("/usr/lib/openssh/sftp-server")
            .arg("-d")
            .arg(&self.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn();
        match child {
            Ok(mut child) => {
                s.channel_success(id)?;
                tokio::spawn(async move {
                    let channel = ch.lock().await.take().unwrap();
                    let stream = channel.into_stream();
                    let (mut reader, mut writer) = tokio::io::split(stream);
                    let mut input = child.stdin.take().unwrap();
                    let mut output = child.stdout.take().unwrap();
                    tokio::select! {_=tokio::io::copy(&mut reader,&mut input)=>{},_=tokio::io::copy(&mut output,&mut writer)=>{}}
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                });
            }
            Err(_) => s.channel_failure(id)?,
        }
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
        s.channel_failure(id)?;
        Ok(())
    }
    async fn shell_request(
        &mut self,
        id: ChannelId,
        s: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        s.channel_failure(id)?;
        Ok(())
    }
}
pub fn key() -> russh::keys::PrivateKey {
    russh::keys::PrivateKey::new(
        russh::keys::ssh_key::private::KeypairData::Ed25519(
            russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&[19; 32]),
        ),
        "public development fixture",
    )
    .unwrap()
}
