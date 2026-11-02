//! Secrets never participate in settings serialization or diagnostic formatting.
use zeroize::Zeroizing;
pub struct Secret(Zeroizing<String>);
impl Secret {
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}
pub fn save(reference: &str, secret: &Secret) -> Result<(), String> {
    keyring::Entry::new("helixlane.ssh", reference)
        .and_then(|entry| entry.set_password(secret.expose()))
        .map_err(|_| "System keychain unavailable; credential was not saved".into())
}
pub fn load(reference: &str) -> Result<Secret, String> {
    keyring::Entry::new("helixlane.ssh", reference)
        .and_then(|entry| entry.get_password())
        .map(Secret::new)
        .map_err(|_| "System keychain unavailable or credential missing".into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostic_format_never_contains_credentials() {
        assert!(!format!("{:?}", Secret::new("hunter2".into())).contains("hunter2"));
    }
}
