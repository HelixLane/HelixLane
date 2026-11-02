//! A transaction owns its capacity until the associated OS connection is closed.
use std::{io, sync::Arc};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
pub(super) async fn reserve(slots: &Arc<Semaphore>) -> io::Result<OwnedSemaphorePermit> {
    slots
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| io::Error::other("Transaction capacity unavailable"))
}
pub(super) fn release<S>(connection: S, permit: OwnedSemaphorePermit) {
    drop(connection);
    drop(permit);
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn held_capacity_keeps_extra_accepts_waiting_and_later_requests_alive() {
        let slots = Arc::new(Semaphore::new(super::super::QUEUE_CAPACITY));
        let mut held = vec![];
        for _ in 0..super::super::QUEUE_CAPACITY {
            held.push(reserve(&slots).await.unwrap());
        }
        let mut extra = vec![];
        for _ in 0..8 {
            let slots = slots.clone();
            extra.push(tokio::spawn(async move {
                let permit = reserve(&slots).await.unwrap();
                release((), permit);
            }));
        }
        tokio::task::yield_now().await;
        assert!(
            extra.iter().all(|task| !task.is_finished()),
            "saturation must wait rather than abandon an accept loop"
        );
        for permit in held {
            release((), permit);
        }
        for task in extra {
            task.await.unwrap();
        }
        let later = tokio::time::timeout(std::time::Duration::from_secs(1), reserve(&slots))
            .await
            .unwrap()
            .unwrap();
        release((), later);
        assert_eq!(slots.available_permits(), super::super::QUEUE_CAPACITY);
    }
    #[tokio::test]
    async fn connection_is_closed_before_capacity_is_released() {
        struct Connection {
            slots: Arc<Semaphore>,
            closed: Arc<std::sync::atomic::AtomicBool>,
        }
        impl Drop for Connection {
            fn drop(&mut self) {
                assert_eq!(self.slots.available_permits(), 0);
                self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let slots = Arc::new(Semaphore::new(1));
        let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let permit = reserve(&slots).await.unwrap();
        release(
            Connection {
                slots: slots.clone(),
                closed: closed.clone(),
            },
            permit,
        );
        assert!(closed.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(slots.available_permits(), 1);
    }
}
