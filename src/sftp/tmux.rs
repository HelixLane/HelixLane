use crate::ssh::{ExecTask, Service, Session};
use std::sync::Arc;
pub fn quote(value: &str) -> Result<String, String> {
    if value.trim().is_empty() || value.chars().any(char::is_control) || value.len() > 512 {
        return Err("Invalid tmux name".into());
    }
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}
pub enum Operation {
    Detect,
    List,
    New(String),
    Rename { old: String, new: String },
}
pub fn command(op: Operation) -> Result<String, String> {
    Ok(match op {
        Operation::Detect => "command -v tmux && tmux -V".into(),
        Operation::List => "tmux list-sessions -F '#{session_name}'".into(),
        Operation::New(name) => format!("tmux new-session -d -s {}", quote(&name)?),
        Operation::Rename { old, new } => {
            format!("tmux rename-session -t {} {}", quote(&old)?, quote(&new)?)
        }
    })
}
pub fn run(session: Arc<Session>, op: Operation) -> Result<ExecTask, String> {
    Ok(Service::global().exec(session, command(op)?.into_bytes()))
}
/// A terminal instance is mandatory; resource identity alone never selects a terminal.
pub fn switch(terminal_view: u64, session: Arc<Session>, name: &str) -> Result<(), String> {
    if terminal_view == 0 || !session.terminal_running() {
        return Err("Select a running target terminal instance".into());
    }
    session.input(
        format!(
            "tmux switch-client -t {0} 2>/dev/null || tmux attach-session -t {0}\n",
            quote(name)?
        )
        .into_bytes(),
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_are_shell_quoted() {
        assert_eq!(quote("a'; touch /tmp/x").unwrap(), "'a'\\''; touch /tmp/x'");
        assert!(quote("a\nb").is_err());
    }
}

#[cfg(test)]
mod control_tests {
    use super::*;
    #[test]
    fn control_bytes_cannot_reach_target_terminal() {
        assert!(quote("name\u{3}bad").is_err());
        assert!(quote("").is_err());
    }
}

#[cfg(test)]
mod real_server_tests {
    use super::*;
    #[test]
    fn real_tmux_generated_create_list_rename_when_ipc_available() {
        if std::process::Command::new("tmux")
            .arg("-V")
            .output()
            .is_err()
        {
            assert!(
                std::env::var_os("HELIXLANE_REQUIRE_NATIVE_INTEGRATION").is_none(),
                "Required tmux executable unavailable"
            );
            eprintln!("SKIP real tmux server: tmux executable unavailable");
            return;
        }
        let root = std::env::temp_dir().join(format!(
            "helixlane-tmux-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let socket = root.join("tmux.sock");
        let run = |op| {
            let generated = command(op).unwrap();
            let args = generated.strip_prefix("tmux ").unwrap();
            std::process::Command::new("sh")
                .arg("-c")
                .arg(format!(
                    "tmux -S {} {}",
                    quote(&socket.display().to_string()).unwrap(),
                    args
                ))
                .output()
                .unwrap()
        };
        let original = "session'quoted; literal";
        let created = run(Operation::New(original.into()));
        if !created.status.success() {
            let error = String::from_utf8_lossy(&created.stderr);
            if error.contains("Operation not permitted") || error.contains("not supported") {
                assert!(
                    std::env::var_os("HELIXLANE_REQUIRE_NATIVE_INTEGRATION").is_none(),
                    "Required tmux IPC denied: {error}"
                );
                eprintln!("SKIP real tmux server: observed unsupported local IPC: {error}");
                let _ = std::fs::remove_dir_all(root);
                return;
            }
            panic!("tmux new-session failed: {error}")
        }
        let listed = run(Operation::List);
        assert!(listed.status.success());
        assert!(String::from_utf8_lossy(&listed.stdout).contains(original));
        let renamed = run(Operation::Rename {
            old: original.into(),
            new: "renamed-safe".into(),
        });
        assert!(
            renamed.status.success(),
            "{}",
            String::from_utf8_lossy(&renamed.stderr)
        );
        let listed = run(Operation::List);
        assert!(String::from_utf8_lossy(&listed.stdout).contains("renamed-safe"));
        let _ = std::process::Command::new("tmux")
            .args(["-S", &socket.display().to_string(), "kill-server"])
            .output();
        let _ = std::fs::remove_dir_all(root);
    }
}
