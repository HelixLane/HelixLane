//! An edit lease captures its remote session and original full-file digest.
use super::*;
pub struct EditLease {
    pub endpoint: Endpoint,
    pub remote: String,
    pub local: std::path::PathBuf,
    baseline: std::sync::Mutex<Option<Fingerprint>>,
    local_digest: std::sync::Mutex<Option<Fingerprint>>,
    pub dirty: std::sync::atomic::AtomicBool,
    pub uploading: std::sync::atomic::AtomicBool,
    pub last_error: std::sync::Mutex<Option<String>>,
}
impl EditLease {
    pub async fn download(
        endpoint: Endpoint,
        remote: String,
        root: &std::path::Path,
    ) -> Result<Self, String> {
        let remote = endpoint.absolute(&remote).await?;
        let baseline = Fingerprint::capture(&endpoint, &remote).await?;
        if baseline.as_ref().is_none_or(|f| f.entry.kind != Kind::File) {
            return Err("Only regular files can be edited".into());
        }
        match std::fs::symlink_metadata(root) {
            Ok(metadata) if metadata.is_symlink() || !metadata.is_dir() => {
                return Err("Unsafe editor temporary directory".into());
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(root).map_err(err)?
            }
            Err(e) => return Err(err(e)),
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).map_err(err)?;
        }
        let local = root.join(format!(
            "edit-{}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            remote.rsplit('/').next().unwrap_or("file")
        ));
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut input = endpoint.reader(&remote).await?.take(32 * 1024 * 1024 + 1);
        let mut output = Endpoint::Local
            .create_new(&local.display().to_string())
            .await?;
        let bytes = tokio::io::copy(&mut input, &mut output)
            .await
            .map_err(err)?;
        output.shutdown().await.map_err(err)?;
        drop(output);
        if bytes > 32 * 1024 * 1024 || Fingerprint::capture(&endpoint, &remote).await? != baseline {
            let _ = tokio::fs::remove_file(&local).await;
            return Err(if bytes > 32 * 1024 * 1024 {
                "External edit limit is 32 MiB"
            } else {
                "Remote file changed during download"
            }
            .into());
        }

        let local_digest =
            Fingerprint::capture(&Endpoint::Local, &local.display().to_string()).await?;
        Ok(Self {
            endpoint,
            remote,
            local,
            baseline: std::sync::Mutex::new(baseline),
            local_digest: std::sync::Mutex::new(local_digest),
            dirty: std::sync::atomic::AtomicBool::new(false),
            uploading: std::sync::atomic::AtomicBool::new(false),
            last_error: std::sync::Mutex::new(None),
        })
    }
    pub async fn saved(&self) -> Result<bool, String> {
        let baseline = self.local_digest.lock().unwrap().clone();
        Ok(
            Fingerprint::capture(&Endpoint::Local, &self.local.display().to_string()).await?
                != baseline,
        )
    }
    pub async fn upload_spec(&self) -> Result<crate::tasks::TransferSpec, String> {
        let baseline = self.baseline.lock().unwrap().clone();
        let spec = crate::tasks::TransferSpec::capture(
            Endpoint::Local,
            self.local.display().to_string(),
            self.endpoint.clone(),
            self.remote.clone(),
            crate::tasks::Conflict::Overwrite,
        )
        .await?;
        if spec.target_state != baseline {
            return Err("Remote file changed; download again or resolve conflict".into());
        }
        Ok(spec)
    }
    pub async fn upload_confirmed(
        self: &Arc<Self>,
        manager: Arc<crate::tasks::TransferManager>,
    ) -> Result<u64, String> {
        use std::sync::atomic::Ordering;
        if self.uploading.swap(true, Ordering::AcqRel) {
            return Err("An editor upload is already running".into());
        }
        let spec = match self.upload_spec().await {
            Ok(spec) => spec,
            Err(e) => {
                self.uploading.store(false, Ordering::Release);
                return Err(e);
            }
        };
        let source_state = spec.source_state.clone();
        let id = manager.submit(spec);
        let lease = self.clone();
        crate::tasks::runtime().spawn(async move {
            loop {
                let job = manager.jobs().into_iter().find(|j| j.id == id);
                match job.map(|j| j.state) {
                    Some(crate::tasks::JobState::Complete) => {
                        match Fingerprint::capture(&lease.endpoint, &lease.remote).await {
                            Ok(current)
                                if current.as_ref().and_then(|f| f.digest.clone())
                                    == source_state.as_ref().and_then(|f| f.digest.clone()) =>
                            {
                                *lease.baseline.lock().unwrap() = current;
                                *lease.local_digest.lock().unwrap() = source_state;
                                lease.dirty.store(false, Ordering::Release);
                                *lease.last_error.lock().unwrap() = None;
                            }
                            _ => {
                                *lease.last_error.lock().unwrap() =
                                    Some("Remote file changed after upload".into())
                            }
                        }
                        break;
                    }
                    Some(crate::tasks::JobState::Failed(e)) => {
                        *lease.last_error.lock().unwrap() = Some(e);
                        break;
                    }
                    Some(crate::tasks::JobState::Cancelled | crate::tasks::JobState::Skipped)
                    | None => break,
                    _ => {
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                }
            }
            lease.uploading.store(false, Ordering::Release);
        });
        Ok(id)
    }
    pub fn open(&self, program: &str) -> Result<(), String> {
        if program.trim().is_empty() {
            return Err("Choose an external editor executable".into());
        }
        let mut child = std::process::Command::new(program)
            .arg(&self.local)
            .spawn()
            .map_err(err)?;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}

/// Leases outlive the browser that opened them and always retain their original endpoint.
pub struct EditorManager {
    leases: std::sync::Mutex<Vec<Arc<EditLease>>>,
}
impl EditorManager {
    pub fn global() -> &'static Self {
        static MANAGER: std::sync::OnceLock<EditorManager> = std::sync::OnceLock::new();
        MANAGER.get_or_init(|| Self {
            leases: std::sync::Mutex::new(vec![]),
        })
    }
    pub fn register(&self, lease: Arc<EditLease>) {
        self.leases.lock().unwrap().push(lease.clone());
        crate::tasks::runtime().spawn(async move {
            loop {
                match lease.saved().await {
                    Ok(changed) => lease
                        .dirty
                        .store(changed, std::sync::atomic::Ordering::Release),
                    Err(e) => *lease.last_error.lock().unwrap() = Some(e),
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        });
    }

    pub fn leases(&self) -> Vec<Arc<EditLease>> {
        self.leases.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn confirmed_edit_can_upload_two_saves_without_retargeting() {
        let root =
            std::env::temp_dir().join(format!("helixlane-edit-repeat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let original = root.join("remote.txt");
        std::fs::write(&original, b"original").unwrap();
        let lease = Arc::new(
            EditLease::download(
                Endpoint::Local,
                original.display().to_string(),
                &root.join("edits"),
            )
            .await
            .unwrap(),
        );
        let manager = crate::tasks::TransferManager::new(1);
        for content in [b"save one".as_slice(), b"save two".as_slice()] {
            std::fs::write(&lease.local, content).unwrap();
            assert!(lease.saved().await.unwrap());
            lease.upload_confirmed(manager.clone()).await.unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while lease.uploading.load(std::sync::atomic::Ordering::Acquire) {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(std::fs::read(&original).unwrap(), content);
            assert!(!lease.saved().await.unwrap());
        }
        assert_eq!(manager.jobs().len(), 2);
    }
}
