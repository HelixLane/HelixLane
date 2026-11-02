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
        let mut s = Settings::default();
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
            ..Settings::default()
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
        let mut s = Settings::default();
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
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    pub version: u32,
    pub language: Language,
    pub theme: ThemePreference,
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
            version: 1,
            language: Language::default(),
            theme: ThemePreference::System,
            workspace: Workspace::default(),
            dock_layout: None,
            top_layout: None,
            recovery: None,
        }
    }
}
impl Settings {
    pub fn path() -> PathBuf {
        std::env::var_os("HELIXLANE_SETTINGS")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("XDG_CONFIG_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| {
                        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
                    })
                    .join("helixlane/settings.sqlite")
            })
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
                Ok(s) if s.version == 1 => {
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
                    id: r.id,
                    host: String::new(),
                    port: 22,
                    username: String::new(),
                    credential_ref: None,
                    auth: crate::workspace::AuthMethod::default(),
                    jump_resource: None,
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
            .max()
            .unwrap_or_default();
        s.workspace.next_id = s.workspace.next_id.max(maximum.saturating_add(1));
        Ok(s)
    }
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let mut db = Self::database(path).map_err(|e| e.to_string())?;
        let tx = db.transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT OR REPLACE INTO settings(id,value) VALUES(1,?1)",
            [serde_json::to_string(self).map_err(|e| e.to_string())?],
        )
        .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM resources", [])
            .map_err(|e| e.to_string())?;
        for r in &self.workspace.resources {
            tx.execute(
                "INSERT INTO resources(id,value) VALUES(?1,?2)",
                rusqlite::params![r.id, serde_json::to_string(r).map_err(|e| e.to_string())?],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }
}
