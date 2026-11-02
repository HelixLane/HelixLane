use super::*;
use crate::sftp::{Endpoint, Fingerprint};
fn root(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("helixlane-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}
#[tokio::test]
async fn cancelled_upload_never_commits_partial_target() {
    let p = root("cancel");
    std::fs::write(p.join("source"), vec![1; 100000]).unwrap();
    let manager = TransferManager::new(1);
    let task = TransferSpec::capture(
        Endpoint::Local,
        p.join("source").display().to_string(),
        Endpoint::Local,
        p.join("target").display().to_string(),
        Conflict::Overwrite,
    )
    .await
    .unwrap();
    let id = manager.submit_paused(task);
    manager.cancel(id);
    manager.run(id).await;
    assert!(!p.join("target").exists());
    assert_eq!(manager.jobs()[0].state, JobState::Cancelled);
    assert_eq!(std::fs::read_dir(p).unwrap().count(), 1);
}
#[tokio::test]
async fn source_and_target_are_revalidated_before_retry() {
    let p = root("retry");
    std::fs::write(p.join("a"), b"first").unwrap();
    let dst = p.join("b").display().to_string();
    let spec = TransferSpec::capture(
        Endpoint::Local,
        p.join("a").display().to_string(),
        Endpoint::Local,
        dst.clone(),
        Conflict::Overwrite,
    )
    .await
    .unwrap();
    std::fs::write(p.join("a"), b"other").unwrap();
    let m = TransferManager::new(2);
    let id = m.submit_paused(spec);
    m.run(id).await;
    assert!(matches!(&m.jobs()[0].state,JobState::Failed(e) if e.contains("Source changed")));
    assert!(!std::path::Path::new(&dst).exists());
}
#[tokio::test]
async fn task_survives_closed_and_retargeted_views_and_conflicts() {
    let p = root("target");
    std::fs::write(p.join("a"), b"content").unwrap();
    let mut w = crate::workspace::Workspace::default();
    let view = w.open(
        crate::workspace::ToolKind::Sftp,
        crate::workspace::TargetMode::Follow,
    );
    let spec = TransferSpec::capture(
        Endpoint::Local,
        p.join("a").display().to_string(),
        Endpoint::Local,
        p.join("b").display().to_string(),
        Conflict::Overwrite,
    )
    .await
    .unwrap();
    let m = TransferManager::new(2);
    let id = m.submit_paused(spec);
    w.activate_resource(2);
    w.close(view);
    m.run(id).await;
    assert_eq!(std::fs::read(p.join("b")).unwrap(), b"content");
    assert_eq!(m.jobs()[0].state, JobState::Complete);
    let spec = TransferSpec::capture(
        Endpoint::Local,
        p.join("a").display().to_string(),
        Endpoint::Local,
        p.join("b").display().to_string(),
        Conflict::Overwrite,
    )
    .await
    .unwrap();
    std::fs::write(p.join("b"), b"updated").unwrap();
    let id = m.submit_paused(spec);
    m.run(id).await;
    assert!(matches!(&m.jobs()[1].state,JobState::Failed(e) if e.contains("Target changed")));
}
#[cfg(unix)]
#[tokio::test]
async fn recursive_copy_does_not_follow_symlinks() {
    let p = root("links");
    std::fs::create_dir(p.join("src")).unwrap();
    std::fs::write(p.join("src/file"), b"yes").unwrap();
    std::os::unix::fs::symlink("/etc", p.join("src/link")).unwrap();
    let spec = TransferSpec::capture(
        Endpoint::Local,
        p.join("src").display().to_string(),
        Endpoint::Local,
        p.join("dst").display().to_string(),
        Conflict::Overwrite,
    )
    .await
    .unwrap();
    let m = TransferManager::new(1);
    let id = m.submit_paused(spec);
    m.run(id).await;
    assert!(p.join("dst/file").exists());
    assert!(!p.join("dst/link").exists());
    assert_eq!(m.jobs()[0].skipped_links, 1);
}
#[tokio::test]
async fn same_length_external_change_is_detected() {
    let p = root("fingerprint");
    let f = p.join("a");
    std::fs::write(&f, b"aaa").unwrap();
    let before = Fingerprint::capture(&Endpoint::Local, &f.display().to_string())
        .await
        .unwrap();
    std::fs::write(&f, b"bbb").unwrap();
    assert_ne!(
        before,
        Fingerprint::capture(&Endpoint::Local, &f.display().to_string())
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn preexisting_temp_is_never_removed_on_create_failure() {
    let p = root("existing-temp");
    std::fs::write(p.join("a"), b"payload").unwrap();
    let to = p.join("b").display().to_string();
    let m = TransferManager::new(1);
    let temp = m.temporary_path(1, &to);
    std::fs::write(&temp, b"user file").unwrap();
    let spec = TransferSpec::capture(
        Endpoint::Local,
        p.join("a").display().to_string(),
        Endpoint::Local,
        to,
        Conflict::Overwrite,
    )
    .await
    .unwrap();
    m.run(m.submit_paused(spec)).await;
    assert_eq!(std::fs::read(temp).unwrap(), b"user file");
}
#[tokio::test]
async fn recursive_resume_revalidates_file_digests() {
    let p = root("recursive-state");
    std::fs::create_dir(p.join("src")).unwrap();
    std::fs::write(p.join("src/a"), b"one").unwrap();
    let spec = TransferSpec::capture(
        Endpoint::Local,
        p.join("src").display().to_string(),
        Endpoint::Local,
        p.join("dst").display().to_string(),
        Conflict::Overwrite,
    )
    .await
    .unwrap();
    std::fs::write(p.join("src/a"), b"two").unwrap();
    let m = TransferManager::new(1);
    m.run(m.submit_paused(spec)).await;
    assert!(matches!(&m.jobs()[0].state,JobState::Failed(e) if e.contains("Source changed")));
}
#[tokio::test]
async fn restart_only_offers_explicit_recovery_and_revalidates() {
    let p = root("journal");
    std::fs::write(p.join("a"), b"one").unwrap();
    let spec = TransferSpec::capture(
        Endpoint::Local,
        p.join("a").display().to_string(),
        Endpoint::Local,
        p.join("b").display().to_string(),
        Conflict::Overwrite,
    )
    .await
    .unwrap();
    let m = TransferManager::with_journal(1, p.join("tasks.json")).unwrap();
    m.submit_paused(spec);
    drop(m);
    let m = TransferManager::with_journal(1, p.join("tasks.json")).unwrap();
    assert!(m.jobs().is_empty());
    assert_eq!(m.recoveries().len(), 1);
    std::fs::write(p.join("a"), b"two").unwrap();
    let identity = m.recoveries()[0].id.clone();
    assert!(m.recover(&identity).await.is_err());
    assert!(!m.recoveries()[0].claimed);
    assert!(!p.join("b").exists());
}

#[tokio::test]
async fn aliased_child_destination_is_rejected_before_recursive_copy() {
    let p = root("alias");
    std::fs::create_dir(p.join("src")).unwrap();
    let source = format!("{}/./src", p.display());
    let target = format!("{}/src/child", p.display());
    assert!(
        TransferSpec::capture(
            Endpoint::Local,
            source,
            Endpoint::Local,
            target,
            Conflict::Overwrite
        )
        .await
        .is_err()
    );
}
#[tokio::test]
async fn cancelled_running_copy_preserves_original_and_cleans_owned_temp() {
    let p = root("running-cancel");
    std::fs::write(p.join("src"), vec![42; 8 * 1024 * 1024]).unwrap();
    std::fs::write(p.join("dst"), b"original").unwrap();
    let spec = TransferSpec::capture(
        Endpoint::Local,
        p.join("src").display().to_string(),
        Endpoint::Local,
        p.join("dst").display().to_string(),
        Conflict::Overwrite,
    )
    .await
    .unwrap();
    let m = TransferManager::new(1);
    let id = m.submit_paused(spec);
    let runner = m.clone();
    let handle = tokio::spawn(async move {
        runner.run(id).await;
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if m.jobs()[0].bytes > 0 {
                m.cancel(id);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    handle.await.unwrap();
    assert_eq!(m.jobs()[0].state, JobState::Cancelled);
    assert_eq!(std::fs::read(p.join("dst")).unwrap(), b"original");
    assert!(
        !std::fs::read_dir(&p).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".part"))
    );
}

#[tokio::test]
async fn conflict_skip_and_rename_preserve_original_and_retry_revalidates() {
    let p = root("conflicts");
    std::fs::write(p.join("src"), b"new").unwrap();
    std::fs::write(p.join("dst"), b"old").unwrap();
    let manager = TransferManager::new(1);
    for policy in [Conflict::Skip, Conflict::Rename] {
        let spec = TransferSpec::capture(
            Endpoint::Local,
            p.join("src").display().to_string(),
            Endpoint::Local,
            p.join("dst").display().to_string(),
            policy,
        )
        .await
        .unwrap();
        manager.run(manager.submit_paused(spec)).await;
    }
    assert_eq!(manager.jobs()[0].state, JobState::Skipped);
    assert_eq!(std::fs::read(p.join("dst")).unwrap(), b"old");
    assert_eq!(std::fs::read(p.join("dst (1)")).unwrap(), b"new");
    let spec = TransferSpec::capture(
        Endpoint::Local,
        p.join("src").display().to_string(),
        Endpoint::Local,
        p.join("retry").display().to_string(),
        Conflict::Overwrite,
    )
    .await
    .unwrap();
    let id = manager.submit_paused(spec);
    let temp = std::path::PathBuf::from(
        manager.temporary_path(id, &p.join("retry").display().to_string()),
    );
    std::fs::write(&temp, b"blocking existing file").unwrap();
    manager.run(id).await;
    assert!(matches!(manager.jobs()[2].state, JobState::Failed(_)));
    std::fs::remove_file(temp).unwrap();
    manager.retry(id).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if manager.jobs()[2].state == JobState::Complete {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(manager.jobs()[2].attempts, 2);
}

#[test]
fn restarted_managers_use_fresh_staging_and_temp_names() {
    let a = TransferManager::new(1);
    let b = TransferManager::new(1);
    assert_ne!(
        a.temporary_path(1, "/target"),
        b.temporary_path(1, "/target")
    );
}

#[tokio::test]
async fn cancel_during_final_destination_digest_never_commits_replacement() {
    let p = root("final-digest-cancel");
    std::fs::write(p.join("source"), b"new-content").unwrap();
    let target = p.join("target");
    let original_len = 128 * 1024 * 1024;
    std::fs::File::create(&target)
        .unwrap()
        .set_len(original_len)
        .unwrap();
    let spec = TransferSpec::capture(
        Endpoint::Local,
        p.join("source").display().to_string(),
        Endpoint::Local,
        target.display().to_string(),
        Conflict::Overwrite,
    )
    .await
    .unwrap();
    let manager = TransferManager::new(1);
    let id = manager.submit_paused(spec);
    let watcher = manager.clone();
    let cancellation = std::thread::spawn(move || {
        while watcher.jobs()[0].bytes == 0 {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        // The tiny payload and its source digest finish before this delay; the
        // 128 MiB destination's final content validation remains outstanding.
        std::thread::sleep(std::time::Duration::from_millis(50));
        watcher.cancel(id);
    });
    manager.run(id).await;
    cancellation.join().unwrap();
    assert_eq!(manager.jobs()[0].state, JobState::Cancelled);
    assert_eq!(std::fs::metadata(&target).unwrap().len(), original_len);
    assert!(
        !std::fs::read_dir(p).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".part"))
    );
}

#[tokio::test]
async fn recovery_claim_is_stable_across_overlapping_recover_and_dismiss() {
    use std::{future::Future, task::Poll};
    let p = root("recovery-overlap");
    std::fs::write(p.join("source"), vec![8; 8 * 1024 * 1024]).unwrap();
    let journal = p.join("tasks.json");
    let initial = TransferManager::with_journal(1, journal.clone()).unwrap();
    for name in ["a", "b", "c"] {
        let spec = TransferSpec::capture(
            Endpoint::Local,
            p.join("source").display().to_string(),
            Endpoint::Local,
            p.join(name).display().to_string(),
            Conflict::Overwrite,
        )
        .await
        .unwrap();
        initial.submit_paused(spec);
    }
    drop(initial);
    let manager = TransferManager::with_journal(1, journal).unwrap();
    let saved = manager.recoveries();
    let a = saved[0].id.clone();
    let b = saved[1].id.clone();
    let c = saved[2].id.clone();
    let mut first = Box::pin(manager.recover(&a));
    // Poll exactly once, then hold the validation future suspended. Filesystem
    // work may finish in a worker, but cannot advance the claim to submission.
    let pending =
        std::future::poll_fn(|cx| Poll::Ready(first.as_mut().poll(cx).is_pending())).await;
    assert!(pending);
    assert!(
        manager
            .recoveries()
            .iter()
            .find(|r| r.id == a)
            .unwrap()
            .claimed
    );
    assert!(manager.dismiss_recovery(&a).is_err());
    assert!(manager.recover(&a).await.is_err());
    manager.dismiss_recovery(&c).unwrap();
    assert!(manager.recoveries().iter().any(|r| r.id == b));
    first.await.unwrap();
    assert_eq!(manager.jobs().len(), 1);
    let remaining = manager.recoveries();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, b);
    // Job state is published just before its journal replacement. Wait for
    // the actual durable snapshot rather than racing that separate write.
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let records: Vec<Recovery> =
                serde_json::from_slice(&std::fs::read(p.join("tasks.json")).unwrap()).unwrap();
            if manager.jobs()[0].state == JobState::Complete
                && records.len() == 1
                && records[0].id == b
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let restarted = TransferManager::with_journal(1, p.join("tasks.json")).unwrap();
    assert_eq!(restarted.recoveries().len(), 1);
    assert_eq!(restarted.recoveries()[0].id, b);
}

#[tokio::test]
async fn commit_rechecks_file_destination_kind_after_fingerprint() {
    let p = root("commit-kind");
    let target = p.join("target").display().to_string();
    std::fs::write(&target, b"original file").unwrap();
    let baseline = Fingerprint::capture(&Endpoint::Local, &target)
        .await
        .unwrap();
    std::fs::remove_file(&target).unwrap();
    std::fs::create_dir(&target).unwrap();
    std::fs::write(
        std::path::Path::new(&target).join("original"),
        b"retained child",
    )
    .unwrap();
    let error = validate_commit_target(
        &Endpoint::Local,
        &target,
        &crate::sftp::Kind::File,
        baseline.as_ref(),
        &AtomicBool::new(false),
    )
    .await
    .unwrap_err();
    assert!(error.contains("Incompatible destination type"));
    assert_eq!(
        std::fs::read(std::path::Path::new(&target).join("original")).unwrap(),
        b"retained child"
    );
}
#[tokio::test]
async fn abandoned_recovery_validation_restores_exact_claim() {
    use std::{future::Future, task::Poll};
    let p = root("recovery-drop");
    std::fs::write(p.join("source"), vec![1; 4 * 1024 * 1024]).unwrap();
    let journal = p.join("tasks.json");
    let original = TransferManager::with_journal(1, journal.clone()).unwrap();
    let spec = TransferSpec::capture(
        Endpoint::Local,
        p.join("source").display().to_string(),
        Endpoint::Local,
        p.join("target").display().to_string(),
        Conflict::Overwrite,
    )
    .await
    .unwrap();
    original.submit_paused(spec);
    drop(original);
    let manager = TransferManager::with_journal(1, journal).unwrap();
    let id = manager.recoveries()[0].id.clone();
    let mut recovery = Box::pin(manager.recover(&id));
    assert!(std::future::poll_fn(|cx| Poll::Ready(recovery.as_mut().poll(cx).is_pending())).await);
    assert!(manager.recoveries()[0].claimed);
    drop(recovery);
    assert!(!manager.recoveries()[0].claimed);
    assert_eq!(manager.recoveries()[0].id, id);
    assert!(manager.jobs().is_empty());
    manager.dismiss_recovery(&id).unwrap();
    assert!(manager.recoveries().is_empty());
}

#[tokio::test]
async fn scoped_close_cancels_only_submitting_views_jobs() {
    let p = root("close-origin");
    std::fs::write(p.join("source"), b"data").unwrap();
    let spec = TransferSpec::capture(
        Endpoint::Local,
        p.join("source").display().to_string(),
        Endpoint::Local,
        p.join("target").display().to_string(),
        Conflict::Rename,
    )
    .await
    .unwrap();
    let manager = TransferManager::new(1);
    let a = manager.submit_paused_for(spec.clone(), Some(10));
    let b = manager.submit_paused_for(spec, Some(20));
    manager.cancel_views(&[10]);
    let jobs = manager.jobs();
    assert_eq!(
        jobs.iter().find(|j| j.id == a).unwrap().state,
        JobState::Cancelled
    );
    assert_eq!(
        jobs.iter().find(|j| j.id == b).unwrap().state,
        JobState::Queued
    );
}
