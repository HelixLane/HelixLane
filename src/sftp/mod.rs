//! Actual SFTP subsystem operations on an explicitly trusted, authenticated SSH transport.
use crate::ssh::Session;
use russh_sftp::{
    client::SftpSession,
    protocol::{FileAttributes, OpenFlags},
};
use sha2::{Digest, Sha256};
use std::{pin::Pin, sync::Arc};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
pub mod browser;
pub mod editor;
pub mod tmux;
#[derive(Clone)]
pub enum Endpoint {
    Local,
    Remote {
        transport: Arc<Session>,
        client: Arc<SftpSession>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Kind {
    File,
    Directory,
    Symlink,
    Other,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub kind: Kind,
    pub size: u64,
    pub modified: u64,
    pub permissions: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Fingerprint {
    pub entry: Entry,
    pub digest: Option<Vec<u8>>,
}
pub type Reader = Pin<Box<dyn AsyncRead + Send>>;
pub type Writer = Pin<Box<dyn AsyncWrite + Send>>;
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
pub fn join(parent: &str, name: &str) -> String {
    format!("{}/{}", parent.trim_end_matches('/'), name)
}
impl Endpoint {
    pub async fn connect(transport: Arc<Session>) -> Result<Self, String> {
        let channel = transport.open_channel().await?;
        channel.request_subsystem(true, "sftp").await.map_err(err)?;
        let client = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            SftpSession::new(channel.into_stream()),
        )
        .await
        .map_err(|_| "SFTP subsystem timed out".to_string())?
        .map_err(err)?;
        Ok(Self::Remote {
            transport,
            client: Arc::new(client),
        })
    }
    pub fn basename<'a>(&self, path: &'a str) -> Result<&'a str, String> {
        source_leaf(path, matches!(self, Self::Local) && cfg!(windows))
    }
    pub fn checked_child_path(&self, parent: &str, name: &str) -> Result<String, String> {
        validate_child_name(name, matches!(self, Self::Local) && cfg!(windows))?;
        Ok(self.child_path(parent, name))
    }
    /// Validate the destination leaf before any target canonicalization/stat.
    pub fn destination_path(
        &self,
        path: &str,
        target: &Endpoint,
        parent: &str,
    ) -> Result<String, String> {
        let name = transfer_leaf(
            path,
            matches!(self, Self::Local) && cfg!(windows),
            matches!(target, Self::Local) && cfg!(windows),
        )?;
        Ok(target.child_path(parent, name))
    }
    pub fn child_path(&self, parent: &str, name: &str) -> String {
        match self {
            Self::Local => std::path::Path::new(parent)
                .join(name)
                .display()
                .to_string(),
            Self::Remote { .. } => join(parent, name),
        }
    }
    pub fn path_within(&self, path: &str, root: &str) -> bool {
        match self {
            Self::Local => std::path::Path::new(path).starts_with(root),
            Self::Remote { .. } => {
                path == root || path.starts_with(&format!("{}/", root.trim_end_matches('/')))
            }
        }
    }
    pub async fn canonicalize(&self, path: &str) -> Result<String, String> {
        match self {
            Self::Local => Ok(tokio::fs::canonicalize(path)
                .await
                .map_err(err)?
                .display()
                .to_string()),
            Self::Remote { client, .. } => client.canonicalize(path).await.map_err(err),
        }
    }
    /// Resolve existing parents, while preserving a symlink leaf for no-follow handling.
    pub fn absolute<'a>(
        &'a self,
        path: &'a str,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            if self
                .stat(path)
                .await?
                .is_some_and(|e| e.kind != Kind::Symlink)
            {
                return self.canonicalize(path).await;
            }
            match self {
                Self::Local => {
                    let p = std::path::Path::new(path);
                    let name = p.file_name().ok_or("Invalid path")?.to_string_lossy();
                    let parent = p
                        .parent()
                        .filter(|p| !p.as_os_str().is_empty())
                        .unwrap_or(std::path::Path::new("."));
                    let parent = self.absolute(&parent.display().to_string()).await?;
                    Ok(std::path::Path::new(&parent)
                        .join(name.as_ref())
                        .display()
                        .to_string())
                }
                Self::Remote { .. } => {
                    let path = path.trim_end_matches('/');
                    let (parent, name) = path.rsplit_once('/').unwrap_or((".", path));
                    validate_child_name(name, false)?;
                    let parent = self
                        .absolute(if parent.is_empty() { "/" } else { parent })
                        .await?;
                    Ok(join(&parent, name))
                }
            }
        })
    }
    pub fn label(&self) -> String {
        match self {
            Self::Local => "local".into(),
            Self::Remote { transport, .. } => format!(
                "{}@{}:{} · SSH #{}",
                transport.profile.username,
                transport.profile.host,
                transport.profile.port,
                transport.id
            ),
        }
    }
    pub async fn stat(&self, path: &str) -> Result<Option<Entry>, String> {
        match self {
            Self::Local => match tokio::fs::symlink_metadata(path).await {
                Ok(m) => {
                    #[cfg(unix)]
                    let permissions = {
                        use std::os::unix::fs::MetadataExt;
                        m.mode()
                    };
                    #[cfg(not(unix))]
                    let permissions = 0;
                    Ok(Some(Entry {
                        name: std::path::Path::new(path)
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into(),
                        path: path.into(),
                        kind: if m.is_symlink() {
                            Kind::Symlink
                        } else if m.is_dir() {
                            Kind::Directory
                        } else if m.is_file() {
                            Kind::File
                        } else {
                            Kind::Other
                        },
                        size: m.len(),
                        modified: m
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|t| t.as_secs())
                            .unwrap_or(0),
                        permissions,
                    }))
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(err(e)),
            },
            Self::Remote { client, .. } => match client.symlink_metadata(path).await {
                Ok(m) => Ok(Some(remote_entry(path, m))),
                Err(russh_sftp::client::error::Error::Status(s))
                    if s.status_code == russh_sftp::protocol::StatusCode::NoSuchFile =>
                {
                    Ok(None)
                }
                Err(e) => Err(err(e)),
            },
        }
    }
    pub async fn list(&self, path: &str) -> Result<Vec<Entry>, String> {
        let mut entries = vec![];
        match self {
            Self::Local => {
                let mut dir = tokio::fs::read_dir(path).await.map_err(err)?;
                while let Some(e) = dir.next_entry().await.map_err(err)? {
                    let p = e.path().to_string_lossy().to_string();
                    if let Some(e) = self.stat(&p).await? {
                        entries.push(e);
                    }
                }
            }
            Self::Remote { client, .. } => {
                for e in client.read_dir(path).await.map_err(err)? {
                    validate_child_name(&e.file_name(), false)?;
                    entries.push(remote_entry(&e.path(), e.metadata()));
                }
            }
        }
        Ok(entries)
    }
    pub async fn mkdir(&self, path: &str) -> Result<(), String> {
        match self {
            Self::Local => tokio::fs::create_dir(path).await.map_err(err),
            Self::Remote { client, .. } => client.create_dir(path).await.map_err(err),
        }
    }
    pub async fn rename(&self, from: &str, to: &str) -> Result<(), String> {
        match self {
            Self::Local => tokio::fs::rename(from, to).await.map_err(err),
            Self::Remote { client, .. } => client.rename(from, to).await.map_err(err),
        }
    }
    pub async fn remove(&self, path: &str) -> Result<(), String> {
        let Some(m) = self.stat(path).await? else {
            return Ok(());
        };
        match self {
            Self::Local => {
                if m.kind == Kind::Directory {
                    tokio::fs::remove_dir(path).await.map_err(err)
                } else {
                    tokio::fs::remove_file(path).await.map_err(err)
                }
            }
            Self::Remote { client, .. } => {
                if m.kind == Kind::Directory {
                    client.remove_dir(path).await.map_err(err)
                } else {
                    client.remove_file(path).await.map_err(err)
                }
            }
        }
    }
    pub async fn chmod(&self, path: &str, mode: u32) -> Result<(), String> {
        if mode > 0o7777 {
            return Err("Invalid permissions".into());
        }
        if self
            .stat(path)
            .await?
            .is_some_and(|e| e.kind == Kind::Symlink)
        {
            return Err("Refusing to follow symlink".into());
        }
        match self {
            Self::Local => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
                        .await
                        .map_err(err)
                }
                #[cfg(not(unix))]
                {
                    Err("Unix permissions unavailable".into())
                }
            }
            Self::Remote { client, .. } => client
                .set_metadata(
                    path,
                    FileAttributes {
                        permissions: Some(mode),
                        ..FileAttributes::empty()
                    },
                )
                .await
                .map_err(err),
        }
    }
    pub async fn reader(&self, path: &str) -> Result<Reader, String> {
        if self.stat(path).await?.is_none_or(|e| e.kind != Kind::File) {
            return Err("Refusing to follow symlink or non-file".into());
        }
        match self {
            Self::Local => Ok(Box::pin(tokio::fs::File::open(path).await.map_err(err)?)),
            Self::Remote { client, .. } => Ok(Box::pin(client.open(path).await.map_err(err)?)),
        }
    }
    pub async fn create_new(&self, path: &str) -> Result<Writer, String> {
        match self {
            Self::Local => {
                let mut options = tokio::fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    options.mode(0o600);
                }
                Ok(Box::pin(options.open(path).await.map_err(err)?))
            }
            Self::Remote { client, .. } => Ok(Box::pin(
                client
                    .open_with_flags_and_attributes(
                        path,
                        OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE,
                        FileAttributes {
                            permissions: Some(0o600),
                            ..FileAttributes::empty()
                        },
                    )
                    .await
                    .map_err(err)?,
            )),
        }
    }
    pub async fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        let mut data = vec![];
        self.reader(path)
            .await?
            .read_to_end(&mut data)
            .await
            .map_err(err)?;
        Ok(data)
    }
}
fn remote_entry(path: &str, m: FileAttributes) -> Entry {
    let kind = m.file_type();
    Entry {
        name: path.rsplit('/').next().unwrap_or(path).into(),
        path: path.into(),
        kind: if kind.is_symlink() {
            Kind::Symlink
        } else if kind.is_dir() {
            Kind::Directory
        } else if kind.is_file() {
            Kind::File
        } else {
            Kind::Other
        },
        size: m.size.unwrap_or(0),
        modified: m.mtime.unwrap_or(0) as u64,
        permissions: m.permissions.unwrap_or(0),
    }
}
impl Fingerprint {
    pub async fn capture(endpoint: &Endpoint, path: &str) -> Result<Option<Self>, String> {
        let Some(entry) = endpoint.stat(path).await? else {
            return Ok(None);
        };
        let digest = if entry.kind == Kind::File {
            let mut reader = endpoint.reader(path).await?;
            let mut hash = Sha256::new();
            let mut buf = vec![0; 65536];
            loop {
                let n = reader.read(&mut buf).await.map_err(err)?;
                if n == 0 {
                    break;
                }
                hash.update(&buf[..n]);
            }
            Some(hash.finalize().to_vec())
        } else {
            None
        };
        Ok(Some(Self { entry, digest }))
    }
}
#[cfg(test)]
mod tests;

pub fn local_home() -> String {
    #[cfg(windows)]
    let keys = ["USERPROFILE", "HOME"];
    #[cfg(not(windows))]
    let keys = ["HOME", "USERPROFILE"];
    keys.iter()
        .find_map(|key| std::env::var_os(key).filter(|v| !v.is_empty()))
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| std::path::PathBuf::from("/"))
        .display()
        .to_string()
}
pub fn validate_child_name(name: &str, windows: bool) -> Result<(), String> {
    if name.is_empty() || matches!(name, "." | "..") || name.contains('/') || name.contains('\0') {
        return Err("Unsafe remote child name".into());
    }
    if windows {
        let base = name
            .split('.')
            .next()
            .unwrap_or("")
            .trim_end()
            .to_ascii_uppercase();
        if name
            .chars()
            .any(|c| c.is_control() || matches!(c, '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*'))
            || name.ends_with(['.', ' '])
            || matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (base.len() == 4
                && (base.starts_with("COM") || base.starts_with("LPT"))
                && base.as_bytes()[3].is_ascii_digit())
        {
            return Err("Unsafe Windows child name".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod path_tests {
    use super::*;
    #[test]
    fn child_names_cannot_escape_download_root() {
        for name in ["..", "a/b", "/etc/passwd", ""] {
            assert!(validate_child_name(name, false).is_err());
        }
        for name in [
            "..\\escape",
            "C:escape",
            "CON.txt",
            "file.",
            "file ",
            "LPT1",
            "bad:name",
        ] {
            assert!(validate_child_name(name, true).is_err());
        }
        assert!(validate_child_name("normal-file.txt", true).is_ok());
        assert!(validate_child_name("name\\with-backslash", false).is_ok());
    }
    #[test]
    fn remote_parent_uses_posix_separators() {
        let mut b = browser::Browser::remote("/home/a\\b/c".into());
        b.up();
        assert_eq!(b.path, "/home/a\\b");
    }
}

fn source_leaf(path: &str, windows: bool) -> Result<&str, String> {
    let name = if windows {
        path.rsplit(['/', '\\']).next().unwrap_or("")
    } else {
        path.rsplit('/').next().unwrap_or("")
    };
    let name = if windows
        && name.as_bytes().get(1) == Some(&b':')
        && name.as_bytes()[0].is_ascii_alphabetic()
    {
        &name[2..]
    } else {
        name
    };
    validate_child_name(name, false)?;
    Ok(name)
}
pub fn transfer_leaf(
    path: &str,
    source_windows: bool,
    target_windows: bool,
) -> Result<&str, String> {
    let name = source_leaf(path, source_windows)?;
    validate_child_name(name, target_windows)?;
    Ok(name)
}
#[cfg(test)]
mod transfer_path_tests {
    use super::*;
    #[test]
    fn top_level_upload_extracts_windows_basename_and_download_rejects_aliases() {
        assert_eq!(
            transfer_leaf(r"C:\Users\me\file.txt", true, false).unwrap(),
            "file.txt"
        );
        assert_eq!(
            transfer_leaf(r"/home/name\with-backslash", false, false).unwrap(),
            r"name\with-backslash"
        );
        for path in [
            r"/home/..\outside.txt",
            "/home/CON.txt",
            "/home/C:escape",
            "/home/LPT1.txt",
            "/home/trailing.",
            "/home/trailing ",
        ] {
            assert!(
                transfer_leaf(path, false, true).is_err(),
                "accepted unsafe top-level destination leaf: {path}"
            );
        }
    }
    #[test]
    fn native_destination_path_is_built_from_validated_leaf() {
        let source = Endpoint::Local;
        assert_eq!(
            source
                .destination_path("/source/file.txt", &Endpoint::Local, "/destination")
                .unwrap(),
            std::path::Path::new("/destination")
                .join("file.txt")
                .display()
                .to_string()
        );
        assert!(
            source
                .destination_path("/source/..", &Endpoint::Local, "/destination")
                .is_err()
        );
    }
}
