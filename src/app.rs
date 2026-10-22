use gpui_kit::assets::IconName;
use gpui_kit::component::{
    Sizable, TitleBar,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    resizable::{ResizableState, h_resizable, resizable_panel, v_resizable},
};
use gpui_kit::{prelude::FluentBuilder as _, *};

use crate::{
    editor::{File, source_view},
    theme::*,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sidebar {
    Project,
    Search,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BottomTab {
    Terminal,
    Problems,
    Output,
}

pub struct HelixLane {
    sidebar: Option<Sidebar>,
    bottom: Option<BottomTab>,
    inspector: bool,
    source_expanded: bool,
    root_expanded: bool,
    tabs: Vec<File>,
    active: Option<File>,
    search: Entity<InputState>,
    // Each visible-panel configuration owns its indexed resize slots.
    horizontal: [Entity<ResizableState>; 4],
    vertical: Entity<ResizableState>,
    _search_subscription: Subscription,
}

fn tool(id: &'static str, name: IconName, label: &'static str, active: bool) -> Button {
    Button::new(id)
        .ghost()
        .icon(icon(name))
        .tooltip(label)
        .w(px(32.))
        .h(px(32.))
        .rounded(px(7.))
        .text_color(rgb(if active { ACCENT } else { MUTED }))
        .when(active, |b| b.bg(rgb(HOVER)))
}

fn panel_header(title: &'static str) -> Div {
    row()
        .h(px(40.))
        .flex_none()
        .px_3()
        .justify_between()
        .text_size(px(12.))
        .child(title)
}

impl HelixLane {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Find a file…"));
        let subscription = cx.subscribe(&search, |_, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        Self {
            sidebar: Some(Sidebar::Project),
            bottom: Some(BottomTab::Terminal),
            inspector: false,
            source_expanded: true,
            root_expanded: true,
            tabs: vec![File::Main, File::Cargo],
            active: Some(File::Main),
            search,
            horizontal: std::array::from_fn(|_| cx.new(|_| ResizableState::default())),
            vertical: cx.new(|_| ResizableState::default()),
            _search_subscription: subscription,
        }
    }

    fn open(&mut self, file: File, cx: &mut Context<Self>) {
        if !self.tabs.contains(&file) {
            self.tabs.push(file);
        }
        self.active = Some(file);
        cx.notify();
    }

    fn close(&mut self, file: File, cx: &mut Context<Self>) {
        if let Some(index) = self.tabs.iter().position(|tab| *tab == file) {
            self.tabs.remove(index);
            if self.active == Some(file) {
                self.active = self
                    .tabs
                    .get(index.min(self.tabs.len().saturating_sub(1)))
                    .copied();
            }
        }
        cx.notify();
    }

    fn toggle_sidebar(&mut self, sidebar: Sidebar, cx: &mut Context<Self>) {
        self.sidebar = if self.sidebar == Some(sidebar) {
            None
        } else {
            Some(sidebar)
        };
        cx.notify();
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        TitleBar::new().h(px(TOOLBAR_HEIGHT)).bg(rgb(FRAME)).child(
            row()
                .size_full()
                .pr_2()
                .gap_3()
                .child(
                    row()
                        .justify_center()
                        .size(px(25.))
                        .rounded(px(6.))
                        .bg(rgb(0x7364bb))
                        .text_color(rgb(0xffffff))
                        .text_size(px(11.))
                        .child("HL"),
                )
                .child(div().text_size(px(13.)).child("HelixLane"))
                .child(icon(IconName::ChevronDown).text_color(rgb(MUTED)))
                .child(div().h(px(17.)).w(px(1.)).bg(rgb(HOVER)))
                .child(icon(IconName::GitBranch).text_color(rgb(MUTED)))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(MUTED))
                        .child("main"),
                )
                .child(div().flex_1())
                .child(
                    row()
                        .gap_2()
                        .text_size(px(12.))
                        .text_color(rgb(MUTED))
                        .child(icon(IconName::Circle))
                        .child("Workspace preview"),
                )
                .child(
                    tool("find-file", IconName::Search, "Find a file", false).on_click(
                        cx.listener(|this, _, window, cx| {
                            this.sidebar = Some(Sidebar::Search);
                            this.search.update(cx, |input, cx| input.focus(window, cx));
                            cx.notify();
                        }),
                    ),
                ),
        )
    }

    fn left_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .items_center()
            .w(px(44.))
            .h_full()
            .flex_none()
            .pt_1()
            .gap_2()
            .child(
                tool(
                    "project",
                    IconName::FolderTree,
                    "Project",
                    self.sidebar == Some(Sidebar::Project),
                )
                .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(Sidebar::Project, cx))),
            )
            .child(
                tool(
                    "search",
                    IconName::Search,
                    "Find a file",
                    self.sidebar == Some(Sidebar::Search),
                )
                .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(Sidebar::Search, cx))),
            )
            .child(div().flex_1())
            .child(
                tool(
                    "terminal-toggle",
                    IconName::Terminal,
                    "Toggle terminal",
                    self.bottom == Some(BottomTab::Terminal),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.bottom = if this.bottom == Some(BottomTab::Terminal) {
                        None
                    } else {
                        Some(BottomTab::Terminal)
                    };
                    cx.notify();
                })),
            )
            .child(
                tool(
                    "problems-toggle",
                    IconName::CircleAlert,
                    "Toggle problems",
                    self.bottom == Some(BottomTab::Problems),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.bottom = if this.bottom == Some(BottomTab::Problems) {
                        None
                    } else {
                        Some(BottomTab::Problems)
                    };
                    cx.notify();
                })),
            )
            .child(div().h_2())
    }

    fn file_row(&self, file: File, indent: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.active == Some(file);
        row()
            .id(("file", file as usize))
            .h(px(29.))
            .mx_2()
            .pl(px(indent))
            .pr_2()
            .gap_2()
            .rounded(px(5.))
            .cursor_pointer()
            .when(selected, |r| r.bg(rgb(SELECTED)))
            .hover(|s| s.bg(rgb(if selected { SELECTED } else { HOVER })))
            .child(
                icon(if file.language() == "Rust" {
                    IconName::FileCode
                } else {
                    IconName::FileText
                })
                .text_color(rgb(if file.language() == "Rust" {
                    ORANGE
                } else {
                    MUTED
                })),
            )
            .child(file.name())
            .on_click(cx.listener(move |this, _, _, cx| this.open(file, cx)))
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let search = self.sidebar == Some(Sidebar::Search);
        let header = panel_header(if search { "Find a file" } else { "Project" }).child(
            tool(
                "close-sidebar",
                IconName::PanelLeftClose,
                "Hide tool window",
                false,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.sidebar = None;
                cx.notify();
            })),
        );
        let mut panel = island().child(header);
        if search {
            let query = self.search.read(cx).value().to_lowercase();
            let files: Vec<_> = File::ALL
                .into_iter()
                .filter(|file| file.path().to_lowercase().contains(&query))
                .collect();
            panel = panel
                .child(div().px_3().pb_3().child(Input::new(&self.search).small()))
                .child(
                    div()
                        .px_3()
                        .pb_2()
                        .text_size(px(11.))
                        .text_color(rgb(MUTED))
                        .child(format!("{} files · preview project", files.len())),
                );
            for file in files {
                panel = panel.child(self.file_row(file, 8., cx));
            }
        } else {
            panel = panel.child(
                row()
                    .id("root-folder")
                    .h(px(29.))
                    .px_3()
                    .gap_2()
                    .cursor_pointer()
                    .child(icon(if self.root_expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    }))
                    .child(icon(IconName::Folder).text_color(rgb(0xb3a0d6)))
                    .child("HelixLane")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.root_expanded = !this.root_expanded;
                        cx.notify();
                    })),
            );
            if self.root_expanded {
                panel = panel.child(
                    row()
                        .id("src-folder")
                        .h(px(29.))
                        .pl_6()
                        .gap_2()
                        .cursor_pointer()
                        .child(icon(if self.source_expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        }))
                        .child(icon(IconName::Folder).text_color(rgb(MUTED)))
                        .child("src")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.source_expanded = !this.source_expanded;
                            cx.notify();
                        })),
                );
                if self.source_expanded {
                    panel = panel
                        .child(self.file_row(File::Main, 48., cx))
                        .child(self.file_row(File::App, 48., cx));
                }
                panel = panel
                    .child(self.file_row(File::Cargo, 24., cx))
                    .child(self.file_row(File::Readme, 24., cx));
            }
        }
        panel
            .child(div().flex_1())
            .child(
                div()
                    .px_3()
                    .py_3()
                    .text_size(px(10.))
                    .text_color(rgb(MUTED))
                    .child("PREVIEW PROJECT"),
            )
            .into_any_element()
    }

    fn editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let tabs = row()
            .h(px(40.))
            .w_full()
            .flex_none()
            .gap_1()
            .px_2()
            .children(self.tabs.iter().map(|&file| {
                let active = self.active == Some(file);
                row()
                    .id(("tab", file as usize))
                    .h(px(29.))
                    .px_2()
                    .gap_2()
                    .rounded(px(5.))
                    .cursor_pointer()
                    .when(active, |r| r.bg(rgb(HOVER)))
                    .text_color(rgb(if active { TEXT } else { MUTED }))
                    .child(
                        icon(IconName::FileCode).text_color(rgb(if file.language() == "Rust" {
                            ORANGE
                        } else {
                            MUTED
                        })),
                    )
                    .child(file.name())
                    .child(
                        Button::new(("close-tab", file as usize))
                            .ghost()
                            .icon(icon(IconName::X))
                            .tooltip("Close tab")
                            .w(px(18.))
                            .h(px(18.))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close(file, cx);
                            })),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.open(file, cx)))
            }));
        let mut editor = island().bg(rgb(EDITOR)).child(tabs);
        if let Some(file) = self.active {
            editor = editor.child(
                row()
                    .h(px(29.))
                    .px_4()
                    .gap_2()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(rgb(MUTED))
                    .child("HelixLane")
                    .child(icon(IconName::ChevronRight))
                    .child(file.path())
                    .child(div().flex_1())
                    .child("Read-only preview"),
            );
            editor = editor.child(div().flex_1().min_h_0().child(source_view(file)));
        } else {
            editor = editor.child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap_4()
                    .child(
                        div()
                            .text_size(px(28.))
                            .text_color(rgb(0x555964))
                            .child("HelixLane"),
                    )
                    .child(
                        div()
                            .text_color(rgb(MUTED))
                            .child("Open a file from the Project panel"),
                    )
                    .child(
                        Button::new("reopen-project")
                            .ghost()
                            .label("Show Project")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.sidebar = Some(Sidebar::Project);
                                cx.notify();
                            })),
                    ),
            );
        }
        editor.into_any_element()
    }

    fn bottom(&self, cx: &mut Context<Self>) -> AnyElement {
        let header = panel_header("")
            .child(
                row().gap_1().children(
                    [
                        (BottomTab::Terminal, "Terminal"),
                        (BottomTab::Problems, "Problems"),
                        (BottomTab::Output, "Output"),
                    ]
                    .into_iter()
                    .enumerate()
                    .map(|(index, (tab, label))| {
                        Button::new(("bottom-tab", index))
                            .ghost()
                            .small()
                            .label(label)
                            .when(self.bottom == Some(tab), |b| {
                                b.bg(rgb(HOVER)).text_color(rgb(TEXT))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.bottom = Some(tab);
                                cx.notify();
                            }))
                    }),
                ),
            )
            .child(div().flex_1())
            .child(
                tool("hide-bottom", IconName::Minus, "Hide tool window", false).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.bottom = None;
                        cx.notify();
                    }),
                ),
            );
        let content = match self.bottom {
            Some(BottomTab::Terminal) => div()
                .flex()
                .flex_col()
                .gap_2()
                .font_family("Menlo")
                .text_size(px(12.))
                .child(
                    div()
                        .text_color(rgb(MUTED))
                        .child("Terminal preview · no shell connected"),
                )
                .child(
                    row()
                        .gap_2()
                        .child(div().text_color(rgb(GREEN)).child("➜"))
                        .child("HelixLane")
                        .child(div().text_color(rgb(ACCENT)).child("git:(main)")),
                )
                .child(
                    div()
                        .text_color(rgb(MUTED))
                        .child("Run cargo run --locked in your terminal to start this workspace."),
                ),
            Some(BottomTab::Problems) => div()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    row()
                        .gap_2()
                        .child(icon(IconName::CircleCheck).text_color(rgb(GREEN)))
                        .child("No diagnostics connected"),
                )
                .child(
                    div()
                        .text_color(rgb(MUTED))
                        .child("Compiler and language server diagnostics will appear here."),
                ),
            _ => div()
                .flex()
                .flex_col()
                .gap_2()
                .font_family("Menlo")
                .text_size(px(12.))
                .child(
                    div()
                        .text_color(rgb(GREEN))
                        .child("HelixLane workspace ready"),
                )
                .child(
                    div()
                        .text_color(rgb(MUTED))
                        .child("Rust + GPUI Kit 0.8.0 · Islands layout preview"),
                ),
        };
        island()
            .child(header)
            .child(
                div()
                    .id("bottom-content")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_4()
                    .py_3()
                    .child(content),
            )
            .into_any_element()
    }

    fn inspector(&self) -> AnyElement {
        let mut panel = island().child(panel_header("File information"));
        if let Some(file) = self.active {
            for (label, value) in [
                ("NAME", file.name().to_owned()),
                ("PATH", file.path().to_owned()),
                ("LANGUAGE", file.language().to_owned()),
                ("LINES", file.source().lines().count().to_string()),
                ("SIZE", format!("{} bytes", file.source().len())),
            ] {
                panel = panel.child(
                    div()
                        .px_4()
                        .py_3()
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(MUTED))
                                .mb_2()
                                .child(label),
                        )
                        .child(value),
                );
            }
        } else {
            panel = panel.child(div().p_4().text_color(rgb(MUTED)).child("No file selected"));
        }
        panel.into_any_element()
    }
}

impl Render for HelixLane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let center = if self.bottom.is_some() {
            v_resizable("editor-bottom")
                .with_state(&self.vertical)
                .child(
                    resizable_panel()
                        .size_range(px(200.)..px(10000.))
                        .child(div().size_full().pb(px(5.)).child(self.editor(cx))),
                )
                .child(
                    resizable_panel()
                        .size(px(210.))
                        .size_range(px(120.)..px(500.))
                        .child(self.bottom(cx)),
                )
                .into_any_element()
        } else {
            self.editor(cx)
        };
        let layout = usize::from(self.sidebar.is_some()) | (usize::from(self.inspector) << 1);
        let mut group =
            h_resizable(("workspace-panels", layout)).with_state(&self.horizontal[layout]);
        if self.sidebar.is_some() {
            group = group.child(
                resizable_panel()
                    .size(px(250.))
                    .size_range(px(180.)..px(400.))
                    .child(div().size_full().pr(px(5.)).child(self.sidebar(cx))),
            );
        }
        group = group.child(
            resizable_panel()
                .size_range(px(320.)..px(10000.))
                .child(center),
        );
        if self.inspector {
            group = group.child(
                resizable_panel()
                    .size(px(210.))
                    .size_range(px(180.)..px(320.))
                    .child(div().size_full().pl(px(5.)).child(self.inspector())),
            );
        }
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(FRAME))
            .text_color(rgb(TEXT))
            .font_family(".AppleSystemUIFont")
            .text_size(px(12.))
            .child(self.toolbar(cx))
            .child(
                row()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .child(self.left_rail(cx))
                    .child(div().flex_1().min_w_0().h_full().child(group))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .w(px(40.))
                            .h_full()
                            .flex_none()
                            .pt_1()
                            .child(
                                tool(
                                    "info-toggle",
                                    IconName::PanelRight,
                                    "Toggle file information",
                                    self.inspector,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.inspector = !this.inspector;
                                        cx.notify();
                                    },
                                )),
                            ),
                    ),
            )
            .child(
                row()
                    .h(px(28.))
                    .flex_none()
                    .px_3()
                    .gap_3()
                    .text_size(px(11.))
                    .text_color(rgb(MUTED))
                    .child(icon(IconName::GitBranch))
                    .child("main")
                    .child(div().text_color(rgb(GREEN)).child("●"))
                    .child("Local workspace")
                    .child(div().flex_1())
                    .child("Read-only preview")
                    .child("UTF-8")
                    .child("LF")
                    .child(self.active.map_or("No file", File::language)),
            )
    }
}
