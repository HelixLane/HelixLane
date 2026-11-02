use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verification {
    Trusted,
    Unknown,
    Changed(String),
}
#[derive(Default, Serialize, Deserialize)]
pub struct HostKeys(BTreeMap<String, String>);
impl HostKeys {
    pub fn verify(&self, host: &str, port: u16, fingerprint: &str) -> Verification {
        match self.0.get(&format!("[{host}]:{port}")) {
            None => Verification::Unknown,
            Some(old) if old == fingerprint => Verification::Trusted,
            Some(old) => Verification::Changed(old.clone()),
        }
    }
    pub fn trust(&mut self, host: &str, port: u16, fingerprint: String) {
        self.0.insert(format!("[{host}]:{port}"), fingerprint);
    }
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| "Host-key store is corrupt; repair it before connecting".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.to_string()),
        }
    }
    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let temp = path.with_extension("pending");
        std::fs::write(
            &temp,
            serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        std::fs::rename(temp, path).map_err(|e| e.to_string())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn first_contact_requires_consent_and_changes_never_match() {
        let mut keys = HostKeys::default();
        assert_eq!(keys.verify("host", 22, "key-a"), Verification::Unknown);
        keys.trust("host", 22, "key-a".into());
        assert_eq!(keys.verify("host", 22, "key-a"), Verification::Trusted);
        assert_eq!(
            keys.verify("host", 22, "key-b"),
            Verification::Changed("key-a".into())
        );
        assert_eq!(keys.verify("host", 2222, "key-a"), Verification::Unknown);
    }
}
