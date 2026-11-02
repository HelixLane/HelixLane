//! Movable resource tool; command targets and terminal-instance selection are explicit.
use crate::{terminal_ui::TerminalPane, theme::*};
use gpui_kit::{
    base::Disableable,
    component::{
        Sizable,
        button::{Button, ButtonVariants},
        input::{Input, InputState},
    },
    *,
};
use helixlane::{
    i18n::Language,
    settings::Settings,
    sftp::tmux,
    ssh::{ExecTask, Service},
    workspace::{ConnectionProfile, ToolKind},
};
use std::{cell::RefCell, rc::Rc, time::Duration};
pub struct TmuxPane {
    polling: Option<Task<()>>,
    window_handle: Option<AnyWindowHandle>,
    id: u64,
    ticket: Option<helixlane::workspace::TargetTicket>,
    visible: bool,
    window_visible: bool,
    visibility: Subscription,
    last_refresh: std::time::Instant,
    collecting: bool,
    settings: Rc<RefCell<Settings>>,
    profile: ConnectionProfile,
    auth: Entity<TerminalPane>,
    name: Entity<InputState>,
    rename: Entity<InputState>,
    pending: Option<(u64, ExecTask, bool)>,
    generation: u64,
    sessions: Vec<String>,
    output: String,
    error: Option<String>,
    terminal: Option<u64>,
}
fn btn(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Button {
    Button::new(id).ghost().small().label(label)
}
impl TmuxPane {
    pub fn new(
        id: u64,
        profile: ConnectionProfile,
        settings: Rc<RefCell<Settings>>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let zh = settings.borrow().language == Language::Chinese;
        let auth = cx.new(|cx| {
            let mut a = TerminalPane::new(id, profile.clone(), settings.clone(), w, cx);
            a.transport_only();
            a
        });
        let visibility =
            cx.observe_window_visibility(w, |this, v, _, _| this.window_visible = v.is_visible());
        let ticket = settings.borrow().workspace.ticket(id);
        let mut this = Self {
            polling: None,
            window_handle: None,
            id,
            ticket,
            visible: true,
            window_visible: w.is_visible(),
            visibility,
            last_refresh: std::time::Instant::now(),
            collecting: false,
            settings,
            profile,
            auth,
            name: cx.new(|cx| {
                InputState::new(w, cx).placeholder(if zh { "会话名称" } else { "Session name" })
            }),
            rename: cx.new(|cx| {
                InputState::new(w, cx).placeholder(if zh {
                    "新会话名称"
                } else {
                    "New session name"
                })
            }),
            pending: None,
            generation: 0,
            sessions: vec![],
            output: String::new(),
            error: None,
            terminal: None,
        };
        this.start_polling(w, cx);
        this
    }
    fn start_polling(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.window_handle = Some(window.window_handle());
        self.polling = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                let Ok(handle) = this.update(cx, |this, _| this.window_handle) else {
                    break;
                };
                if let Some(handle) = handle {
                    let _ = handle.update(cx, |_, window, cx| {
                        let _ = this.update(cx, |this, cx| this.sync_binding(window, cx));
                    });
                }
                if this
                    .update(cx, |this, cx| {
                        this.poll(cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }
    pub fn cancel_owned_jobs(&mut self, cx: &mut Context<Self>) {
        self.auth.update(cx, |a, _| a.cancel_owned_jobs());
        if let Some((_, task, _)) = &mut self.pending {
            task.cancel();
        }
    }
    pub fn rebind_window(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self.window_visible = w.is_visible();
        self.visibility =
            cx.observe_window_visibility(w, |this, v, _, _| this.window_visible = v.is_visible());
        for field in [&mut self.name, &mut self.rename] {
            let value = field.read(cx).value();
            *field = cx.new(|cx| InputState::new(w, cx).default_value(value));
        }
        self.auth.update(cx, |a, cx| a.rebind_window(w, cx));
        self.start_polling(w, cx);
    }
    pub fn set_visible(&mut self, visible: bool) {
        if visible && !self.visible {
            self.last_refresh = std::time::Instant::now() - Duration::from_secs(6);
        }
        self.visible = visible;
    }
    fn sync_binding(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let profile = {
            let settings = self.settings.borrow();
            settings
                .workspace
                .capture_task_target(self.id)
                .map(|t| t.profile)
        };
        if let Some(profile) = profile {
            self.bind(profile, w, cx);
        }
    }
    pub fn bind(&mut self, p: ConnectionProfile, w: &mut Window, cx: &mut Context<Self>) {
        let ticket = self.settings.borrow().workspace.ticket(self.id);
        if p == self.profile && ticket == self.ticket {
            return;
        }
        self.ticket = ticket;
        self.profile = p.clone();
        self.auth.update(cx, |a, cx| a.bind(p, w, cx));
        self.generation += 1;
        self.pending = None;
        self.sessions.clear();
        self.output.clear();
        self.error = None;
        self.terminal = None;
    }
    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.name.read(cx).focus_handle(cx)
    }
    fn poll(&mut self, cx: &mut Context<Self>) {
        let should = self.settings.borrow().workspace.should_refresh(
            self.id,
            self.visible,
            !self.window_visible,
        );
        if self.collecting
            && self.pending.is_none()
            && should
            && self.last_refresh.elapsed() > Duration::from_secs(5)
            && self.auth.read(cx).shared_session().is_some()
        {
            self.last_refresh = std::time::Instant::now();
            self.run(tmux::Operation::List, true, cx);
        }
        if let Some((generation, task, list)) = &mut self.pending
            && let Ok(result) = task.result.try_recv()
        {
            if *generation == self.generation {
                match result {
                    Ok(o) => {
                        let text = String::from_utf8_lossy(&o.bytes).to_string();
                        if *list && o.exit_status == Some(0) {
                            self.sessions = text.lines().map(str::to_string).collect();
                        }
                        if o.exit_status != Some(0) {
                            if *list {
                                self.sessions.clear();
                            }
                            self.error = Some(if text.trim().is_empty() {
                                "tmux command failed; check installation and session availability"
                                    .into()
                            } else {
                                text
                            });
                        } else {
                            self.error = None;
                            self.output = text;
                        }
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            self.pending = None;
        }
    }
    fn run(&mut self, op: tmux::Operation, list: bool, cx: &mut Context<Self>) {
        let Some(session) = self.auth.read(cx).shared_session() else {
            self.error = Some("Connection not ready".into());
            return;
        };
        if list {
            self.collecting = true;
            self.last_refresh = std::time::Instant::now();
        }
        match tmux::run(session, op) {
            Ok(task) => self.pending = Some((self.generation, task, list)),
            Err(e) => self.error = Some(e),
        }
    }
}
impl Render for TmuxPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = &self.visibility;
        let p = Palette::current(cx);
        let zh = self.settings.borrow().language == Language::Chinese;
        let t = |en, cn| if zh { cn } else { en };
        let mut content = div()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .p_2()
            .gap_2()
            .child(div().flex_none().child(self.auth.clone()))
            .child(
                row()
                    .gap_2()
                    .child(btn("detect", t("Detect tmux", "检测 tmux")).on_click(
                        cx.listener(|this, _, _, cx| this.run(tmux::Operation::Detect, false, cx)),
                    ))
                    .child(btn("list", t("Refresh sessions", "刷新会话")).on_click(
                        cx.listener(|this, _, _, cx| this.run(tmux::Operation::List, true, cx)),
                    )),
            );
        if let Some(e) = &self.error {
            content = content.child(div().text_color(rgb(p.warning)).child(format!(
                "{}: {}",
                t("Error", "错误"),
                self.settings.borrow().language.file_error(e)
            )));
        }
        let mut list = div()
            .id("tmux-sessions")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_1();
        if self.sessions.is_empty() {
            list = list.child(div().p_3().text_color(rgb(p.muted)).child(t(
                "No sessions loaded. Detect tmux, then refresh.",
                "尚未加载会话。请先检测 tmux，然后刷新。",
            )));
        }
        for (i, name) in self.sessions.clone().into_iter().enumerate() {
            list = list.child(btn(("session", i), name.clone()).on_click(cx.listener(
                move |this, _, w, cx| {
                    this.name
                        .update(cx, |s, cx| s.set_value(name.clone(), w, cx))
                },
            )));
        }
        content = content.child(list).child(
            row()
                .gap_2()
                .flex_wrap()
                .child(div().w(px(180.)).child(Input::new(&self.name).small()))
                .child(
                    btn("new", t("New session", "新建会话")).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.run(
                                tmux::Operation::New(this.name.read(cx).value().to_string()),
                                false,
                                cx,
                            )
                        },
                    )),
                )
                .child(div().w(px(180.)).child(Input::new(&self.rename).small()))
                .child(btn("rename", t("Rename", "重命名")).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.run(
                            tmux::Operation::Rename {
                                old: this.name.read(cx).value().to_string(),
                                new: this.rename.read(cx).value().to_string(),
                            },
                            false,
                            cx,
                        )
                    },
                ))),
        );
        let terminals = {
            let settings = self.settings.borrow();
            settings
                .workspace
                .views
                .iter()
                .filter(|v| v.tool == ToolKind::Terminal)
                .filter_map(|v| {
                    let session = Service::global().session(v.session_id?)?;
                    if !session.terminal_running() || session.profile != self.profile {
                        return None;
                    }
                    let resource = settings
                        .workspace
                        .target(v.id)
                        .and_then(|id| settings.workspace.resources.iter().find(|r| r.id == id))
                        .map(|r| r.name.clone())
                        .unwrap_or_default();
                    Some((
                        v.id,
                        format!(
                            "{} · {} #{} · SSH #{}",
                            resource,
                            t("Terminal", "终端"),
                            v.id,
                            session.id
                        ),
                    ))
                })
                .collect::<Vec<_>>()
        };
        content = content.child(
            div()
                .text_color(rgb(p.muted))
                .child(t("Target terminal instance", "目标终端实例")),
        );
        let mut targets = row().gap_1().flex_wrap();
        for (id, label) in terminals {
            let selected = self.terminal == Some(id);
            targets = targets.child(
                btn(
                    ("terminal-target", id),
                    format!("{}{}", if selected { "✓ " } else { "" }, label),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.terminal = Some(id);
                    cx.notify();
                })),
            );
        }
        content = content.child(targets).child(
            btn("switch", t("Switch selected terminal", "切换所选终端"))
                .disabled(self.terminal.is_none())
                .on_click(cx.listener(|this, _, _, cx| {
                    let id = this.terminal.unwrap_or(0);
                    let session = this
                        .settings
                        .borrow()
                        .workspace
                        .views
                        .iter()
                        .find(|v| v.id == id && v.tool == ToolKind::Terminal)
                        .and_then(|v| v.session_id)
                        .and_then(|id| Service::global().session(id));
                    let result = if let Some(s) = session {
                        if s.profile != this.profile {
                            Err("Target terminal connection changed".into())
                        } else {
                            tmux::switch(id, s, &this.name.read(cx).value())
                        }
                    } else {
                        Err("Select a running target terminal instance".into())
                    };
                    if let Err(e) = result {
                        this.error = Some(e)
                    }
                })),
        );
        if !self.output.is_empty() {
            content = content.child(
                div()
                    .max_h(px(80.))
                    .overflow_hidden()
                    .text_size(px(11.))
                    .child(self.output.clone()),
            );
        }
        content.into_any_element()
    }
}
