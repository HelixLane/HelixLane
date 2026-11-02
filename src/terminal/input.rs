pub fn encode(
    key: &str,
    character: Option<&str>,
    control: bool,
    alt: bool,
    shift: bool,
    application: bool,
) -> Option<Vec<u8>> {
    let mut bytes = if control && key.len() == 1 {
        let byte = key.as_bytes()[0].to_ascii_uppercase();
        if (b'@'..=b'_').contains(&byte) {
            vec![byte & 0x1f]
        } else if byte == b' ' {
            vec![0]
        } else {
            return None;
        }
    } else {
        let text = match key {
            "enter" => "\r",
            "backspace" => "\x7f",
            "escape" => "\x1b",
            "tab" if shift => "\x1b[Z",
            "tab" => "\t",
            "up" if application => "\x1bOA",
            "down" if application => "\x1bOB",
            "right" if application => "\x1bOC",
            "left" if application => "\x1bOD",
            "up" => "\x1b[A",
            "down" => "\x1b[B",
            "right" => "\x1b[C",
            "left" => "\x1b[D",
            "home" => "\x1b[H",
            "end" => "\x1b[F",
            "delete" => "\x1b[3~",
            "insert" => "\x1b[2~",
            "pageup" => "\x1b[5~",
            "pagedown" => "\x1b[6~",
            "f1" => "\x1bOP",
            "f2" => "\x1bOQ",
            "f3" => "\x1bOR",
            "f4" => "\x1bOS",
            "f5" => "\x1b[15~",
            "f6" => "\x1b[17~",
            "f7" => "\x1b[18~",
            "f8" => "\x1b[19~",
            "f9" => "\x1b[20~",
            "f10" => "\x1b[21~",
            "f11" => "\x1b[23~",
            "f12" => "\x1b[24~",
            _ => character?,
        };
        text.as_bytes().to_vec()
    };
    if alt {
        bytes.insert(0, 0x1b);
    }
    Some(bytes)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raw_input_controls_and_application_arrows() {
        assert_eq!(
            encode("c", Some("c"), true, false, false, false),
            Some(vec![3])
        );
        assert_eq!(
            encode("up", None, false, false, false, false),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            encode("up", None, false, false, false, true),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            encode("x", Some("中"), false, false, false, false),
            Some("中".as_bytes().to_vec())
        );
        assert_eq!(
            encode("x", Some("x"), false, true, false, false),
            Some(b"\x1bx".to_vec())
        );
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum Shortcut {
    Copy,
    Paste,
    Search,
    Suppress,
}
pub fn shortcut(
    key: &str,
    control: bool,
    shift: bool,
    platform: bool,
    macos: bool,
) -> Option<Shortcut> {
    if (platform && macos) || (control && shift) {
        match key {
            "c" => return Some(Shortcut::Copy),
            "v" => return Some(Shortcut::Paste),
            "f" => return Some(Shortcut::Search),
            _ => {}
        }
    }
    platform.then_some(Shortcut::Suppress)
}
#[cfg(test)]
mod shortcut_tests {
    use super::*;
    #[test]
    fn platform_shortcuts_never_become_remote_terminal_text() {
        assert_eq!(
            shortcut("c", false, false, true, true),
            Some(Shortcut::Copy)
        );
        assert_eq!(
            shortcut("v", false, false, true, true),
            Some(Shortcut::Paste)
        );
        assert_eq!(
            shortcut("f", false, false, true, true),
            Some(Shortcut::Search)
        );
        assert_eq!(
            shortcut("x", false, false, true, true),
            Some(Shortcut::Suppress)
        );
        assert_eq!(
            shortcut("v", false, false, true, false),
            Some(Shortcut::Suppress)
        );
        assert_eq!(shortcut("c", true, false, false, false), None);
        assert_eq!(
            shortcut("c", true, true, false, false),
            Some(Shortcut::Copy)
        );
    }
}
