#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_resource_corruption_recovers_snapshot_and_secret_references() {
        let path = std::env::temp_dir().join(format!(
            "helix-partial-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        let mut s = Settings::demo_fixture();
        s.workspace.resources.truncate(2);
        s.workspace.resources[0].profiles[0].credential_ref = Some("keyring:host-a".into());
        s.workspace.resources[1].profiles[0].credential_ref = Some("keyring:host-b".into());
        s.save(&path).unwrap();
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute("UPDATE resources SET value='{' WHERE id=1", [])
            .unwrap();
        let recovered = Settings::load(&path).unwrap();
        assert_eq!(recovered.workspace.resources, s.workspace.resources);
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM recovery", [], |r| r.get::<_, u32>(0))
                .unwrap(),
            1
        );
        recovered.save(&path).unwrap();
        assert_eq!(
            Settings::load(&path).unwrap().workspace.resources,
            s.workspace.resources
        );
        drop(db);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn restore_resets_sessions_preserves_preferences_and_repairs_id_allocator() {
        let path =
            std::env::temp_dir().join(format!("helix-restore-{}.sqlite", std::process::id()));
        let mut s = Settings {
            language: Language::Chinese,
            theme: ThemePreference::Light,
            ..Settings::demo_fixture()
        };
        let id = s.workspace.open(
            crate::workspace::ToolKind::Terminal,
            crate::workspace::TargetMode::Fixed(1),
        );
        s.workspace.views[0].session_id = Some(99);
        s.workspace.next_id = 1;
        s.save(&path).unwrap();
        let mut restored = Settings::load(&path).unwrap();
        assert_eq!(restored.language, Language::Chinese);
        assert_eq!(restored.theme, ThemePreference::Light);
        assert_eq!(restored.workspace.views[0].session_id, None);
        assert!(restored.workspace.add_resource() > id);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn corrupt_settings_recover_credentials() {
        let path = std::env::temp_dir().join(format!("helix-test-{}.sqlite", std::process::id()));
        let mut s = Settings::demo_fixture();
        s.workspace.resources[0].profiles[0].credential_ref = Some("keyring:opaque".into());
        s.save(&path).unwrap();
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute("UPDATE settings SET value='{'", []).unwrap();
        let recovered = Settings::load(&path).unwrap();
        assert_eq!(
            recovered.workspace.resources[0].profiles[0]
                .credential_ref
                .as_deref(),
            Some("keyring:opaque")
        );
        drop(db);
        std::fs::remove_file(path).unwrap();
    }
}
use crate::{i18n::Language, workspace::Workspace};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ThemePreference {
    #[default]
    System,
    Dark,
    Light,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TerminalFont {
    pub family: String,
    pub size: u16,
}
impl Default for TerminalFont {
    fn default() -> Self {
        Self {
            family: "monospace".into(),
            size: 14,
        }
    }
}
impl TerminalFont {
    pub fn sanitized(&self) -> Self {
        let family = if self.family.is_empty()
            || self.family.len() > 128
            || self.family.chars().any(char::is_control)
        {
            "monospace".into()
        } else {
            self.family.clone()
        };
        Self {
            family,
            size: self.size.clamp(10, 28),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    pub version: u32,
    pub language: Language,
    pub theme: ThemePreference,
    #[serde(default)]
    pub terminal_font: TerminalFont,
    pub workspace: Workspace,
    #[serde(default)]
    pub dock_layout: Option<serde_json::Value>,
    #[serde(default)]
    pub top_layout: Option<serde_json::Value>,
    #[serde(skip)]
    pub recovery: Option<String>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 2,
            language: Language::default(),
            theme: ThemePreference::System,
            terminal_font: TerminalFont::default(),
            workspace: Workspace::default(),
            dock_layout: None,
            top_layout: None,
            recovery: None,
        }
    }
}
impl Settings {
    pub fn demo_fixture() -> Self {
        Self {
            workspace: Workspace::demo_fixture(),
            ..Self::default()
        }
    }

    pub fn path() -> PathBuf {
        if let Some(path) = std::env::var_os("HELIXLANE_SETTINGS") {
            return PathBuf::from(path);
        }
        fn env_path(name: &str) -> Option<PathBuf> {
            std::env::var_os(name)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
        }
        #[cfg(target_os = "windows")]
        let root = env_path("APPDATA")
            .or_else(|| env_path("USERPROFILE").map(|p| p.join("AppData/Roaming")));
        #[cfg(target_os = "macos")]
        let root = env_path("HOME").map(|p| p.join("Library/Application Support"));
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let root =
            env_path("XDG_CONFIG_HOME").or_else(|| env_path("HOME").map(|p| p.join(".config")));
        // Missing OS home is an explicit startup error, never a cwd-relative database.
        root.expect("Cannot locate user settings directory; set HELIXLANE_SETTINGS explicitly")
            .join("helixlane/settings.sqlite")
    }
    /// One central boundary covers toolbar/profile/layout/autosave writes together.
    /// Transient resources and their view identities never enter recovery snapshots.
    pub fn persistent_snapshot(&self) -> Self {
        let mut saved = self.clone();
        saved.terminal_font = saved.terminal_font.sanitized();
        for resource in &mut saved.workspace.resources {
            for profile in &mut resource.profiles {
                profile.route = None;
            }
        }
        let transient: std::collections::HashSet<_> = saved
            .workspace
            .resources
            .iter()
            .filter(|r| r.profiles.iter().any(|p| p.ephemeral))
            .map(|r| r.id)
            .collect();
        let removed: std::collections::HashSet<_> = saved.workspace.views.iter().filter(|v|v.ephemeral || matches!(v.mode,crate::workspace::TargetMode::Fixed(id) if transient.contains(&id))).map(|v|v.id).collect();
        for id in &removed {
            saved.workspace.close(*id);
        }
        saved
            .workspace
            .resources
            .retain(|r| !transient.contains(&r.id));
        if saved
            .workspace
            .active_resource
            .is_some_and(|id| transient.contains(&id))
        {
            saved.workspace.active_resource = saved.workspace.resources.first().map(|r| r.id);
        }
        fn clean(value: &mut serde_json::Value, removed: &std::collections::HashSet<u64>) {
            fn transient(v: &serde_json::Value, removed: &std::collections::HashSet<u64>) -> bool {
                v.get("panel_name").and_then(|v| v.as_str()) == Some("helixlane.tool")
                    && v.get("info")
                        .and_then(|v| v.get("panel"))
                        .and_then(|v| v.as_u64())
                        .is_some_and(|id| removed.contains(&id))
            }
            if transient(value, removed) {
                *value = serde_json::json!({"panel_name":"TabPanel","children":[],"info":{"tabs":{"active_index":0}}});
                return;
            }
            match value {
                serde_json::Value::Array(values) => {
                    for v in values {
                        clean(v, removed);
                    }
                }
                serde_json::Value::Object(map) => {
                    if let Some(children) = map.get_mut("children").and_then(|v| v.as_array_mut()) {
                        let kept: Vec<bool> =
                            children.iter().map(|v| !transient(v, removed)).collect();
                        let changed = kept.iter().any(|keep| !*keep);
                        let mut index = 0;
                        children.retain(|_| {
                            let keep = kept[index];
                            index += 1;
                            keep
                        });
                        if changed && let Some(info) = map.get_mut("info") {
                            if let Some(active) =
                                info.get_mut("tabs").and_then(|v| v.get_mut("active_index"))
                            {
                                let old = active.as_u64().unwrap_or(0) as usize;
                                let adjusted =
                                    kept.iter().take(old).filter(|keep| **keep).count().min(
                                        kept.iter().filter(|keep| **keep).count().saturating_sub(1),
                                    );
                                *active = serde_json::json!(adjusted);
                            }
                            if let Some(sizes) = info
                                .get_mut("stack")
                                .and_then(|v| v.get_mut("sizes"))
                                .and_then(|v| v.as_array_mut())
                            {
                                let mut index = 0;
                                sizes.retain(|_| {
                                    let keep = kept.get(index).copied().unwrap_or(false);
                                    index += 1;
                                    keep
                                });
                            }
                        }
                    }
                    for v in map.values_mut() {
                        clean(v, removed);
                    }
                }
                _ => {}
            }
        }
        for value in [&mut saved.dock_layout, &mut saved.top_layout]
            .into_iter()
            .flatten()
        {
            clean(value, &removed);
        }
        for view in &mut saved.workspace.views {
            if view.page.resource.is_some_and(|id| transient.contains(&id)) {
                view.page = Default::default();
            }
        }
        for window in &mut saved.workspace.windows {
            if window
                .active_resource
                .is_some_and(|id| transient.contains(&id))
            {
                window.active_resource = None;
            }
            if let Some(value) = &mut window.dock_layout {
                clean(value, &removed);
            }
            if let Some(value) = &mut window.top_layout {
                clean(value, &removed);
            }
        }
        saved
    }
    fn database(path: &Path) -> rusqlite::Result<rusqlite::Connection> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let db = rusqlite::Connection::open(path)?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS settings (id INTEGER PRIMARY KEY, value TEXT NOT NULL); CREATE TABLE IF NOT EXISTS resources (id INTEGER PRIMARY KEY,value TEXT NOT NULL); CREATE TABLE IF NOT EXISTS recovery (value TEXT NOT NULL);")?;
        Ok(db)
    }
    pub fn load(path: &Path) -> Result<Self, String> {
        let db = Self::database(path).map_err(|e| e.to_string())?;
        let raw = db
            .query_row("SELECT value FROM settings WHERE id=1", [], |r| {
                r.get::<_, String>(0)
            })
            .ok();
        let mut snapshot_valid = false;
        let mut s = match raw {
            Some(ref raw) => match serde_json::from_str::<Self>(raw) {
                Ok(s) if matches!(s.version, 1 | 2) => {
                    snapshot_valid = true;
                    s
                }
                _ => {
                    let _ = db.execute("INSERT INTO recovery(value) VALUES(?1)", [raw]);
                    Self {
                        recovery: Some("recovered_settings".into()),
                        ..Self::default()
                    }
                }
            },
            None => Self::default(),
        };
        let mut statement = db
            .prepare("SELECT id,value FROM resources ORDER BY id")
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([], |r| Ok((r.get::<_, u64>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        // The transactional settings snapshot is a second recoverable copy, not disposable
        // just because one independently stored resource row was damaged.
        let mut resources: std::collections::BTreeMap<_, _> = if snapshot_valid {
            s.workspace
                .resources
                .iter()
                .cloned()
                .map(|r| (r.id, r))
                .collect()
        } else {
            Default::default()
        };
        let mut rows_found = false;
        for row in rows {
            let (id, raw) = row.map_err(|e| e.to_string())?;
            rows_found = true;
            match serde_json::from_str::<crate::workspace::Resource>(&raw) {
                Ok(resource) if resource.id == id => {
                    resources.insert(id, resource);
                }
                _ => {
                    let archived =
                        serde_json::json!({"kind":"resource","resource_id":id,"raw":raw});
                    db.execute(
                        "INSERT INTO recovery(value) VALUES(?1)",
                        [archived.to_string()],
                    )
                    .map_err(|e| e.to_string())?;
                    s.recovery = Some("recovered_resource".into());
                }
            }
        }
        if snapshot_valid || rows_found {
            s.workspace.resources = resources.into_values().collect();
        }
        for r in &mut s.workspace.resources {
            if r.profiles.is_empty() {
                r.profiles.push(crate::workspace::ConnectionProfile {
                    ephemeral: false,
                    id: r.id,
                    host: String::new(),
                    port: 22,
                    username: String::new(),
                    credential_ref: None,
                    auth: crate::workspace::AuthMethod::default(),
                    jump_resource: None,
                    route: None,
                    transport_policy: Default::default(),
                });
                s.recovery = Some("recovered_profile".into());
            }
        }
        for v in &mut s.workspace.views {
            v.session_id = None;
            v.generation = v.generation.saturating_add(1);
        }
        let maximum = s
            .workspace
            .resources
            .iter()
            .map(|r| r.id)
            .chain(s.workspace.views.iter().map(|v| v.id))
            .chain(s.workspace.windows.iter().map(|w| w.id))
            .max()
            .unwrap_or_default();
        s.workspace.next_id = s.workspace.next_id.max(maximum.saturating_add(1));
        s.workspace.ensure_windows();
        s.version = 2;
        Ok(s)
    }
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let snapshot = self.persistent_snapshot();
        let mut db = Self::database(path).map_err(|e| e.to_string())?;
        let tx = db.transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT OR REPLACE INTO settings(id,value) VALUES(1,?1)",
            [serde_json::to_string(&snapshot).map_err(|e| e.to_string())?],
        )
        .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM resources", [])
            .map_err(|e| e.to_string())?;
        for r in &snapshot.workspace.resources {
            tx.execute(
                "INSERT INTO resources(id,value) VALUES(?1,?2)",
                rusqlite::params![r.id, serde_json::to_string(r).map_err(|e| e.to_string())?],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod multiwindow_tests {
    use super::*;
    use crate::workspace::{TargetMode, ToolKind};
    #[test]
    fn temporary_references_are_removed_from_every_window_and_dock() {
        let mut settings = Settings::demo_fixture();
        settings.workspace.ensure_windows();
        let other = settings.workspace.create_window(None);
        settings.workspace.resources[0].profiles[0].ephemeral = true;
        let view = settings
            .workspace
            .open_in(other, ToolKind::Sftp, TargetMode::Fixed(1));
        let record = settings
            .workspace
            .windows
            .iter_mut()
            .find(|w| w.id == other)
            .unwrap();
        record.active_resource = Some(1);
        record.dock_layout = Some(
            serde_json::json!({"panel_name":"helixlane.tool","info":{"panel":view},"children":[]}),
        );
        let follow = settings
            .workspace
            .open_in(other, ToolKind::Information, TargetMode::Follow);
        let page = &mut settings
            .workspace
            .views
            .iter_mut()
            .find(|v| v.id == follow)
            .unwrap()
            .page;
        page.resource = Some(1);
        page.remote_path = Some("temporary-page-must-not-persist".into());
        let saved = settings.persistent_snapshot();
        assert!(
            !serde_json::to_string(&saved)
                .unwrap()
                .contains("temporary-page-must-not-persist")
        );
        assert!(!saved.workspace.views.iter().any(|v| v.id == view));
        assert_eq!(saved.workspace.window(other).unwrap().active_resource, None);
        let layout = saved
            .workspace
            .window(other)
            .unwrap()
            .dock_layout
            .as_ref()
            .unwrap();
        assert_ne!(layout.get("panel_name").unwrap(), "helixlane.tool");
        assert!(
            !serde_json::to_string(&saved)
                .unwrap()
                .contains("host-1.example.invalid")
        );
    }
    #[test]
    fn window_geometry_and_top_extent_survive_serialization() {
        let mut settings = Settings::demo_fixture();
        settings.workspace.ensure_windows();
        let mut value = serde_json::to_value(&settings).unwrap();
        value["workspace"]["windows"][0]["bounds"] = serde_json::json!([10., 20., 1100., 700.]);
        value["workspace"]["windows"][0]["top_height"] = serde_json::json!(240.);
        let restored: Settings = serde_json::from_value(value.clone()).unwrap();
        let actual = serde_json::to_value(restored.persistent_snapshot()).unwrap();
        assert_eq!(
            actual["workspace"]["windows"][0]["bounds"],
            value["workspace"]["windows"][0]["bounds"]
        );
        assert_eq!(
            actual["workspace"]["windows"][0]["top_height"],
            value["workspace"]["windows"][0]["top_height"]
        );
    }
}

#[cfg(test)]
mod page_state_tests {
    use super::*;
    #[test]
    fn saved_file_navigation_restores_without_sessions_or_task_restart() {
        let mut settings = Settings::demo_fixture();
        let id = settings.workspace.open(
            crate::workspace::ToolKind::Sftp,
            crate::workspace::TargetMode::Fixed(1),
        );
        let mut value = serde_json::to_value(settings).unwrap();
        let page = serde_json::json!({"browser_hidden":[true,false],"browser_sort":["Size","Name"],"resource":1,"local_path":"/tmp/local","remote_path":"/var/log","local_scroll":[0.,-80.],"remote_scroll":[0.,-120.]});
        value["workspace"]["views"][0]["page"] = page.clone();
        let restored: Settings = serde_json::from_value(value).unwrap();
        let actual = serde_json::to_value(restored.persistent_snapshot()).unwrap();
        assert_eq!(actual["workspace"]["views"][0]["page"], page);
        assert_eq!(restored.workspace.views[0].id, id);
        assert_eq!(restored.workspace.views[0].session_id, None);
    }
}

#[cfg(test)]
mod production_tests {
    use super::*;
    #[test]
    fn fresh_workspace_is_empty_and_old_preferences_have_safe_defaults() {
        let settings = Settings::default();
        assert!(settings.workspace.resources.is_empty());
        assert!(settings.workspace.views.is_empty());
        assert_eq!(settings.workspace.active_resource, None);
        let mut old = serde_json::to_value(&settings).unwrap();
        old.as_object_mut().unwrap().remove("terminal_font");
        let restored: Settings = serde_json::from_value(old).unwrap();
        assert_eq!(restored.terminal_font, TerminalFont::default());
        assert_eq!(
            TerminalFont {
                family: "bad\nfont".into(),
                size: 65535
            }
            .sanitized(),
            TerminalFont {
                family: "monospace".into(),
                size: 28
            }
        );
        let page:crate::workspace::ViewPageState=serde_json::from_str(r#"{"resource":null,"local_path":null,"remote_path":null,"local_scroll":[0,0],"remote_scroll":[0,0]}"#).unwrap();
        assert_eq!(page.browser_hidden, [false; 2]);
        assert_eq!(page.browser_sort, [crate::sftp::browser::Sort::Name; 2]);
    }
}
