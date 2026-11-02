use super::*;
use crate::workspace::{AuthMethod, TargetMode, ToolKind, Workspace};
use std::{cell::Cell, rc::Rc};

#[test]
fn context_capture_excludes_secrets_and_does_not_follow_later_edits() {
    let mut workspace = Workspace::demo_fixture();
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
    let mut workspace = Workspace::demo_fixture();
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
    let mut workspace = Workspace::demo_fixture();
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

#[test]
fn native_profiles_stay_inside_builtin_boundary_and_cli_metadata_stays_private() {
    let mut workspace = Workspace::demo_fixture();
    let id = workspace.open(ToolKind::Sftp, TargetMode::Follow);
    workspace.resources[0].profiles[0].ephemeral = true;
    workspace.resources[0].profiles[0].credential_ref = Some("native-only".into());
    let builtin = ToolProvider::builtin(ToolKind::Sftp)
        .instantiate(id, &workspace)
        .unwrap();
    let plugin = ToolProvider::plugin("example", ResourceCapabilities::metadata())
        .instantiate(id, &workspace)
        .unwrap();
    assert_eq!(
        builtin
            .native_target()
            .unwrap()
            .profile
            .credential_ref
            .as_deref(),
        Some("native-only")
    );
    assert!(plugin.native_target().is_none());
    assert!(plugin.resource_context().unwrap().resource.is_none());
    workspace.views[0].ephemeral = true;
    let transient_builtin = ToolProvider::builtin(ToolKind::Sftp)
        .instantiate(id, &workspace)
        .unwrap();
    assert!(transient_builtin.native_target().unwrap().profile.ephemeral);
    assert_eq!(
        transient_builtin
            .native_target()
            .unwrap()
            .profile
            .credential_ref
            .as_deref(),
        Some("native-only")
    );
}

#[test]
fn enabled_plugin_and_dock_placement_restore_through_sqlite() {
    use crate::{settings::Settings, workspace::Location};
    let mut settings = Settings::demo_fixture();
    let id = settings
        .workspace
        .open(ToolKind::Plugin, TargetMode::Fixed(2));
    settings.workspace.move_view(id, Location::Right);
    settings.workspace.views[0].plugin_enabled = true;
    let path = std::env::temp_dir().join(format!(
        "helix-plugin-restore-{}-{}.sqlite",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    settings.save(&path).unwrap();
    let restored = Settings::load(&path).unwrap();
    assert_eq!(restored.workspace.views[0].id, id);
    assert_eq!(restored.workspace.views[0].location, Location::Right);
    assert!(restored.workspace.views[0].plugin_enabled);
    assert_eq!(restored.workspace.target(id), Some(2));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn ephemeral_view_identity_is_refused_before_plugin_instantiation() {
    let mut workspace = Workspace::demo_fixture();
    let id = workspace.open(ToolKind::Plugin, TargetMode::Follow);
    workspace.views[0].ephemeral = true;
    assert!(
        ToolProvider::plugin("example", ResourceCapabilities::metadata())
            .instantiate(id, &workspace)
            .is_err(),
        "CLI-only view identities must never enter a JavaScript host context"
    );
}

#[test]
fn collection_activity_pauses_metadata_without_revoking_user_job_lease() {
    let mut workspace = crate::workspace::Workspace::demo_fixture();
    let id = workspace.open(
        crate::workspace::ToolKind::Plugin,
        crate::workspace::TargetMode::Follow,
    );
    let instance = ToolProvider::plugin("activity", ResourceCapabilities::metadata())
        .instantiate(id, &workspace)
        .unwrap();
    let lease = instance.lease();
    instance.set_collection_active(false);
    assert!(instance.resource_context().is_err());
    assert!(
        lease.is_active(),
        "pausing collection must not revoke submitted user jobs"
    );
    instance.set_collection_active(true);
    assert!(instance.resource_context().is_ok());
}
