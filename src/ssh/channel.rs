//! Capacity is held for the channel/stream lifetime, including SFTP background tasks.
use super::*;
use crate::workspace::TransportPolicy;
use std::{
    ops::{Deref, DerefMut},
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
#[derive(Default)]
pub(super) struct Capacity {
    active: std::sync::atomic::AtomicUsize,
}
pub(super) struct Permit(Arc<Capacity>);
impl Capacity {
    pub fn acquire(self: &Arc<Self>, policy: TransportPolicy) -> Result<Permit, String> {
        if policy == TransportPolicy::SingleChannel {
            self.active.compare_exchange(0,1,Ordering::AcqRel,Ordering::Acquire).map_err(|_| "Single-channel connection is occupied. Close its channel or explicitly choose Independent connection.".to_string())?;
        } else {
            self.active.fetch_add(1, Ordering::AcqRel);
        }
        Ok(Permit(self.clone()))
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::AcqRel);
    }
}
pub struct ManagedChannel {
    channel: russh::Channel<client::Msg>,
    permit: Permit,
}
impl ManagedChannel {
    pub(super) fn new(channel: russh::Channel<client::Msg>, permit: Permit) -> Self {
        Self { channel, permit }
    }
    pub fn into_stream(self) -> ManagedStream {
        ManagedStream {
            stream: self.channel.into_stream(),
            _permit: self.permit,
        }
    }
}
impl Deref for ManagedChannel {
    type Target = russh::Channel<client::Msg>;
    fn deref(&self) -> &Self::Target {
        &self.channel
    }
}
impl DerefMut for ManagedChannel {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.channel
    }
}
pub struct ManagedStream {
    stream: russh::ChannelStream<client::Msg>,
    _permit: Permit,
}
impl AsyncRead for ManagedStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}
impl AsyncWrite for ManagedStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
