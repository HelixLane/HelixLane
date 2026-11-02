//! One view's explicitly submitted file jobs. Collection generations and visibility
//! never cancel this scope; captured endpoints survive Follow changes and Continue.
use super::{Endpoint, Kind};
use std::future::Future;
use tokio::sync::watch;

pub const CANCELLED: &str =
    "Cancelled; requests already sent may complete / 已取消；已发送的请求可能仍会完成";

#[derive(Clone)]
pub struct JobScope(watch::Sender<bool>);
impl Default for JobScope {
    fn default() -> Self {
        Self(watch::channel(false).0)
    }
}
impl JobScope {
    pub fn cancel(&self) {
        self.0.send_replace(true);
    }
    pub fn check(&self) -> Result<(), String> {
        if *self.0.borrow() {
            Err(CANCELLED.into())
        } else {
            Ok(())
        }
    }
    pub async fn run<T>(&self, work: impl Future<Output = Result<T, String>>) -> Result<T, String> {
        let mut cancelled = self.0.subscribe();
        tokio::select! {
            biased;
            _ = cancelled.wait_for(|cancelled| *cancelled) => Err(CANCELLED.into()),
            result = work => result,
        }
    }
    /// Check between each committed mutation, as well as while a request awaits.
    pub async fn for_each<I, F, Fut>(&self, items: I, mut operation: F) -> Result<(), String>
    where
        I: IntoIterator,
        F: FnMut(I::Item) -> Fut,
        Fut: Future<Output = Result<(), String>>,
    {
        for item in items {
            self.check()?;
            self.run(operation(item)).await?;
        }
        Ok(())
    }
    /// Linearize a synchronous external launch with cancellation. Once cancel()
    /// returns, no later editor launch can pass this gate. Already opened editors
    /// and their registered leases are independent of preparation ownership.
    pub fn start<T>(&self, operation: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        let cancelled = self.0.borrow();
        if *cancelled {
            return Err(CANCELLED.into());
        }
        let result = operation();
        drop(cancelled);
        result
    }
}

pub fn delete_tree<'a>(
    endpoint: &'a Endpoint,
    path: &'a str,
    scope: &'a JobScope,
) -> std::pin::Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
    Box::pin(async move {
        scope.check()?;
        if scope
            .run(endpoint.stat(path))
            .await?
            .is_some_and(|e| e.kind == Kind::Directory)
        {
            let children = scope.run(endpoint.list(path)).await?;
            scope
                .for_each(children, |child| async move {
                    delete_tree(endpoint, &child.path, scope).await
                })
                .await?;
        }
        scope.check()?;
        scope.run(endpoint.remove(path)).await
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn scoped_slow_mutations_stop_subsequent_steps_but_other_views_and_continue_survive() {
        crate::tasks::runtime().block_on(async {
            let root =
                std::env::temp_dir().join(format!("helix-scoped-mutations-{}", std::process::id()));
            let first = root.join("first-committed");
            let next = root.join("next-must-survive");
            let peer = root.join("other-view");
            let captured = root.join("captured-before-follow-retarget");
            for path in [&first, &next, &peer, &captured] {
                std::fs::create_dir_all(path).unwrap();
                std::fs::write(path.join("nested-file"), b"content").unwrap();
            }
            let scope = JobScope::default();
            let running = scope.clone();
            let (entered, wait_entered) = tokio::sync::oneshot::channel();
            let gate = Arc::new(tokio::sync::Notify::new());
            let work_gate = gate.clone();
            let selected = [first.clone(), next.clone()];
            let work = tokio::spawn(async move {
                let mut entered = Some(entered);
                running
                    .for_each(selected, |path| {
                        let scope = running.clone();
                        let entered = entered.take();
                        let gate = work_gate.clone();
                        async move {
                            delete_tree(&Endpoint::Local, &path.display().to_string(), &scope)
                                .await?;
                            if let Some(entered) = entered {
                                let _ = entered.send(());
                                // A slow request boundary after one committed tree.
                                gate.notified().await;
                            }
                            Ok(())
                        }
                    })
                    .await
            });
            wait_entered.await.unwrap();
            scope.cancel();
            assert_eq!(work.await.unwrap().unwrap_err(), CANCELLED);
            assert!(!first.exists(), "committed changes are not undone");
            assert!(
                next.join("nested-file").exists(),
                "subsequent recursive delete escaped cancellation"
            );
            let other_view = JobScope::default();
            delete_tree(&Endpoint::Local, &peer.display().to_string(), &other_view)
                .await
                .unwrap();
            assert!(!peer.exists());
            // Continue drops presentation ownership without setting cancellation;
            // a captured target is unaffected by later ambient resource changes.
            let presentation = JobScope::default();
            let submitted = presentation.clone();
            drop(presentation);
            let mut ambient_target = captured.clone();
            let target = ambient_target.clone();
            ambient_target = next.clone();
            delete_tree(&Endpoint::Local, &target.display().to_string(), &submitted)
                .await
                .unwrap();
            assert!(!captured.exists());
            assert!(ambient_target.exists());
            std::fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    fn cancelled_editor_gate_cannot_launch_but_other_view_can() {
        let scope = JobScope::default();
        scope.cancel();
        let opened = std::cell::Cell::new(false);
        assert_eq!(
            scope
                .start(|| {
                    opened.set(true);
                    Ok(())
                })
                .unwrap_err(),
            CANCELLED
        );
        assert!(!opened.get());
        JobScope::default()
            .start(|| {
                opened.set(true);
                Ok(())
            })
            .unwrap();
        assert!(opened.get());
    }
}
