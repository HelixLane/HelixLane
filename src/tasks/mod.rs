//! View-independent transfer jobs. All endpoints and snapshots are captured at submission.
use crate::sftp::{Endpoint, Entry, Fingerprint, Kind, join};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub enum Conflict {
    Overwrite,
    Skip,
    Rename,
}
#[derive(Clone)]
pub struct TransferSpec {
    pub source: Endpoint,
    pub source_path: String,
    pub target: Endpoint,
    pub target_path: String,
    pub source_state: Option<Fingerprint>,
    pub target_state: Option<Fingerprint>,
    pub conflict: Conflict,
    pub source_tree: BTreeMap<String, Fingerprint>,
    pub target_tree: BTreeMap<String, Fingerprint>,
}
impl TransferSpec {
    pub async fn into_directory(
        source: Endpoint,
        source_path: String,
        target: Endpoint,
        target_directory: &str,
        conflict: Conflict,
    ) -> Result<Self, String> {
        let target_path = source.destination_path(&source_path, &target, target_directory)?;
        Self::capture(source, source_path, target, target_path, conflict).await
    }

    pub async fn capture(
        source: Endpoint,
        source_path: String,
        target: Endpoint,
        mut target_path: String,
        conflict: Conflict,
    ) -> Result<Self, String> {
        let source_path = source.absolute(&source_path).await?;
        target_path = target.absolute(&target_path).await?;
        let source_state = Fingerprint::capture(&source, &source_path).await?;
        if source_state.is_none() {
            return Err("Source missing".into());
        }
        if source.label() == target.label() && source.path_within(&target_path, &source_path) {
            return Err("Target cannot be source or a child of source".into());
        }
        if matches!(conflict, Conflict::Rename) {
            let original = target_path.clone();
            let mut n = 1;
            while target.stat(&target_path).await?.is_some() {
                target_path = format!("{original} ({n})");
                n += 1;
                if n > 10000 {
                    return Err("No available target name".into());
                }
            }
        }
        let target_state = Fingerprint::capture(&target, &target_path).await?;
        if matches!(conflict, Conflict::Overwrite) {
            ensure_target_kind(
                &source_state.as_ref().unwrap().entry.kind,
                target_state.as_ref().map(|f| &f.entry),
                &target_path,
            )?;
        }
        let source_tree = snapshot(&source, &source_path).await?;
        let target_tree = snapshot(&target, &target_path).await?;
        Ok(Self {
            source_tree,
            target_tree,
            source,
            source_path,
            target,
            target_path,
            source_state,
            target_state,
            conflict,
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum JobState {
    Queued,
    Running,
    Complete,
    Skipped,
    Cancelled,
    Failed(String),
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Job {
    pub id: u64,
    pub source: String,
    pub target: String,
    pub staging: Option<String>,
    pub bytes: u64,
    pub total: u64,
    #[serde(default)]
    pub staging_bytes: u64,
    pub skipped_links: u64,
    pub attempts: u32,
    pub state: JobState,
}
struct Work {
    recovery_id: String,
    spec: TransferSpec,
    job: Job,
    cancel: Arc<AtomicBool>,
}
pub struct TransferManager {
    jobs: Mutex<BTreeMap<u64, Work>>,
    token: String,
    next: AtomicU64,
    limit: tokio::sync::Semaphore,
    journal: Option<std::path::PathBuf>,
    recovered: Mutex<Vec<Recovery>>,
    persistence_error: Mutex<Option<String>>,
    journal_write: Mutex<()>,
}
impl TransferManager {
    pub fn new(concurrency: usize) -> Arc<Self> {
        static TOKENS: AtomicU64 = AtomicU64::new(1);
        let token = format!(
            "{}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            TOKENS.fetch_add(1, Ordering::Relaxed)
        );
        Arc::new(Self {
            token,
            jobs: Mutex::new(BTreeMap::new()),
            journal: None,
            recovered: Mutex::new(vec![]),
            persistence_error: Mutex::new(None),
            journal_write: Mutex::new(()),
            next: AtomicU64::new(1),
            limit: tokio::sync::Semaphore::new(concurrency.clamp(1, 16)),
        })
    }
    pub fn global() -> &'static Arc<Self> {
        static M: OnceLock<Arc<TransferManager>> = OnceLock::new();
        M.get_or_init(|| {
            let path = crate::settings::Settings::path().with_file_name("transfer-journal.json");
            Self::with_journal(2, path).unwrap_or_else(|e| {
                let m = Self::new(2);
                *m.persistence_error.lock().unwrap() = Some(e);
                m
            })
        })
    }
    pub fn jobs(&self) -> Vec<Job> {
        self.jobs
            .lock()
            .unwrap()
            .values()
            .map(|w| w.job.clone())
            .collect()
    }
    fn temporary_path(&self, id: u64, to: &str) -> String {
        format!(
            "{to}.helixlane-{}-{}-{id}.part",
            std::process::id(),
            self.token
        )
    }
    pub fn submit_paused(&self, spec: TransferSpec) -> u64 {
        let id = self.enqueue(spec, None);
        self.persist();
        id
    }
    fn enqueue(&self, spec: TransferSpec, recovery_id: Option<String>) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let remote_cross = matches!((&spec.source,&spec.target),(Endpoint::Remote{transport:a,..},Endpoint::Remote{transport:b,..}) if a.id!=b.id);
        let job = Job {
            id,
            source: format!("{}:{}", spec.source.label(), spec.source_path),
            target: format!("{}:{}", spec.target.label(), spec.target_path),
            staging: remote_cross.then(|| {
                std::env::temp_dir()
                    .join(format!(
                        "helixlane-transfer-{}-{}-{id}",
                        std::process::id(),
                        self.token
                    ))
                    .display()
                    .to_string()
            }),
            bytes: 0,
            total: spec
                .source_tree
                .values()
                .filter(|f| f.entry.kind == Kind::File)
                .map(|f| f.entry.size)
                .sum(),
            staging_bytes: 0,
            skipped_links: 0,
            attempts: 0,
            state: JobState::Queued,
        };
        self.jobs.lock().unwrap().insert(
            id,
            Work {
                recovery_id: recovery_id.unwrap_or_else(|| format!("{}-{id}", self.token)),
                spec,
                job,
                cancel: Arc::new(AtomicBool::new(false)),
            },
        );
        id
    }
    pub fn submit(self: &Arc<Self>, spec: TransferSpec) -> u64 {
        let id = self.submit_paused(spec);
        self.start(id);
        id
    }
    pub fn start(self: &Arc<Self>, id: u64) {
        let manager = self.clone();
        runtime().spawn(async move {
            manager.run(id).await;
        });
    }
    pub fn cancel(&self, id: u64) {
        if let Some(w) = self.jobs.lock().unwrap().get_mut(&id) {
            w.cancel.store(true, Ordering::Release);
            if w.job.state == JobState::Queued {
                w.job.state = JobState::Cancelled;
            }
        }
    }
    pub fn retry(self: &Arc<Self>, id: u64) -> Result<(), String> {
        let mut jobs = self.jobs.lock().unwrap();
        let w = jobs.get_mut(&id).ok_or("Task missing")?;
        if !matches!(w.job.state, JobState::Failed(_) | JobState::Cancelled) {
            return Err("Task is not retryable".into());
        }
        w.cancel.store(false, Ordering::Release);
        w.job.state = JobState::Queued;
        w.job.bytes = 0;
        drop(jobs);
        self.persist();
        self.start(id);
        Ok(())
    }
    fn update(&self, id: u64, f: impl FnOnce(&mut Job)) {
        if let Some(w) = self.jobs.lock().unwrap().get_mut(&id) {
            f(&mut w.job)
        }
    }
    pub async fn run(&self, id: u64) {
        let _permit = self.limit.acquire().await.unwrap();
        let (spec, cancel, staging) = {
            let mut jobs = self.jobs.lock().unwrap();
            let Some(w) = jobs.get_mut(&id) else { return };
            if w.job.state == JobState::Cancelled {
                return;
            }
            if w.job.state != JobState::Queued {
                return;
            }
            w.job.state = JobState::Running;
            w.job.attempts += 1;
            (w.spec.clone(), w.cancel.clone(), w.job.staging.clone())
        };
        self.persist();
        let result: Result<JobState, String> = async {
            check(&cancel)?;
            if snapshot(&spec.source, &spec.source_path).await? != spec.source_tree {
                return Err("Source changed; recapture transfer".into());
            }
            if snapshot(&spec.target, &spec.target_path).await? != spec.target_tree {
                return Err("Target changed; resolve conflict".into());
            }
            if Fingerprint::capture(&spec.source, &spec.source_path).await? != spec.source_state {
                return Err("Source changed; recapture transfer".into());
            }
            if Fingerprint::capture(&spec.target, &spec.target_path).await? != spec.target_state {
                return Err("Target changed; resolve conflict".into());
            }
            if spec.target_state.is_some() && matches!(spec.conflict, Conflict::Skip) {
                return Ok(JobState::Skipped);
            }
            if let Some(path) = staging {
                if std::path::Path::new(&path).exists() {
                    return Err("Local staging path already exists".into());
                }
                std::fs::create_dir(&path).map_err(|e| e.to_string())?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                        .map_err(|e| e.to_string())?;
                }
                let local = join(&path, "payload");
                let first = self
                    .copy_tree(
                        id,
                        &spec.source,
                        &spec.source_path,
                        &Endpoint::Local,
                        &local,
                        &cancel,
                    )
                    .await;
                if first.is_ok() {
                    self.update(id, |j| {
                        j.staging_bytes = j.bytes;
                        j.bytes = 0;
                    });
                }
                let result = match first {
                    Ok(()) => {
                        self.copy_tree(
                            id,
                            &Endpoint::Local,
                            &local,
                            &spec.target,
                            &spec.target_path,
                            &cancel,
                        )
                        .await
                    }
                    Err(e) => Err(e),
                };
                let _ = tokio::fs::remove_dir_all(&path).await;
                self.update(id, |j| j.staging_bytes = 0);
                result?;
            } else {
                self.copy_tree(
                    id,
                    &spec.source,
                    &spec.source_path,
                    &spec.target,
                    &spec.target_path,
                    &cancel,
                )
                .await?;
            }
            Ok(JobState::Complete)
        }
        .await;
        self.update(id, |j| {
            j.state = match result {
                Ok(s) => s,
                // A retained backup needs an actionable diagnostic even when
                // cancellation arrived after the commit transaction began.
                Err(e) if e.starts_with("Commit failed:") || e.starts_with("Upload committed;") => {
                    JobState::Failed(e)
                }
                Err(_) if cancel.load(Ordering::Acquire) => JobState::Cancelled,
                Err(e) => JobState::Failed(e),
            }
        });
        self.persist();
    }
    fn copy_tree<'a>(
        &'a self,
        id: u64,
        source: &'a Endpoint,
        from: &'a str,
        target: &'a Endpoint,
        to: &'a str,
        cancel: &'a AtomicBool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            check(cancel)?;
            let expectations = {
                let jobs = self.jobs.lock().unwrap();
                jobs.get(&id).map(|w| {
                    (
                        (source.label() == w.spec.source.label()
                            && source.path_within(from, &w.spec.source_path))
                        .then(|| w.spec.source_tree.get(from).cloned()),
                        (target.label() == w.spec.target.label()
                            && target.path_within(to, &w.spec.target_path))
                        .then(|| w.spec.target_tree.get(to).cloned()),
                    )
                })
            };
            if let Some((expected_source, expected_target)) = expectations {
                if let Some(expected) = expected_source
                    && Fingerprint::capture(source, from).await? != expected
                {
                    return Err("Source changed during transfer".into());
                }
                if let Some(expected) = expected_target
                    && Fingerprint::capture(target, to).await? != expected
                {
                    return Err("Target changed during transfer".into());
                }
            }
            let entry = source.stat(from).await?.ok_or("Source missing")?;
            match entry.kind {
                Kind::Symlink | Kind::Other => {
                    self.update(id, |j| j.skipped_links += 1);
                    Ok(())
                }
                Kind::Directory => {
                    if let Some(existing) = target.stat(to).await? {
                        if existing.kind != Kind::Directory {
                            return Err("Directory conflict".into());
                        }
                    } else {
                        target.mkdir(to).await?;
                    }
                    for e in source.list(from).await? {
                        crate::sftp::validate_child_name(
                            &e.name,
                            matches!(target, Endpoint::Local) && cfg!(windows),
                        )?;
                        self.copy_tree(
                            id,
                            source,
                            &e.path,
                            target,
                            &target.child_path(to, &e.name),
                            cancel,
                        )
                        .await?;
                    }
                    Ok(())
                }
                Kind::File => {
                    let before = Fingerprint::capture(source, from).await?;
                    let target_before = Fingerprint::capture(target, to).await?;
                    ensure_target_kind(&entry.kind, target_before.as_ref().map(|f| &f.entry), to)?;
                    let temp = self.temporary_path(id, to);
                    let mut owned_temp = false;
                    let result = async {
                        let mut input = source.reader(from).await?;
                        let mut output = target.create_new(&temp).await?;
                        owned_temp = true;
                        let mut buf = vec![0; 65536];
                        loop {
                            check(cancel)?;
                            let n = input.read(&mut buf).await.map_err(|e| e.to_string())?;
                            if n == 0 {
                                break;
                            }
                            output
                                .write_all(&buf[..n])
                                .await
                                .map_err(|e| e.to_string())?;
                            self.update(id, |j| {j.bytes += n as u64;if j.staging.as_ref().is_some_and(|path|to.starts_with(&format!("{path}/"))){j.staging_bytes+=n as u64;}});
                        }
                        output.shutdown().await.map_err(|e| e.to_string())?;
                        drop(output);
                        check(cancel)?;
                        if Fingerprint::capture(source, from).await? != before {
                            return Err("Source changed during transfer".into());
                        }
                        check(cancel)?;
                        let validated_target=Fingerprint::capture(target,to).await?;
                        ensure_target_kind(&entry.kind,validated_target.as_ref().map(|f|&f.entry),to)?;
                        if validated_target!=target_before{return Err("Target changed during transfer".into())}
                        // Validation may await a full-file digest. Cancellation
                        // accepted during that read must stop before commit.
                        check(cancel)?;
                        match target {
                            Endpoint::Local => {
                                validate_commit_target(target,to,&entry.kind,target_before.as_ref(),cancel).await?;
                                target.rename(&temp,to).await?;
                            },
                            Endpoint::Remote { .. } => {
                                if target_before.is_some() {
                                    let backup = format!("{temp}.previous");
                                    if target.stat(&backup).await?.is_some() {
                                        return Err("Backup path exists".into());
                                    }
                                    // The backup lookup also awaits the server.
                                    // After this barrier commit/rollback is one
                                    // coherent transaction, even if cancelled.
                                    validate_commit_target(target,to,&entry.kind,target_before.as_ref(),cancel).await?;
                                    target.rename(to, &backup).await?;
                                    if let Err(e) = target.rename(&temp, to).await {
                                        if let Err(rollback)=target.rename(&backup,to).await{return Err(format!("Commit failed: {e}; original retained at {backup}; rollback failed: {rollback}"))}
                                        return Err(e);
                                    }
                                    target.remove(&backup).await.map_err(|e|format!("Upload committed; previous target retained at {backup}; cleanup failed: {e}"))?;
                                } else {
                                    validate_commit_target(target,to,&entry.kind,None,cancel).await?;
                                    target.rename(&temp, to).await?;
                                }
                            }
                        }
                        Ok(())
                    }
                    .await;
                    if result.is_err() && owned_temp {
                        let _ = target.remove(&temp).await;
                    }
                    result
                }
            }
        })
    }
}
fn ensure_target_kind(source: &Kind, target: Option<&Entry>, path: &str) -> Result<(), String> {
    if matches!(source, Kind::File | Kind::Directory)
        && target.is_some_and(|target| &target.kind != source)
    {
        return Err(format!(
            "Incompatible destination type at {path}; original target left unchanged"
        ));
    }
    Ok(())
}
async fn validate_commit_target(
    target: &Endpoint,
    path: &str,
    source_kind: &Kind,
    expected: Option<&Fingerprint>,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let current = target.stat(path).await?;
    ensure_target_kind(source_kind, current.as_ref(), path)?;
    if current.as_ref() != expected.map(|f| &f.entry) {
        return Err("Target changed before commit".into());
    }
    // Last cancellation barrier before the first mutating rename. After that
    // rename starts, the caller completes commit or rollback coherently.
    check(cancel)
}
fn check(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err("Cancelled".into())
    } else {
        Ok(())
    }
}
#[cfg(test)]
mod tests;

pub fn runtime() -> &'static tokio::runtime::Runtime {
    static R: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    R.get_or_init(|| tokio::runtime::Runtime::new().unwrap())
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum SavedEndpoint {
    Local,
    Remote(crate::workspace::ConnectionProfile),
}
impl SavedEndpoint {
    fn capture(e: &Endpoint) -> Self {
        match e {
            Endpoint::Local => Self::Local,
            Endpoint::Remote { transport, .. } => Self::Remote(transport.profile.clone()),
        }
    }
    async fn resolve(&self) -> Result<Endpoint, String> {
        match self {
            Self::Local => Ok(Endpoint::Local),
            Self::Remote(p) => {
                let session=crate::ssh::Service::global().resumable(p).into_iter().last().ok_or("Reconnect and verify the captured connection profile before recovering this task")?;
                Endpoint::connect(session).await
            }
        }
    }
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Recovery {
    #[serde(default)]
    pub id: String,
    #[serde(skip)]
    pub claimed: bool,
    pub job: Job,
    pub source: SavedEndpoint,
    pub source_path: String,
    pub target: SavedEndpoint,
    pub target_path: String,
    pub source_tree: BTreeMap<String, Fingerprint>,
    pub target_tree: BTreeMap<String, Fingerprint>,
    pub conflict: Conflict,
}
impl TransferManager {
    pub fn with_journal(concurrency: usize, path: std::path::PathBuf) -> Result<Arc<Self>, String> {
        let recovered: Vec<Recovery> = match std::fs::read(&path) {
            Ok(data) => serde_json::from_slice(&data).map_err(|e| {
                format!(
                    "Transfer journal invalid; preserved at {}: {e}",
                    path.display()
                )
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
            Err(e) => return Err(e.to_string()),
        };
        let recovered: Vec<Recovery> = recovered
            .into_iter()
            .filter(|r| !matches!(r.job.state, JobState::Complete | JobState::Skipped))
            .collect();
        let mut manager = Self::new(concurrency);
        let m = Arc::get_mut(&mut manager).unwrap();
        m.journal = Some(path);
        let mut recovered = recovered;
        let mut identities = std::collections::HashSet::new();
        for (index, record) in recovered.iter_mut().enumerate() {
            if record.id.is_empty() {
                record.id = format!("{}-recovery-{index}", m.token);
            }
            record.claimed = false;
            if !identities.insert(record.id.clone()) {
                return Err("Transfer journal has duplicate recovery identities".into());
            }
        }
        m.recovered = Mutex::new(recovered);
        Ok(manager)
    }
    pub fn recoveries(&self) -> Vec<Recovery> {
        self.recovered.lock().unwrap().clone()
    }
    pub fn persistence_error(&self) -> Option<String> {
        self.persistence_error.lock().unwrap().clone()
    }
    /// Claims one stable identity before yielding. Concurrent recovery and
    /// dismissal of a claimed record are rejected; unrelated records remain usable.
    pub async fn recover(self: &Arc<Self>, identity: &str) -> Result<u64, String> {
        let (mut claim, saved) = self.claim_recovery(identity)?;
        self.persist();
        let source = saved.source.resolve().await?;
        let target = saved.target.resolve().await?;
        if snapshot(&source, &saved.source_path).await? != saved.source_tree {
            return Err("Source changed; choose a new transfer".into());
        }
        if snapshot(&target, &saved.target_path).await? != saved.target_tree {
            return Err("Target changed; choose a new transfer".into());
        }
        let spec = TransferSpec::capture(
            source,
            saved.source_path,
            target,
            saved.target_path,
            saved.conflict,
        )
        .await?;
        if spec.source_tree != saved.source_tree {
            return Err("Source changed; choose a new transfer".into());
        }
        if spec.target_tree != saved.target_tree {
            return Err("Target changed; choose a new transfer".into());
        }
        let id = self.enqueue(spec, Some(saved.id));
        {
            let mut records = self.recovered.lock().unwrap();
            if let Some(index) = records.iter().position(|r| r.id == claim.identity) {
                records.remove(index);
            }
        }
        claim.consumed = true;
        self.persist();
        self.start(id);
        Ok(id)
    }
    fn claim_recovery(
        self: &Arc<Self>,
        identity: &str,
    ) -> Result<(RecoveryClaim, Recovery), String> {
        let mut records = self.recovered.lock().unwrap();
        let record = records
            .iter_mut()
            .find(|r| r.id == identity)
            .ok_or("Recovery missing")?;
        if record.claimed {
            return Err("Recovery is already being validated".into());
        }
        record.claimed = true;
        Ok((
            RecoveryClaim {
                manager: self.clone(),
                identity: identity.into(),
                consumed: false,
            },
            record.clone(),
        ))
    }
    pub fn dismiss_recovery(&self, identity: &str) -> Result<(), String> {
        let mut records = self.recovered.lock().unwrap();
        let index = records
            .iter()
            .position(|r| r.id == identity)
            .ok_or("Recovery missing")?;
        if records[index].claimed {
            return Err("Recovery is already being validated".into());
        }
        records.remove(index);
        drop(records);
        self.persist();
        Ok(())
    }
    fn persist(&self) {
        let Some(path) = &self.journal else { return };
        let _write = self.journal_write.lock().unwrap();
        let mut records = self.recovered.lock().unwrap().clone();
        records.extend(
            self.jobs
                .lock()
                .unwrap()
                .values()
                .filter(|w| !matches!(w.job.state, JobState::Complete | JobState::Skipped))
                .filter(|w| [&w.spec.source, &w.spec.target].into_iter().all(|e| !matches!(e, Endpoint::Remote { transport, .. } if transport.profile.contains_temporary_endpoint())))
                .map(|w| Recovery {
                    id: w.recovery_id.clone(),
                    claimed: false,
                    job: w.job.clone(),
                    source: SavedEndpoint::capture(&w.spec.source),
                    source_path: w.spec.source_path.clone(),
                    target: SavedEndpoint::capture(&w.spec.target),
                    target_path: w.spec.target_path.clone(),
                    source_tree: w.spec.source_tree.clone(),
                    target_tree: w.spec.target_tree.clone(),
                    conflict: w.spec.conflict,
                }),
        );
        let result = (|| {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let temp = path.with_extension("json.tmp");
            // A concurrent journal snapshot can observe the claimed record and
            // its newly enqueued task together. They retain the same identity.
            let records = records
                .into_iter()
                .map(|r| (r.id.clone(), r))
                .collect::<BTreeMap<_, _>>()
                .into_values()
                .collect::<Vec<_>>();
            let bytes = serde_json::to_vec_pretty(&records).map_err(|e| e.to_string())?;
            std::fs::write(&temp, bytes).map_err(|e| e.to_string())?;
            std::fs::rename(temp, path).map_err(|e| e.to_string())
        })();
        *self.persistence_error.lock().unwrap() = result.err();
    }
}
struct RecoveryClaim {
    manager: Arc<TransferManager>,
    identity: String,
    consumed: bool,
}
impl Drop for RecoveryClaim {
    fn drop(&mut self) {
        if !self.consumed {
            if let Some(record) = self
                .manager
                .recovered
                .lock()
                .unwrap()
                .iter_mut()
                .find(|r| r.id == self.identity)
            {
                record.claimed = false;
            }
            self.manager.persist();
        }
    }
}
type Snapshot = BTreeMap<String, Fingerprint>;
type SnapshotFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<Snapshot, String>> + Send + 'a>>;
fn snapshot<'a>(endpoint: &'a Endpoint, path: &'a str) -> SnapshotFuture<'a> {
    Box::pin(async move {
        let mut result = BTreeMap::new();
        if let Some(f) = Fingerprint::capture(endpoint, path).await? {
            let directory = f.entry.kind == Kind::Directory;
            result.insert(path.to_string(), f);
            if directory {
                for child in endpoint.list(path).await? {
                    result.extend(snapshot(endpoint, &child.path).await?);
                }
            }
        }
        Ok(result)
    })
}
