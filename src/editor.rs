use gpui_kit::*;

use crate::theme::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum File {
    Main,
    App,
    Cargo,
    Readme,
}

impl File {
    pub const ALL: [Self; 4] = [Self::Main, Self::App, Self::Cargo, Self::Readme];

    pub fn name(self) -> &'static str {
        match self {
            Self::Main => "main.rs",
            Self::App => "app.rs",
            Self::Cargo => "Cargo.toml",
            Self::Readme => "README.md",
        }
    }

    pub fn path(self) -> &'static str {
        match self {
            Self::Main => "src/main.rs",
            Self::App => "src/app.rs",
            Self::Cargo => "Cargo.toml",
            Self::Readme => "README.md",
        }
    }

    pub fn language(self) -> &'static str {
        match self {
            Self::Main | Self::App => "Rust",
            Self::Cargo => "TOML",
            Self::Readme => "Markdown",
        }
    }

    pub fn source(self) -> &'static str {
        match self {
            Self::Main => include_str!("main.rs"),
            Self::App => include_str!("app.rs"),
            Self::Cargo => include_str!("../Cargo.toml"),
            Self::Readme => include_str!("../README.md"),
        }
    }
}

fn highlighted(line: &str, rust: bool) -> AnyElement {
    let mut fragments = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        let mut token = ch.to_string();
        let color = if ch == '/' && chars.peek() == Some(&'/') {
            token.extend(chars.by_ref());
            MUTED
        } else if ch == '"' {
            let mut escaped = false;
            for next in chars.by_ref() {
                token.push(next);
                if next == '"' && !escaped {
                    break;
                }
                escaped = next == '\\' && !escaped;
            }
            GREEN
        } else if ch.is_alphanumeric() || ch == '_' {
            while chars
                .peek()
                .is_some_and(|c| c.is_alphanumeric() || *c == '_')
            {
                token.push(chars.next().unwrap());
            }
            if rust
                && matches!(
                    token.as_str(),
                    "use"
                        | "mod"
                        | "fn"
                        | "pub"
                        | "let"
                        | "mut"
                        | "if"
                        | "else"
                        | "impl"
                        | "struct"
                        | "enum"
                        | "match"
                        | "self"
                        | "Self"
                        | "true"
                        | "false"
                        | "const"
                        | "return"
                        | "for"
                        | "in"
                )
            {
                ORANGE
            } else if ch.is_uppercase() {
                0x9eaff3
            } else if ch.is_ascii_digit() {
                0xb3a0d6
            } else {
                TEXT
            }
        } else {
            TEXT
        };
        fragments.push(div().flex_none().text_color(rgb(color)).child(token));
    }
    row().children(fragments).into_any_element()
}

pub fn source_view(file: File) -> AnyElement {
    div()
        .id(("source", file as usize))
        .size_full()
        .overflow_scroll()
        .font_family("Menlo")
        .text_size(px(12.))
        .line_height(px(23.))
        .py_3()
        .children(file.source().lines().enumerate().map(|(index, line)| {
            row()
                .min_w_full()
                .h(px(23.))
                .flex_none()
                .child(
                    div()
                        .w(px(54.))
                        .flex_none()
                        .pr_4()
                        .text_right()
                        .text_color(rgb(0x626773))
                        .child((index + 1).to_string()),
                )
                .child(highlighted(line, file.language() == "Rust"))
                .child(div().w_8().flex_none())
        }))
        .into_any_element()
}
