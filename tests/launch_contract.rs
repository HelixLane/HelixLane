use helixlane::launch::*;
fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|v| v.to_string()).collect()
}
fn open(v: &[&str]) -> OpenRequest {
    match parse_native(&args(v)).unwrap() {
        Entry::Open(r) => r,
        _ => panic!("expected request"),
    }
}
#[test]
fn native_uri_and_saved_resource_parity() {
    let a = open(&[
        "open",
        "sftp://alice@example.invalid:2222/a%20b",
        "--name",
        "A",
    ]);
    let b = open(&[
        "helixlane://open?protocol=sftp&host=example.invalid&user=alice&port=2222&path=%2Fa%20b&name=A",
    ]);
    assert_eq!(a.connection(), b.connection());
    assert_eq!(a.remote_path, b.remote_path);
    assert_eq!(a.name, b.name);
    assert!(parse_native(&args(&["open", "--resource", "1", "--user", "alice"])).is_err());
}
#[test]
fn unsafe_inputs_are_sanitized() {
    for value in [
        "helixlane://open?host=x&password=canary",
        "helixlane://open?host=x&host=y",
        "helixlane://open?host=%ZZ",
        "helixlane://open?host=x&path=%FF",
        "ssh://u:canary@host",
        "helixlane://open?host=x&path=%00",
    ] {
        let error = parse_native(&args(&[value])).unwrap_err();
        assert!(!error.contains("canary"));
    }
    assert!(parse_native(&args(&["open", "host;touch x"])).is_err());
}
#[test]
fn proxies_preserve_tokens_and_decode_legacy_credentials() {
    let Entry::Open(p) = parse_putty(&args(&[
        "-ssh", "-P", "2222", "-l", "alice", "-pw", "a b&$", "host",
    ]))
    .unwrap() else {
        panic!()
    };
    assert_eq!(p.secret.unwrap().expose(), "a b&$");
    let Entry::Open(w) = parse_winscp(&args(&[
        "sftp://alice:a%3Ab%26%20word@host/a%20b",
        "/sessionname=Work",
    ]))
    .unwrap() else {
        panic!()
    };
    assert_eq!(w.secret.unwrap().expose(), "a:b& word");
    assert_eq!(w.remote_path.as_deref(), Some("/a b"));
    for v in ["-i", "-load"] {
        assert!(parse_putty(&args(&[v, "canary"])).is_err());
    }
    assert!(parse_winscp(&args(&["/script=canary"])).is_err());
}
#[test]
fn wire_is_bounded_versioned_and_secret_redacted() {
    let r = open(&["open", "alice@host", "--password", "canary"]);
    assert!(!format!("{r:?}").contains("canary"));
    let bytes = encode(&r).unwrap();
    assert_eq!(decode(&bytes).unwrap().secret.unwrap().expose(), "canary");
    assert!(decode(&vec![0; MAX_FRAME + 1]).is_err());
    assert!(decode(br#"{"version":99}"#).is_err());
}
#[test]
fn saved_follow_identity_survives_external_activation() {
    use helixlane::{settings::Settings, workspace::*};
    let mut s = Settings::default();
    let saved = s.workspace.open(ToolKind::Terminal, TargetMode::Follow);
    s.workspace.views[0].session_id = Some(99);
    s.dock_layout = Some(
        serde_json::json!({"panel_name":"TabPanel","children":[{"panel_name":"helixlane.tool","info":{"panel":saved}}],"info":{"tabs":{"active_index":0}}}),
    );
    let path = std::env::temp_dir().join(format!("helix-i1-{}.sqlite", std::process::id()));
    let r = open(&["open", "alice@host", "--new-view"]);
    let temp = r.apply(&mut s.workspace).unwrap().view_id;
    let baseline = s.persistent_snapshot();
    for _ in 0..5 {
        let activate = open(&["open", "alice@host", "--activate"]);
        assert_eq!(activate.apply(&mut s.workspace).unwrap().view_id, saved);
        assert!(
            !s.workspace
                .views
                .iter()
                .find(|v| v.id == saved)
                .unwrap()
                .ephemeral
        );
        assert_eq!(
            s.workspace
                .views
                .iter()
                .find(|v| v.id == saved)
                .unwrap()
                .session_id,
            Some(99)
        );
        assert_eq!(s.persistent_snapshot().workspace, baseline.workspace);
        s.save(&path).unwrap();
        let restored = Settings::load(&path).unwrap();
        assert_eq!(restored.workspace.views[0].id, saved);
        assert_eq!(restored.workspace.views[0].mode, TargetMode::Follow);
        assert_eq!(restored.workspace.next_id, baseline.workspace.next_id);
        assert_eq!(restored.dock_layout, baseline.dock_layout);
    }
    std::fs::remove_file(path).unwrap();
    s.workspace.set_mode(temp, TargetMode::Fixed(1));
    assert!(
        !s.persistent_snapshot()
            .workspace
            .views
            .iter()
            .any(|v| v.id == temp)
    );
    assert!(s.workspace.views.iter().any(|v| v.id == temp));
}
#[test]
fn new_window_has_no_workspace_side_effects() {
    let r = open(&["open", "alice@host", "--new-window"]);
    let mut w = helixlane::workspace::Workspace::default();
    let before = w.clone();
    assert!(r.apply(&mut w).is_err());
    assert_eq!(w, before);
}
#[test]
fn explicit_auth_and_option_order_are_consistent() {
    for options in [
        vec!["open", "--auth-none", "alice@host"],
        vec!["open", "alice@host", "--auth-none"],
    ] {
        assert!(matches!(
            open(&options).connection().unwrap().auth,
            helixlane::workspace::AuthMethod::None
        ));
    }
    for options in [
        vec!["open", "host", "--password", "x", "--auth-none"],
        vec!["open", "host", "--ssh", "--sftp"],
        vec!["open", "host", "--port", "0"],
        vec!["open", "host", "--user", "a", "--user", "b"],
        vec!["open", "--resource", "1", "--password", "x"],
    ] {
        assert!(parse_native(&args(&options)).is_err());
    }
}
#[test]
fn malformed_uri_fields_and_controls_fail() {
    for raw in [
        "helixlane://open?host=x&mode=bad",
        "helixlane://open?host=x&port=0",
        "helixlane://open?host=x&user=a&resource=1",
        "helixlane://u@open?host=x",
        "helixlane://open/extra?host=x",
        "helixlane://open?host=x#fragment",
        "helixlane://open?host=x&name=",
        "helixlane://open?host=x&user=%0A",
        "helixlane://open?host=x&path=%7f",
        "helixlane://open?host=x&%68ost=y",
        "helixlane://open?host=x&protocol=ftp",
        "helixlane://open?host=x&path=%C0%AF",
    ] {
        assert!(parse_native(&args(&[raw])).is_err(), "{raw}");
    }
}
#[test]
fn activation_and_new_view_have_stable_distinct_identities() {
    let mut w = helixlane::workspace::Workspace::default();
    let r = open(&["open", "alice@host"]);
    let first = r.apply(&mut w).unwrap();
    assert_eq!(r.apply(&mut w).unwrap(), first);
    let fresh = open(&["open", "alice@host", "--new-view"])
        .apply(&mut w)
        .unwrap();
    assert_ne!(fresh.view_id, first.view_id);
    assert_eq!(fresh.resource_id, first.resource_id);
    assert_eq!(
        w.resources
            .iter()
            .filter(|r| r.profiles[0].ephemeral)
            .count(),
        1
    );
}
#[test]
fn ephemeral_snapshot_prunes_only_temporary_nodes_and_never_live_workspace() {
    use helixlane::{settings::Settings, workspace::*};
    let mut s = Settings::default();
    let a = s.workspace.open(ToolKind::Terminal, TargetMode::Fixed(1));
    let temp = open(&["open", "alice@host", "--new-view"])
        .apply(&mut s.workspace)
        .unwrap()
        .view_id;
    let b = s.workspace.open(ToolKind::Sftp, TargetMode::Fixed(2));
    s.dock_layout = Some(
        serde_json::json!({"panel_name":"StackPanel","children":[{"panel_name":"TabPanel","children":[{"panel_name":"helixlane.tool","info":{"panel":a}},{"panel_name":"helixlane.tool","info":{"panel":temp}},{"panel_name":"helixlane.tool","info":{"panel":b}}],"info":{"tabs":{"active_index":2}}},{"panel_name":"SavedUnrelated","children":[],"info":{"tabs":{"active_index":7}}}],"info":{"stack":{"sizes":[381.5,197.25]}}}),
    );
    let before = s.workspace.clone();
    let saved = s.persistent_snapshot();
    assert_eq!(s.workspace, before);
    let layout = saved.dock_layout.as_ref().unwrap();
    assert_eq!(
        layout["info"]["stack"]["sizes"],
        serde_json::json!([381.5, 197.25])
    );
    assert_eq!(layout["children"][0]["info"]["tabs"]["active_index"], 1);
    assert_eq!(layout["children"][1]["info"]["tabs"]["active_index"], 7);
    assert_eq!(
        layout["children"][0]["children"].as_array().unwrap().len(),
        2
    );
    let path = std::env::temp_dir().join(format!(
        "helix-launch-contract-{}.sqlite",
        std::process::id()
    ));
    s.save(&path).unwrap();
    let raw = std::fs::read(&path).unwrap();
    assert!(!raw.windows(b"alice@host".len()).any(|v| v == b"alice@host"));
    let loaded = Settings::load(&path).unwrap();
    assert!(!loaded.workspace.views.iter().any(|v| v.id == temp));
    assert!(!loaded.workspace.resources.iter().any(|r| r.name == "host"));
    std::fs::remove_file(path).unwrap();
}
#[test]
fn wire_rejects_invalid_targets_unknown_fields_and_controls() {
    let r = open(&["open", "alice@host", "--password", "canary"]);
    let encoded = encode(&r).unwrap();
    let original: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    for (field, value) in [
        ("version", serde_json::json!(0)),
        ("unknown", serde_json::json!(true)),
        ("password", serde_json::json!("bad\nvalue")),
    ] {
        let mut modified = original.clone();
        modified[field] = value;
        assert!(decode(&serde_json::to_vec(&modified).unwrap()).is_err());
    }
    let mut huge = r;
    huge.secret = Some(helixlane::ssh::credentials::Secret::new(
        "a".repeat(MAX_FRAME),
    ));
    assert!(encode(&huge).is_err());
}
#[test] fn explicit_sftp_root_path_reaches_request(){assert_eq!(open(&["open","sftp://alice@host/"]).remote_path.as_deref(),Some("/"));}
#[test] fn putty_rejects_non_host_sftp_urls(){assert!(parse_putty(&args(&["sftp://alice@host/a"])).is_err());}
