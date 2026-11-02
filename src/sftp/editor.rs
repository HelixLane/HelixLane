//! An edit lease captures its remote session and original full-file digest.
use super::*;
// A cancelled preparation future must not leave a partial private download.
// Completed/opened leases retain the existing independent EditorManager lifetime.
struct PendingDownload {
    path: std::path::PathBuf,
    keep: bool,
}
impl Drop for PendingDownload {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
async fn copy_to_temporary(
    input: impl tokio::io::AsyncRead + Unpin,
    local: &std::path::Path,
) -> Result<(u64, PendingDownload), String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    // Create and arm cleanup without an await gap. create_new never replaces
    // another job's file, and the guard owns only a successfully created path.
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(local).map_err(err)?;
    let pending = PendingDownload {
        path: local.into(),
        keep: false,
    };
    let mut output = tokio::fs::File::from_std(file);
    let mut input = input.take(32 * 1024 * 1024 + 1);
    let bytes = tokio::io::copy(&mut input, &mut output)
        .await
        .map_err(err)?;
    output.shutdown().await.map_err(err)?;
    drop(output);
    Ok((bytes, pending))
}

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
        // Pin the validated directory's canonical identity once. Temporary roots
        // may have aliased ancestors (e.g. macOS /var -> /private/var), while
        // TransferSpec captures canonical source paths. All local fingerprints
        // must use that same identity, including the immutable upload snapshot.
        let root = std::fs::canonicalize(root).map_err(err)?;
        // Local Windows paths may be drive/UNC/verbatim paths; remote paths are
        // POSIX. Only the endpoint's leaf belongs in the private copy's name.
        // POSIX leaves may also contain characters forbidden on Windows. Keep
        // the copy portable and bounded; its name never changes the target.
        let name: String = endpoint
            .basename(&remote)?
            .chars()
            .take(48)
            .map(|c| {
                if c.is_control()
                    || matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
                {
                    '_'
                } else {
                    c
                }
            })
            .collect();
        let local = root.join(format!(
            "edit-{}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            name.trim_end_matches(['.', ' '])
        ));
        let input = endpoint.reader(&remote).await?;
        let (bytes, mut pending) = copy_to_temporary(input, &local).await?;
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
        pending.keep = true;
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
    #[cfg(unix)]
    #[tokio::test]
    async fn private_editor_names_are_portable_without_changing_source_identity() {
        let root =
            std::env::temp_dir().join(format!("helixlane-edit-portable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        // These are legal POSIX source names, including a literal Windows path.
        // The private copy must still be one portable filename on every host.
        for name in [
            r"\\?\C:\Users\me\remote.txt",
            "report:final?.txt",
            "trailing. ",
        ] {
            let original = root.join(name);
            std::fs::write(&original, b"source bytes").unwrap();
            let lease = EditLease::download(
                Endpoint::Local,
                original.display().to_string(),
                &root.join("edits"),
            )
            .await
            .unwrap();
            let local_name = lease.local.file_name().unwrap().to_str().unwrap();
            assert!(
                validate_child_name(local_name, true).is_ok(),
                "private editor filename is not portable: {local_name}"
            );
            assert_eq!(
                lease.local.parent().unwrap(),
                root.join("edits").canonicalize().unwrap()
            );
            assert_eq!(
                lease.remote,
                original.canonicalize().unwrap().display().to_string()
            );
            assert_eq!(std::fs::read(&lease.local).unwrap(), b"source bytes");
            assert!(!lease.saved().await.unwrap());
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn aliased_temporary_parent_keeps_two_confirmed_saves_clean() {
        let root =
            std::env::temp_dir().join(format!("helixlane-edit-alias-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let actual = root.join("actual");
        std::fs::create_dir_all(&actual).unwrap();
        let alias = root.join("alias");
        std::os::unix::fs::symlink(&actual, &alias).unwrap();
        let original = actual.join("remote.txt");
        std::fs::write(&original, b"original").unwrap();
        let lease = Arc::new(
            EditLease::download(
                Endpoint::Local,
                original.display().to_string(),
                &alias.join("edits"),
            )
            .await
            .unwrap(),
        );
        assert!(!lease.saved().await.unwrap());
        let manager = crate::tasks::TransferManager::new(1);
        for content in [
            b"save through alias".as_slice(),
            b"second atomic save".as_slice(),
        ] {
            // Editors commonly save by replacing the regular file atomically.
            let replacement = lease.local.with_extension("replacement");
            std::fs::write(&replacement, content).unwrap();
            std::fs::rename(&replacement, &lease.local).unwrap();
            assert!(lease.saved().await.unwrap());
            lease.upload_confirmed(manager.clone()).await.unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while lease.uploading.load(std::sync::atomic::Ordering::Acquire) {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(*lease.last_error.lock().unwrap(), None);
            assert_eq!(std::fs::read(&original).unwrap(), content);
            assert!(!lease.saved().await.unwrap());
        }
        assert_eq!(manager.jobs().len(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }

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

#[cfg(test)]
mod preparation_tests {
    use super::*;
    use crate::sftp::jobs::{CANCELLED, JobScope};
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::io::AsyncWriteExt;

    #[test]
    fn cancelled_slow_editor_preparation_removes_partial_file_and_never_opens_editor() {
        crate::tasks::runtime().block_on(async {
            let local =
                std::env::temp_dir().join(format!("helix-editor-cancel-{}", std::process::id()));
            let _ = std::fs::remove_file(&local);
            let (mut source, reader) = tokio::io::duplex(32);
            let scope = JobScope::default();
            let running = scope.clone();
            let destination = local.clone();
            let opened = Arc::new(AtomicBool::new(false));
            let launched = opened.clone();
            let job = tokio::spawn(async move {
                let (_, mut pending) = running.run(copy_to_temporary(reader, &destination)).await?;
                running.start(|| {
                    launched.store(true, Ordering::SeqCst);
                    pending.keep = true;
                    Ok::<_, String>(())
                })
            });
            source.write_all(b"partial download").await.unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                loop {
                    if std::fs::metadata(&local).is_ok_and(|m| m.len() > 0) {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            // The source deliberately remains open: completion cannot win the race.
            scope.cancel();
            assert_eq!(job.await.unwrap().unwrap_err(), CANCELLED);
            assert!(!opened.load(Ordering::SeqCst));
            assert!(
                !local.exists(),
                "cancelled editor download left a partial file"
            );
        });
    }
}
