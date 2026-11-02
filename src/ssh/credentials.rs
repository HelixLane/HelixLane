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

#[cfg(test)]
thread_local! { static SECRET_DROPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
impl Secret {
    pub(crate) fn test_drop_count() -> usize {
        SECRET_DROPS.get()
    }
}
#[cfg(test)]
impl Drop for Secret {
    fn drop(&mut self) {
        SECRET_DROPS.set(SECRET_DROPS.get() + 1);
    }
}

impl<'de> serde::Deserialize<'de> for Secret {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SecretVisitor;
        impl<'de> serde::de::Visitor<'de> for SecretVisitor {
            type Value = Secret;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a secret string")
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Secret, E> {
                Ok(Secret::new(value.into()))
            }
            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Secret, E> {
                Ok(Secret::new(value))
            }
        }
        deserializer.deserialize_string(SecretVisitor)
    }
}
