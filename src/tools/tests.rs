use super::*;
use crate::workspace::{AuthMethod, TargetMode, ToolKind, Workspace};
use std::{cell::Cell, rc::Rc};

#[test]
fn context_capture_excludes_secrets_and_does_not_follow_later_edits() {
    let mut workspace = Workspace::default();
    let id = workspace.open(ToolKind::Information, TargetMode::Follow);
    workspace.resources[0].profiles[0].credential_ref = Some("SECRET_REF".into());
    workspace.resources[0].profiles[0].auth = AuthMethod::PrivateKey {
        path: "PRIVATE_KEY_PATH".into(),
    };
    let instance = ToolProvider::builtin(ToolKind::Information)
        .instantiate(id, &workspace)
        .unwrap();
    workspace.activate_resource(2);
    workspace.resources[0].name = "Changed".into();
    let encoded = serde_json::to_string(instance.context()).unwrap();
    assert!(!encoded.contains("SECRET_REF"));
    assert!(!encoded.contains("PRIVATE_KEY_PATH"));
    assert_eq!(instance.context().resource.as_ref().unwrap().id, 1);
    assert_ne!(
        instance.context().resource.as_ref().unwrap().name,
        "Changed"
    );
}

#[test]
fn denied_host_calls_do_not_run_and_grants_are_per_instance() {
    let mut workspace = Workspace::default();
    let id = workspace.open(ToolKind::Information, TargetMode::Follow);
    let denied = ToolProvider::plugin("denied", ResourceCapabilities::default())
        .instantiate(id, &workspace)
        .unwrap();
    let allowed = ToolProvider::plugin("allowed", ResourceCapabilities::metadata())
        .instantiate(id, &workspace)
        .unwrap();
    assert!(denied.resource_context().is_err());
    assert!(allowed.resource_context().is_ok());
    assert!(denied.resource_context().is_err());
}

#[test]
fn unload_revokes_old_handles_and_drops_subscriptions_and_tasks_once() {
    let mut workspace = Workspace::default();
    let id = workspace.open(ToolKind::Information, TargetMode::Follow);
    let mut instance = ToolProvider::plugin("example", ResourceCapabilities::metadata())
        .instantiate(id, &workspace)
        .unwrap();
    let lease = instance.lease();
    let count = Rc::new(Cell::new(0));
    for _ in 0..2 {
        let count = count.clone();
        instance.on_cleanup(move || count.set(count.get() + 1));
    }
    instance.unload();
    instance.unload();
    assert_eq!(count.get(), 2);
    assert!(!lease.is_active());
    assert!(instance.resource_context().is_err());
    drop(instance);
    assert_eq!(count.get(), 2);
}
