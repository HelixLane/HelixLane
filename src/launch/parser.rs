use super::*;
use std::collections::{HashMap, HashSet};
fn validate_encoding(raw: &str) -> Result<(), String> {
    if !valid_text(raw) {
        return Err(invalid());
    }
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len()
                || !bytes[i + 1].is_ascii_hexdigit()
                || !bytes[i + 2].is_ascii_hexdigit()
            {
                return Err(invalid());
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    Ok(())
}
fn decode_bytes(raw: &str) -> Result<Zeroizing<Vec<u8>>, String> {
    validate_encoding(raw)?;
    let mut bytes = Zeroizing::new(Vec::with_capacity(raw.len()));
    let raw = raw.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' {
            let high = (raw[i + 1] as char).to_digit(16).ok_or_else(invalid)?;
            let low = (raw[i + 2] as char).to_digit(16).ok_or_else(invalid)?;
            bytes.push(((high << 4) | low) as u8);
            i += 3;
        } else {
            bytes.push(raw[i]);
            i += 1;
        }
    }
    Ok(bytes)
}
fn decode_text(raw: &str) -> Result<String, String> {
    let bytes = decode_bytes(raw)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| invalid())?;
    if !valid_text(text) {
        return Err(invalid());
    }
    Ok(text.into())
}
fn decode_secret(raw: &str) -> Result<Secret, String> {
    let bytes = decode_bytes(raw)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| invalid())?;
    if !valid_text(text) {
        return Err(invalid());
    }
    Ok(Secret::new(text.into()))
}
fn decode_form(raw: &str) -> Result<String, String> {
    decode_text(&raw.replace('+', " "))
}
fn connection_url(raw: &str, legacy: bool) -> Result<OpenRequest, String> {
    // Never allocate a decoded full URL or hand credential-bearing text to Url.
    validate_encoding(raw)?;
    let (scheme, tail) = raw.split_once("://").ok_or_else(invalid)?;
    let protocol = match scheme {
        "ssh" if !legacy => Protocol::Ssh,
        "sftp" => Protocol::Sftp,
        _ => return Err(invalid()),
    };
    let split = tail.find(['/', '?', '#']).unwrap_or(tail.len());
    let (authority, suffix) = tail.split_at(split);
    if suffix.contains(['?', '#']) {
        return Err(invalid());
    }
    let (userinfo, host) = authority
        .rsplit_once('@')
        .map(|(u, h)| (Some(u), h))
        .unwrap_or((None, authority));
    let (username, password) = if let Some(info) = userinfo {
        if info.contains('@') {
            return Err(invalid());
        }
        match info.split_once(':') {
            Some((user, password)) => {
                if !legacy {
                    return Err("Connection URL contains unsupported or forbidden fields / 连接 URL 不允许包含密码或额外字段".into());
                }
                (decode_text(user)?, Some(decode_secret(password)?))
            }
            None => (decode_text(info)?, None),
        }
    } else {
        (String::new(), None)
    };
    let sanitized = format!("{scheme}://{host}{suffix}");
    let u = url::Url::parse(&sanitized).map_err(|_| invalid())?;
    let r = OpenRequest {
        target: Target::Connection(Connection {
            host: u
                .host_str()
                .ok_or_else(invalid)?
                .trim_start_matches('[')
                .trim_end_matches(']')
                .into(),
            username,
            port: u.port().unwrap_or(22),
            auth: AuthMethod::Password,
        }),
        protocol,
        mode: OpenMode::Activate,
        remote_path: if u.path().is_empty() {
            None
        } else {
            Some(decode_text(u.path())?)
        },
        name: None,
        secret: password,
    };
    r.validate()?;
    Ok(r)
}
fn own_uri(raw: &str) -> Result<OpenRequest, String> {
    validate_encoding(raw)?;
    let (stem, query) = raw.split_once('?').ok_or_else(invalid)?;
    if !(stem.eq_ignore_ascii_case("helixlane://open")
        || stem.eq_ignore_ascii_case("helixlane://open/"))
        || query.contains('#')
    {
        return Err(invalid());
    }
    let mut fields = HashMap::new();
    for item in query.split('&') {
        let (key, value) = item.split_once('=').ok_or_else(invalid)?;
        let key = decode_form(key)?;
        if ![
            "protocol", "host", "user", "port", "path", "name", "mode", "resource",
        ]
        .contains(&key.as_str())
            || fields.contains_key(&key)
        {
            return Err(invalid());
        }
        fields.insert(key, decode_form(value)?);
    }
    let protocol = match fields.get("protocol").map(String::as_str) {
        None | Some("ssh") => Protocol::Ssh,
        Some("sftp") => Protocol::Sftp,
        _ => return Err(invalid()),
    };
    let mode = match fields.get("mode").map(String::as_str) {
        None | Some("activate") => OpenMode::Activate,
        Some("new-view") => OpenMode::NewView,
        Some("new-window") => OpenMode::NewWindow,
        _ => return Err(invalid()),
    };
    let target = if let Some(id) = fields.get("resource") {
        if ["host", "user", "port"]
            .iter()
            .any(|k| fields.contains_key(*k))
        {
            return Err(invalid());
        }
        Target::Resource(id.parse().map_err(|_| invalid())?)
    } else {
        Target::Connection(Connection {
            host: fields.get("host").ok_or_else(invalid)?.clone(),
            username: fields.get("user").cloned().unwrap_or_default(),
            port: fields
                .get("port")
                .map(|v| v.parse::<u16>().map_err(|_| invalid()))
                .transpose()?
                .unwrap_or(22),
            auth: AuthMethod::Password,
        })
    };
    let r = OpenRequest {
        target,
        protocol,
        mode,
        remote_path: fields.remove("path"),
        name: fields.remove("name"),
        secret: None,
    };
    r.validate()?;
    Ok(r)
}
fn bare(raw: &str) -> Result<OpenRequest, String> {
    if raw.contains("://") {
        return connection_url(raw, false);
    }
    let (user, host) = raw.split_once('@').unwrap_or(("", raw));
    let r = OpenRequest {
        target: Target::Connection(Connection {
            host: host.into(),
            username: user.into(),
            port: 22,
            auth: AuthMethod::Password,
        }),
        ..OpenRequest::focus()
    };
    r.validate()?;
    Ok(r)
}
pub fn parse_native(args: &[String]) -> Result<Entry, String> {
    if args.is_empty() {
        return Ok(Entry::Gui);
    }
    if args.len() == 1 {
        match args[0].as_str() {
            "--help" | "-h" => return Ok(Entry::Help),
            "--version" | "-V" => return Ok(Entry::Version),
            "--launch-stdin" => return Ok(Entry::Stdin),
            s if s.starts_with("helixlane:") => return own_uri(s).map(Entry::Open),
            s if s.contains("://") => return connection_url(s, false).map(Entry::Open),
            _ => {}
        }
    }
    if args[0] != "open" {
        return Err(invalid());
    }
    let mut r = OpenRequest::focus();
    let mut target = None;
    let mut opts = HashMap::new();
    let mut seen = HashSet::new();
    let mut auth_none = false;
    let mut i = 1;
    while i < args.len() {
        let key = args[i].as_str();
        if key.starts_with('-') {
            let group = match key {
                "--ssh" | "--sftp" => "protocol",
                "--activate" | "--new-view" | "--new-window" => "mode",
                "--password" | "--key" | "--auth-none" => "auth",
                _ => key,
            };
            if !seen.insert(group) {
                return Err(invalid());
            }
            if [
                "--resource",
                "--user",
                "--port",
                "--password",
                "--key",
                "--path",
                "--name",
            ]
            .contains(&key)
            {
                i += 1;
                let v = args.get(i).ok_or_else(invalid)?;
                opts.insert(key, v.as_str());
            } else {
                match key {
                    "--ssh" => r.protocol = Protocol::Ssh,
                    "--sftp" => r.protocol = Protocol::Sftp,
                    "--activate" => r.mode = OpenMode::Activate,
                    "--new-view" => r.mode = OpenMode::NewView,
                    "--new-window" => r.mode = OpenMode::NewWindow,
                    "--auth-none" => auth_none = true,
                    _ => return Err(invalid()),
                }
            }
        } else if target.replace(key).is_some() {
            return Err(invalid());
        }
        i += 1;
    }
    if let Some(id) = opts.get("--resource") {
        if target.is_some()
            || ["--user", "--port", "auth"]
                .iter()
                .any(|k| seen.contains(k))
        {
            return Err(invalid());
        }
        r.target = Target::Resource(id.parse().map_err(|_| invalid())?);
    } else {
        let parsed = bare(target.ok_or_else(invalid)?)?;
        r.target = parsed.target;
        if !seen.contains("protocol") {
            r.protocol = parsed.protocol
        }
        r.remote_path = parsed.remote_path;
    }
    if let Target::Connection(c) = &mut r.target {
        if let Some(user) = opts.get("--user") {
            c.username = (*user).into()
        }
        if let Some(port) = opts.get("--port") {
            c.port = port.parse().map_err(|_| invalid())?
        }
        if let Some(password) = opts.get("--password") {
            r.secret = Some(Secret::new((*password).into()));
            c.auth = AuthMethod::Password;
        }
        if let Some(path) = opts.get("--key") {
            c.auth = AuthMethod::PrivateKey {
                path: (*path).into(),
            };
        }
        if auth_none {
            c.auth = AuthMethod::None;
        }
    }
    if let Some(path) = opts.get("--path") {
        r.remote_path = Some((*path).into())
    }
    r.name = opts.get("--name").map(|s| (*s).into());
    r.validate()?;
    Ok(Entry::Open(r))
}
pub fn parse_putty(args: &[String]) -> Result<Entry, String> {
    if args.len() == 1 && ["--help", "--version"].contains(&args[0].as_str()) {
        return parse_native(args);
    }
    let mut native = Zeroizing::new(vec!["open".to_string()]);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-ssh" => native.push("--ssh".into()),
            "-P" | "-l" | "-pw" => {
                native.push(
                    match args[i].as_str() {
                        "-P" => "--port",
                        "-l" => "--user",
                        _ => "--password",
                    }
                    .into(),
                );
                i += 1;
                native.push(args.get(i).ok_or_else(invalid)?.clone());
            }
            v if !v.starts_with('-') && !v.contains("://") => native.push(v.into()),
            _ => return Err("Unsupported PuTTY option / 不支持的 PuTTY 参数；参数值已隐藏".into()),
        }
        i += 1;
    }
    parse_native(&native)
}
pub fn parse_winscp(args: &[String]) -> Result<Entry, String> {
    if args.len() == 1 && ["--help", "--version"].contains(&args[0].as_str()) {
        return parse_native(args);
    }
    let mut r = None;
    let mut name = None;
    let mut new = false;
    for a in args {
        if let Some(value) = a.strip_prefix("/sessionname=") {
            if name.replace(value.to_string()).is_some() {
                return Err(invalid());
            }
        } else if a == "/newinstance" {
            if new {
                return Err(invalid());
            }
            new = true;
        } else if a.starts_with("sftp://") && r.is_none() {
            r = Some(connection_url(a, true)?);
        } else {
            return Err("Unsupported WinSCP argument / 不支持的 WinSCP 参数；参数值已隐藏".into());
        }
    }
    let mut r = r.ok_or_else(invalid)?;
    r.name = name;
    if new {
        r.mode = OpenMode::NewWindow
    }
    r.validate()?;
    Ok(Entry::Open(r))
}
