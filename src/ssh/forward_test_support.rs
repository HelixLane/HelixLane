//! Scheduling controls for tests of the real forwarding future.
use super::*;
use std::{future::Future, task::Poll};

/// Pause after registration, before the test sends a real SSH channel open.
/// Only this future stops being polled; transport I/O and timers keep running,
/// including on a current-thread runtime. No production lock is held as a gate.
pub(in crate::ssh) fn pause_after_listening(
    service: &Service,
    session: Arc<Session>,
    spec: ForwardSpec,
) -> (Arc<ForwardJob>, oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (stop, stopped) = watch::channel(false);
    let job = Arc::new(ForwardJob {
        id: service.next.fetch_add(1, Ordering::Relaxed),
        profile: session.profile.clone(),
        spec,
        status: Mutex::new(ForwardStatus::Starting),
        stop,
    });
    session.forwards.lock().unwrap().insert(job.id, job.clone());
    let (paused, reached) = oneshot::channel();
    let (resume, resumed) = oneshot::channel();
    let running = job.clone();
    service.runtime.spawn(async move {
        let mut forward = std::pin::pin!(run_forward(&session, &running, stopped));
        std::future::poll_fn(|cx| {
            if let Poll::Ready(result) = forward.as_mut().poll(cx) {
                panic!("forward ended before its listening gate: {result:?}");
            }
            if matches!(*running.status.lock().unwrap(), ForwardStatus::Listening(_)) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
        paused.send(()).expect("test dropped listening event");
        resumed.await.expect("test dropped forward resume gate");
        let result = forward.await;
        *running.status.lock().unwrap() = match result {
            Ok(()) => ForwardStatus::Stopped,
            Err(error) => ForwardStatus::Failed(error),
        };
        session.state.lock().unwrap().revision += 1;
    });
    (job, reached, resume)
}
