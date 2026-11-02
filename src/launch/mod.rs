//! A single validated launch contract shared by native CLI, URI and compatibility launchers.
use crate::{
    ssh::credentials::Secret,
    workspace::{AuthMethod, ConnectionProfile, Resource, TargetMode, ToolKind, Workspace},
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;
pub mod ipc;
mod parser;
pub use parser::{parse_native, parse_putty, parse_winscp};
pub const MAX_FRAME: usize = 64 * 1024;
pub const HELP: &str = "HelixLane / 连接工作台\n\nUsage / 用法:\n  helixlane open <host|user@host|ssh://...|sftp://...> [options]\n  helixlane open --resource ID [--ssh|--sftp]\n  helixlane helixlane://open?protocol=sftp&host=HOST&user=USER&path=PATH\nOptions / 选项: --ssh --sftp --user USER --port PORT --password PASSWORD\n  --key FILE --auth-none --path PATH --name NAME\n  --activate --new-view --new-window --help --version\nTemporary credentials remain in memory. URI passwords are forbidden.\n临时凭据仅保留在内存；URI 不允许包含密码。\n";
pub const PUTTY_HELP: &str = "HelixLane PuTTY launcher / PuTTY 兼容启动器\nUsage / 用法: helixlane-putty [-ssh] [-P port] [-l user] [-pw password] [user@]host\n--help --version\nUnsupported / 不支持: -i, -load, remote commands, Telnet, serial.\nPasswords remain in memory / 密码仅保留在内存。\n";
pub const WINSCP_HELP: &str = "HelixLane WinSCP launcher / WinSCP 兼容启动器\nUsage / 用法: helixlane-winscp sftp://user:encoded-password@host:port/path [/sessionname=name] [/newinstance]\n--help --version\nPercent-encode credentials and paths / 凭据和路径使用百分号编码。\nUnsupported / 不支持: /script, /command, saved session import.\n";
pub(crate) fn invalid() -> String {
    "Invalid launch request / 连接参数无效；参数值已隐藏".into()
}
pub fn diagnostic(message: &str) -> String {
    if message
        .chars()
        .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
    {
        message.into()
    } else {
        format!("{message} / 启动未完成；请使用 --help 查看用法。")
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    #[default]
    Ssh,
    Sftp,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OpenMode {
    #[default]
    Activate,
    NewView,
    NewWindow,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    pub host: String,
    pub username: String,
    pub port: u16,
    pub auth: AuthMethod,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    Focus,
    Resource(u64),
    Connection(Connection),
}
#[derive(Debug)]
pub struct OpenRequest {
    pub target: Target,
    pub protocol: Protocol,
    pub mode: OpenMode,
    pub remote_path: Option<String>,
    pub name: Option<String>,
    pub secret: Option<Secret>,
}
impl Clone for OpenRequest {
    fn clone(&self) -> Self {
        Self {
            target: self.target.clone(),
            protocol: self.protocol,
            mode: self.mode,
            remote_path: self.remote_path.clone(),
            name: self.name.clone(),
            secret: self.secret.as_ref().map(|s| Secret::new(s.expose().into())),
        }
    }
}
#[derive(Debug)]
pub enum Entry {
    Gui,
    Help,
    Version,
    Stdin,
    Open(OpenRequest),
}
#[derive(Debug, PartialEq, Eq)]
pub struct Opened {
    pub view_id: u64,
    pub resource_id: u64,
}
pub(crate) fn valid_text(value: &str) -> bool {
    value.len() <= MAX_FRAME && !value.chars().any(char::is_control)
}
pub(crate) fn valid_host(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && valid_text(value)
        && if value.contains(':') {
            value
                .trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<std::net::Ipv6Addr>()
                .is_ok()
        } else {
            value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
        }
}
impl OpenRequest {
    pub fn focus() -> Self {
        Self {
            target: Target::Focus,
            protocol: Protocol::Ssh,
            mode: OpenMode::Activate,
            remote_path: None,
            name: None,
            secret: None,
        }
    }
    pub fn connection(&self) -> Option<&Connection> {
        if let Target::Connection(c) = &self.target {
            Some(c)
        } else {
            None
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.remote_path.as_ref().is_some_and(|s| !valid_text(s))
            || self
                .name
                .as_ref()
                .is_some_and(|s| s.is_empty() || !valid_text(s))
            || self
                .secret
                .as_ref()
                .is_some_and(|s| !valid_text(s.expose()))
        {
            return Err(invalid());
        }
        match &self.target {
            Target::Focus => {
                if self.mode != OpenMode::Activate
                    || self.remote_path.is_some()
                    || self.name.is_some()
                    || self.secret.is_some()
                {
                    return Err(invalid());
                }
            }
            Target::Resource(id) => {
                if *id == 0 || self.secret.is_some() {
                    return Err(invalid());
                }
            }
            Target::Connection(c) => {
                if !valid_host(&c.host)
                    || !valid_text(&c.username)
                    || c.username.contains(['@', '/', '\\'])
                    || c.port == 0
                {
                    return Err(invalid());
                }
                if matches!(c.auth, AuthMethod::Agent | AuthMethod::KeyboardInteractive) {
                    return Err(invalid());
                }
                if let AuthMethod::PrivateKey { path } = &c.auth
                    && (path.is_empty() || !valid_text(path))
                {
                    return Err(invalid());
                }
                if self.secret.is_some() && !matches!(c.auth, AuthMethod::Password) {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }
    pub fn apply(&self, w: &mut Workspace) -> Result<Opened, String> {
        self.apply_in(w, 1)
    }
    pub fn apply_in(&self, w: &mut Workspace, window: u64) -> Result<Opened, String> {
        self.validate()?;
        if let Target::Resource(id) = self.target
            && !w.resources.iter().any(|r| r.id == id)
        {
            return Err("Saved resource unavailable / 已保存资源不可用".into());
        }
        let window = if self.mode == OpenMode::NewWindow {
            w.create_window(None)
        } else {
            window
        };
        let rid = match &self.target {
            Target::Focus => return Err(invalid()),
            Target::Resource(id) => {
                if !w.resources.iter().any(|r| r.id == *id) {
                    return Err("Saved resource unavailable / 已保存资源不可用".into());
                }
                *id
            }
            Target::Connection(c) => {
                if let Some(r) = w.resources.iter().find(|r| {
                    r.profiles.first().is_some_and(|p| {
                        p.ephemeral
                            && p.host == c.host
                            && p.username == c.username
                            && p.port == c.port
                            && p.auth == c.auth
                    })
                }) {
                    r.id
                } else {
                    let id = w.next_id;
                    w.next_id += 1;
                    w.resources.push(Resource {
                        id,
                        name: self.name.clone().unwrap_or_else(|| c.host.clone()),
                        group: "Temporary / 临时".into(),
                        tags: vec![],
                        notes: String::new(),
                        profiles: vec![ConnectionProfile {
                            ephemeral: true,
                            id,
                            host: c.host.clone(),
                            port: c.port,
                            username: c.username.clone(),
                            credential_ref: None,
                            auth: c.auth.clone(),
                            jump_resource: None,
                            route: None,
                            transport_policy: Default::default(),
                        }],
                    });
                    id
                }
            }
        };
        w.activate_resource_in(window, rid);
        let tool = match self.protocol {
            Protocol::Ssh => ToolKind::Terminal,
            Protocol::Sftp => ToolKind::Sftp,
        };
        if self.mode == OpenMode::Activate
            && let Some(id) = w
                .views
                .iter()
                .find(|v| v.window_id == window && v.tool == tool && w.target(v.id) == Some(rid))
                .map(|v| v.id)
        {
            w.layout.focus(id);
            return Ok(Opened {
                view_id: id,
                resource_id: rid,
            });
        }
        let id = w.open_in(window, tool, TargetMode::Fixed(rid));
        // Only newly-created external views are temporary. A reused saved Follow view
        // retains its identity, session and persistence even when following a temp host.
        if let Some(view) = w.views.iter_mut().find(|v| v.id == id) {
            view.ephemeral = matches!(self.target, Target::Connection(_));
        }
        Ok(Opened {
            view_id: id,
            resource_id: rid,
        })
    }
}
#[derive(Serialize)]
struct WireRef<'a> {
    version: u8,
    target: &'a Target,
    protocol: Protocol,
    mode: OpenMode,
    remote_path: &'a Option<String>,
    name: &'a Option<String>,
    password: Option<&'a str>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    version: u8,
    target: Target,
    protocol: Protocol,
    mode: OpenMode,
    remote_path: Option<String>,
    name: Option<String>,
    password: Option<Secret>,
}
pub fn encode(r: &OpenRequest) -> Result<Zeroizing<Vec<u8>>, String> {
    r.validate()?;
    let bytes = Zeroizing::new(
        serde_json::to_vec(&WireRef {
            version: 1,
            target: &r.target,
            protocol: r.protocol,
            mode: r.mode,
            remote_path: &r.remote_path,
            name: &r.name,
            password: r.secret.as_ref().map(Secret::expose),
        })
        .map_err(|_| invalid())?,
    );
    if bytes.len() > MAX_FRAME {
        return Err(invalid());
    }
    Ok(bytes)
}
pub fn decode(bytes: &[u8]) -> Result<OpenRequest, String> {
    if bytes.len() > MAX_FRAME {
        return Err(invalid());
    }
    let mut w: Wire = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    if w.version != 1 {
        return Err(invalid());
    }
    let r = OpenRequest {
        target: w.target.clone(),
        protocol: w.protocol,
        mode: w.mode,
        remote_path: w.remote_path.take(),
        name: w.name.take(),
        secret: w.password.take(),
    };
    r.validate()?;
    Ok(r)
}
pub fn os_arguments() -> Result<Zeroizing<Vec<String>>, String> {
    collect_os_arguments(std::env::args_os().skip(1))
}
fn collect_os_arguments(
    args: impl IntoIterator<Item = std::ffi::OsString>,
) -> Result<Zeroizing<Vec<String>>, String> {
    let mut values = Zeroizing::new(Vec::new());
    for arg in args {
        let bytes = Zeroizing::new(arg.into_encoded_bytes());
        let value = std::str::from_utf8(&bytes).map_err(|_| invalid())?;
        values.push(value.to_owned());
    }
    Ok(values)
}
/// Launch the sibling main executable without copying credentials to child argv.
pub fn run_proxy(parse: fn(&[String]) -> Result<Entry, String>, help: &str) -> Result<(), String> {
    let args = os_arguments()?;
    let entry = parse(&args)?;
    drop(args);
    let request = match entry {
        Entry::Help => {
            print!("{help}");
            return Ok(());
        }
        Entry::Version => {
            println!("HelixLane {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Entry::Open(r) => r,
        _ => return Err(invalid()),
    };
    if ipc::forward(&request)? {
        return Ok(());
    }
    let binary = std::env::current_exe()
        .map_err(|_| invalid())?
        .with_file_name(if cfg!(windows) {
            "helixlane.exe"
        } else {
            "helixlane"
        });
    let bytes = encode(&request)?;
    use std::io::Write;
    let mut child = std::process::Command::new(binary)
        .arg("--launch-stdin")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|_| "Unable to start HelixLane / 无法启动连接工作台".to_string())?;
    child
        .stdin
        .take()
        .ok_or_else(invalid)?
        .write_all(&bytes)
        .map_err(|_| invalid())?;
    Ok(())
}

/// Windows desktop/proxy binaries never allocate a console when launched by Explorer.
/// A calling terminal may be attached; inherited redirected streams remain intact.
pub fn attach_parent_console() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::{Foundation::INVALID_HANDLE_VALUE, System::Console::*};
        let ids = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE];
        let handles = ids.map(|id| GetStdHandle(id));
        AttachConsole(ATTACH_PARENT_PROCESS);
        for (id, handle) in ids.into_iter().zip(handles) {
            if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                SetStdHandle(id, handle);
            }
        }
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    #[test]
    fn partially_deserialized_password_is_already_secret_owned() {
        for bytes in [
            br#"{"password":"private-canary","unknown":true}"#.as_slice(),
            br#"{"password":"private-canary","password":"duplicate"}"#.as_slice(),
            br#"{"password":"private-canary","version":"#.as_slice(),
        ] {
            let before = Secret::test_drop_count();
            let error = decode(bytes).unwrap_err();
            assert!(!error.contains("private-canary"));
            assert_eq!(
                Secret::test_drop_count(),
                before + 1,
                "parsed password must acquire zeroizing ownership before later fields fail"
            );
        }
    }
    #[test]
    fn parsed_legacy_password_is_dropped_when_a_later_url_field_fails() {
        for url in [
            "sftp://alice:private-canary@host/%00",
            "sftp://alice:private-canary@bad host/a",
        ] {
            let before = Secret::test_drop_count();
            let error = parse_winscp(&[url.into()]).unwrap_err();
            assert!(!error.contains("private-canary"));
            assert_eq!(Secret::test_drop_count(), before + 1);
        }
        let error = parse_putty(&["-pw".into(), "private-canary".into(), "-unsupported".into()])
            .unwrap_err();
        assert!(!error.contains("private-canary"));
    }
    #[test]
    fn os_argument_collection_is_protected_before_unicode_validation() {
        let protected: Zeroizing<Vec<String>> =
            collect_os_arguments([std::ffi::OsString::from("private-canary")]).unwrap();
        assert_eq!(protected[0], "private-canary");
        drop(protected);
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let error = collect_os_arguments([
                std::ffi::OsString::from("private-canary"),
                std::ffi::OsString::from_vec(vec![255]),
            ])
            .unwrap_err();
            assert!(!error.contains("private-canary"));
        }
    }
}
