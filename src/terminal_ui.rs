//! Desktop terminal view. Network sessions are owned by Service, never by this entity.
mod input;
use crate::theme::*;
use alacritty_terminal::vte::ansi::{Color, NamedColor};
use gpui_kit::base::Disableable;
use gpui_kit::{
    component::{
        Sizable,
        button::{Button, ButtonVariants},
        input::{Input, InputState},
    },
    prelude::FluentBuilder as _,
    *,
};
use helixlane::{
    i18n::Language,
    settings::Settings,
    ssh::{Service, Session, Status, TerminalStatus, credentials::Secret},
    workspace::{AuthMethod, ConnectionProfile},
};
use std::{cell::RefCell, rc::Rc, sync::Arc, time::Duration};

pub struct TerminalPane {
    owned_terminals: Vec<Arc<Session>>,
    owned_forwards: Vec<Arc<helixlane::ssh::forward::ForwardJob>>,
    transport_only: bool,
    view_id: u64,
    settings: Rc<RefCell<Settings>>,
    profile: ConnectionProfile,
    bound_profile: ConnectionProfile,
    session: Option<Arc<Session>>,
    sessions: Vec<Arc<Session>>,
    password: Entity<InputState>,
    username: Entity<InputState>,
    launch_secret: Option<Secret>,
    key_path: Entity<InputState>,
    search: Entity<InputState>,
    remember: bool,
    auth_fields: Vec<Entity<InputState>>,
    shown_prompt: Option<helixlane::ssh::auth::AuthPrompt>,
    forward_bind: Entity<InputState>,
    forward_host: Entity<InputState>,
    forward_port: Entity<InputState>,
    forward_mode: u8,
    show_network: bool,
    focus: FocusHandle,
    bounds: Rc<RefCell<Bounds<Pixels>>>,
    selecting: Option<(usize, usize)>,
    _search_subscription: Subscription,
    preedit: String,
    composition_target: Option<u64>,
}
fn action(id: &'static str, label: impl Into<SharedString>) -> Button {
    Button::new(id).ghost().small().label(label)
}
impl TerminalPane {
    pub fn new(
        view_id: u64,
        profile: ConnectionProfile,
        settings: Rc<RefCell<Settings>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.spawn(async move |this, cx| {
            let mut revision = None;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(40))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        let current = this
                            .session
                            .as_ref()
                            .map(|s| (s.id, s.state.lock().unwrap().revision));
                        if current != revision {
                            revision = current;
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let path = match &profile.auth {
            AuthMethod::PrivateKey { path } => path.clone(),
            _ => String::new(),
        };
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Find in visible screen"));
        let search_subscription = cx.observe(&search, |_, _, cx| cx.notify());
        Self {
            owned_terminals: vec![],
            owned_forwards: vec![],
            transport_only: false,
            _search_subscription: search_subscription,
            preedit: String::new(),
            composition_target: None,
            view_id,
            settings,
            bound_profile: profile.clone(),
            username: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(profile.username.clone())
                    .placeholder("Username / 用户名")
            }),
            launch_secret: None,
            profile,
            session: None,
            sessions: Vec::new(),
            password: cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder("Password / passphrase")
            }),
            key_path: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(path)
                    .placeholder("/home/user/.ssh/id_ed25519")
            }),
            search,
            remember: false,
            auth_fields: Vec::new(),
            shown_prompt: None,
            forward_bind: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value("127.0.0.1:0")
                    .placeholder("Bind IP:port / 监听地址")
            }),
            forward_host: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value("127.0.0.1")
                    .placeholder("Destination host / 目标主机")
            }),
            forward_port: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value("80")
                    .placeholder("Destination port / 目标端口")
            }),
            forward_mode: 0,
            show_network: false,
            focus: cx.focus_handle(),
            bounds: Default::default(),
            selecting: None,
        }
    }
    pub fn cancel_owned_jobs(&mut self) {
        for session in &self.owned_terminals {
            session.stop_terminal();
        }
        for job in &self.owned_forwards {
            job.stop();
        }
    }
    pub fn rebind_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for field in [
            &mut self.username,
            &mut self.key_path,
            &mut self.search,
            &mut self.forward_bind,
            &mut self.forward_host,
            &mut self.forward_port,
        ] {
            let value = field.read(cx).value();
            *field = cx.new(|cx| InputState::new(window, cx).default_value(value));
        }
        let password = self.password.read(cx).value();
        self.password = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .default_value(password)
        });
        for field in &mut self.auth_fields {
            let value = field.read(cx).value();
            *field = cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .default_value(value)
            });
        }
        self._search_subscription = cx.observe(&self.search, |_, _, cx| cx.notify());
        self.preedit.clear();
        self.composition_target = None;
    }
    pub fn transport_only(&mut self) {
        self.transport_only = true;
    }
    pub fn session_identity(&self) -> Option<u64> {
        self.session.as_ref().map(|s| s.id)
    }
    pub fn shared_session(&self) -> Option<Arc<Session>> {
        self.session
            .clone()
            .filter(|s| s.state.lock().unwrap().status == Status::Connected)
    }
    pub fn bind(
        &mut self,
        profile: ConnectionProfile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.bound_profile == profile {
            return;
        }
        self.bound_profile = profile.clone();
        self.username.update(cx, |s, cx| {
            s.set_value(profile.username.clone(), window, cx)
        });
        self.launch_secret = None;
        self.profile = profile;
        self.session = self
            .sessions
            .iter()
            .rev()
            .find(|s| s.profile == self.profile)
            .cloned()
            .or_else(|| {
                if self.transport_only
                    && self.profile.transport_policy
                        != helixlane::workspace::TransportPolicy::Independent
                {
                    Service::global()
                        .resumable(&self.profile)
                        .into_iter()
                        .last()
                } else {
                    None
                }
            });
        self.password
            .update(cx, |s, cx| s.set_value("", window, cx));
        let path = match &self.profile.auth {
            AuthMethod::PrivateKey { path } => path.clone(),
            _ => String::new(),
        };
        self.key_path
            .update(cx, |s, cx| s.set_value(path, window, cx));
        self.selecting = None;
        self.preedit.clear();
        if let Some(view) = self
            .settings
            .borrow_mut()
            .workspace
            .views
            .iter_mut()
            .find(|v| v.id == self.view_id)
        {
            view.session_id = self.session.as_ref().map(|s| s.id);
        }
        cx.notify();
    }
    fn persist_profile(&self, cx: &App) {
        if self.profile.ephemeral {
            // Update the transient model for this view, without credential refs or disk writes.
            let mut settings = self.settings.borrow_mut();
            for resource in &mut settings.workspace.resources {
                if let Some(p) = resource
                    .profiles
                    .iter_mut()
                    .find(|p| p.id == self.profile.id)
                {
                    *p = self.profile.clone();
                    p.route = None;
                }
            }
            return;
        }
        let mut settings = self.settings.borrow_mut();
        for resource in &mut settings.workspace.resources {
            if let Some(p) = resource
                .profiles
                .iter_mut()
                .find(|p| p.id == self.profile.id)
            {
                *p = self.profile.clone();
                p.route = None;
            }
        }
        drop(settings);
        if let Some(owner) = cx.try_global::<crate::application::ApplicationOwner>()
            && let Err(error) = owner.save()
        {
            eprintln!("Unable to save connection profile: {error}");
        }
    }
    /// Consume launch auth once, using the same visible host-trust/session flow.
    pub fn launch(&mut self, secret: Option<Secret>, window: &mut Window, cx: &mut Context<Self>) {
        if self.session.as_ref().is_some_and(|s| {
            matches!(
                s.state.lock().unwrap().status,
                Status::Connected
                    | Status::Connecting
                    | Status::Authenticating
                    | Status::HostKey { .. }
            )
        }) {
            return;
        }
        let supplied = secret.is_some();
        self.launch_secret = secret;
        let ready = !self.profile.username.is_empty()
            && (supplied
                || self.profile.credential_ref.is_some()
                || matches!(
                    self.profile.auth,
                    AuthMethod::None
                        | AuthMethod::PrivateKey { .. }
                        | AuthMethod::Agent
                        | AuthMethod::KeyboardInteractive
                ));
        if ready {
            self.connect(window, cx);
        } else {
            window.focus(&self.password.read(cx).focus_handle(cx), cx);
            cx.notify();
        }
    }
    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.profile.username = self.username.read(cx).value().to_string();
        if self.profile.username.trim().is_empty() {
            window.focus(&self.username.read(cx).focus_handle(cx), cx);
            return;
        }
        if self.profile.ephemeral {
            self.remember = false;
            self.profile.credential_ref = None;
        }
        if matches!(self.profile.auth, AuthMethod::PrivateKey { .. }) {
            self.profile.auth = AuthMethod::PrivateKey {
                path: self.key_path.read(cx).value().to_string(),
            };
        }
        // The keychain reference is harmless metadata; the secret is never in Settings.
        if self.remember {
            self.profile.credential_ref = Some(format!("profile-{}", self.profile.id));
        }
        self.profile
            .capture_route(&self.settings.borrow().workspace.resources);
        self.persist_profile(cx);
        self.bound_profile = self.profile.clone();
        let secret = self
            .launch_secret
            .take()
            .unwrap_or_else(|| Secret::new(self.password.read(cx).value().to_string()));
        self.password
            .update(cx, |s, cx| s.set_value("", window, cx));
        let session = if self.transport_only {
            Service::global().connect_transport(self.profile.clone(), secret, self.remember)
        } else {
            Service::global().connect(self.profile.clone(), secret, self.remember)
        };
        if !self.transport_only {
            self.owned_terminals.push(session.clone());
        }
        self.sessions.push(session.clone());
        if let Some(view) = self
            .settings
            .borrow_mut()
            .workspace
            .views
            .iter_mut()
            .find(|v| v.id == self.view_id)
        {
            view.session_id = Some(session.id);
        }
        self.session = Some(session);
        window.focus(&self.focus, cx);
        cx.notify();
    }
    fn position(&self, position: Point<Pixels>) -> (usize, usize) {
        let bounds = *self.bounds.borrow();
        let x = f32::from(position.x - bounds.origin.x).max(0.) / 8.4;
        let y = f32::from(position.y - bounds.origin.y).max(0.) / 20.;
        let dimensions = self
            .session
            .as_ref()
            .map(|s| s.state.lock().unwrap().terminal.dimensions())
            .unwrap_or((80, 24));
        (
            (x as usize).min(dimensions.0 - 1),
            (y as usize).min(dimensions.1 - 1),
        )
    }
    fn copy(&self, cx: &mut App) {
        if let Some(session) = &self.session
            && let Some(text) = session.state.lock().unwrap().terminal.selection()
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }
    fn paste_clipboard(&self, cx: &mut App) {
        if let Some(session) = &self.session
            && let Some(text) = cx.read_from_clipboard().and_then(|c| c.text())
        {
            let bracketed = session.state.lock().unwrap().terminal.bracketed_paste();
            let text = if bracketed {
                format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', ""))
            } else {
                text
            };
            session.input(text.into_bytes());
        }
    }
    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        use helixlane::terminal::input::Shortcut;
        if let Some(shortcut) = helixlane::terminal::input::shortcut(
            &k.key,
            k.modifiers.control,
            k.modifiers.shift,
            k.modifiers.platform,
            cfg!(target_os = "macos"),
        ) {
            match shortcut {
                Shortcut::Copy => self.copy(cx),
                Shortcut::Paste => self.paste_clipboard(cx),
                Shortcut::Search => self.search.update(cx, |s, cx| s.focus(window, cx)),
                Shortcut::Suppress => {}
            }
            cx.stop_propagation();
            return;
        }
        let Some(session) = &self.session else {
            return;
        };
        if !session.terminal_running() {
            return;
        }
        let application = session.state.lock().unwrap().terminal.application_cursor();
        let bytes = helixlane::terminal::input::encode(
            &k.key,
            if k.modifiers.control || k.modifiers.alt {
                k.key_char
                    .as_deref()
                    .or_else(|| (k.key.len() == 1).then_some(k.key.as_str()))
            } else {
                None
            },
            k.modifiers.control,
            k.modifiers.alt,
            k.modifiers.shift,
            application,
        );
        if let Some(bytes) = bytes {
            session.input(bytes);
            cx.stop_propagation();
        }
    }
}
impl Focusable for TerminalPane {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for TerminalPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::current(cx);
        let language = self.settings.borrow().language;
        let status = self
            .session
            .as_ref()
            .map(|s| s.state.lock().unwrap().status.clone())
            .unwrap_or(Status::Disconnected);
        let connected = status == Status::Connected;
        let terminal_status = self
            .session
            .as_ref()
            .map(|session| session.state.lock().unwrap().terminal_status)
            .unwrap_or(TerminalStatus::NotRequested);
        let terminal_running = connected && terminal_status == TerminalStatus::Running;
        let busy = matches!(
            status,
            Status::Connecting | Status::Authenticating | Status::HostKey { .. }
        );
        let text = |en: &'static str, zh: &'static str| {
            if language == Language::Chinese {
                zh
            } else {
                en
            }
        };
        let mut content = div()
            .flex()
            .flex_col()
            .w_full()
            .when(!self.transport_only, |d| d.h_full())
            .min_w_0()
            .min_h_0()
            .bg(rgb(p.editor));
        let hop_label = self
            .session
            .as_ref()
            .and_then(|s| s.state.lock().unwrap().hop.clone())
            .map(|h| format!(" · {}", h.label()))
            .unwrap_or_default();
        let status_label = match &status {
            Status::Connecting => text("Connecting…", "连接中…").to_string(),
            Status::Authenticating => {
                format!("{}{hop_label}", text("Authenticating…", "正在认证…"))
            }
            Status::Connected => format!(
                "{} · #{}",
                if terminal_status == TerminalStatus::Finished {
                    text(
                        "Shell finished · SSH connected",
                        "Shell 已结束 · SSH 仍已连接",
                    )
                } else {
                    text("Connected", "已连接")
                },
                self.session.as_ref().unwrap().id
            ),
            Status::Disconnected => text("Disconnected", "未连接").into(),
            Status::HostKey { .. } => {
                text("Host identity requires approval", "请验证主机身份").into()
            }
            Status::Failed(e) => format!(
                "{}{hop_label}: {}",
                text("Connection failed", "连接失败"),
                language.ssh_error(e)
            ),
        };
        if let Some(notice) = self
            .session
            .as_ref()
            .and_then(|session| session.state.lock().unwrap().input_notice.clone())
        {
            content = content.child(
                div()
                    .px_3()
                    .py_1()
                    .text_size(px(11.))
                    .text_color(rgb(p.warning))
                    .child(language.ssh_error(&notice).to_string()),
            );
        }
        content = content.child(
            row()
                .h(px(36.))
                .px_3()
                .gap_2()
                .border_b_1()
                .border_color(rgb(p.border))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(11.))
                        .text_color(rgb(if connected { p.accent } else { p.muted }))
                        .child(status_label),
                )
                .child(if connected || busy {
                    action("disconnect", text("Disconnect", "断开")).on_click(cx.listener(
                        |this, _, _, cx| {
                            if let Some(s) = &this.session {
                                s.disconnect();
                            }
                            cx.notify();
                        },
                    ))
                } else {
                    action("connect", text("Connect", "连接"))
                        .on_click(cx.listener(|this, _, w, cx| this.connect(w, cx)))
                })
                .when(!self.transport_only, |d| {
                    d.child(
                        action("copy", text("Copy", "复制"))
                            .on_click(cx.listener(|this, _, _, cx| this.copy(cx))),
                    )
                    .child(
                        action("paste", text("Paste", "粘贴"))
                            .on_click(cx.listener(|this, _, _, cx| this.paste_clipboard(cx))),
                    )
                }),
        );
        if self.session.is_none() {
            for session in Service::global()
                .resumable(&self.profile)
                .into_iter()
                .filter(|s| {
                    self.transport_only
                        || s.state.lock().unwrap().terminal_status != TerminalStatus::NotRequested
                })
            {
                let label = format!("{} #{}", text("Resume session", "恢复会话"), session.id);
                content = content.child(
                    Button::new(("resume", session.id))
                        .ghost()
                        .small()
                        .label(label)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.session = Some(session.clone());
                            this.sessions.push(session.clone());
                            if let Some(view) = this
                                .settings
                                .borrow_mut()
                                .workspace
                                .views
                                .iter_mut()
                                .find(|v| v.id == this.view_id)
                            {
                                view.session_id = Some(session.id);
                            }
                            window.focus(&this.focus, cx);
                            cx.notify();
                        })),
                );
            }
        }
        if !connected && !busy {
            let mut auth = row()
                .px_3()
                .py_2()
                .gap_2()
                .flex_wrap()
                .child(
                    div()
                        .text_size(px(11.))
                        .child(text("Authentication", "认证")),
                )
                .child(
                    action("password", text("Password", "密码")).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.profile.auth = AuthMethod::Password;
                            cx.notify();
                        },
                    )),
                )
                .child(
                    action("key", text("Private key", "私钥")).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.profile.auth = AuthMethod::PrivateKey {
                                path: String::new(),
                            };
                            cx.notify();
                        },
                    )),
                )
                .child(action("none", text("None", "无")).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.profile.auth = AuthMethod::None;
                        cx.notify();
                    },
                )));
            auth = auth
                .child(
                    action("agent", text("SSH agent", "SSH Agent")).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.profile.auth = AuthMethod::Agent;
                            this.remember = false;
                            cx.notify();
                        },
                    )),
                )
                .child(
                    action("interactive", text("Interactive", "交互式")).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.profile.auth = AuthMethod::KeyboardInteractive;
                            this.remember = false;
                            cx.notify();
                        },
                    )),
                );
            auth = auth.child(div().text_size(px(11.)).text_color(rgb(p.accent)).child(
                match self.profile.auth {
                    AuthMethod::Password => text("Password selected", "已选密码"),
                    AuthMethod::PrivateKey { .. } => text("Private key selected", "已选私钥"),
                    AuthMethod::None => text("No authentication selected", "已选无认证"),
                    AuthMethod::Agent => text("SSH agent selected", "已选 SSH Agent"),
                    AuthMethod::KeyboardInteractive => {
                        text("Interactive selected", "已选交互式认证")
                    }
                },
            ));
            content = content.child(auth);
            if self.profile.ephemeral || self.profile.username.is_empty() {
                content = content.child(
                    div()
                        .px_3()
                        .pb_2()
                        .child(Input::new(&self.username).small()),
                );
            }
            if matches!(self.profile.auth, AuthMethod::PrivateKey { .. }) {
                content = content.child(
                    div()
                        .px_3()
                        .pb_2()
                        .child(Input::new(&self.key_path).small()),
                );
            }
            if matches!(
                self.profile.auth,
                AuthMethod::Password | AuthMethod::PrivateKey { .. }
            ) {
                content = content.child(
                    row()
                        .px_3()
                        .pb_2()
                        .gap_2()
                        .child(div().flex_1().child(Input::new(&self.password).small()))
                        .child(
                            action(
                                "remember",
                                if self.remember {
                                    text("✓ Save in system keychain", "✓ 保存到系统钥匙串")
                                } else {
                                    text("Save in system keychain", "保存到系统钥匙串")
                                },
                            )
                            .disabled(self.profile.ephemeral)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.remember = !this.remember && !this.profile.ephemeral;
                                cx.notify();
                            })),
                        ),
                );
            }
            content=content.child(div().px_3().pb_2().text_size(px(11.)).text_color(rgb(p.muted)).child(if self.profile.ephemeral {text("Temporary connection: credentials remain in memory.","临时连接：凭据仅保留在内存。")} else {text("Credentials stay in memory unless you select the system keychain. Empty password uses an existing keychain reference.","凭据仅保留在内存中，除非选择系统钥匙串。密码留空时使用已有钥匙串引用。")}));
        }
        if let Status::HostKey {
            fingerprint,
            previous,
        } = status
        {
            let changed = previous.is_some();
            content=content.child(div().m_3().p_3().border_1().border_color(rgb(p.warning)).rounded(px(6.)).flex().flex_col().gap_2()
                .child(div().text_color(rgb(p.warning)).child(if changed {text("WARNING: HOST KEY CHANGED","警告：主机密钥已更改")} else {text("Verify this host before connecting","连接前请验证主机身份")}))
                .child(self.session.as_ref().and_then(|s|s.state.lock().unwrap().hop.clone()).map(|h|h.label()).unwrap_or_else(||format!("{}:{}",self.profile.host,self.profile.port)))
                .children(previous.map(|old|div().text_size(px(11.)).child(format!("{}: {old}",text("Previous","原指纹")))))
                .child(div().text_size(px(11.)).child(format!("{}: {fingerprint}",text("Presented","新指纹"))))
                .child(div().text_size(px(11.)).child(text("Compare this SHA256 fingerprint with a trusted source. Accepting stores this host identity.","请通过可信渠道核对 SHA256 指纹。接受后将保存此主机身份。")))
                .child(row().gap_2().child(action("reject",text("Reject","拒绝")).on_click(cx.listener(|this,_,_,_|{if let Some(s)=&this.session {s.trust(false);}}))).child(action("trust",if changed {text("Replace trusted key","替换受信任密钥")} else {text("Trust and connect","信任并连接")}).on_click(cx.listener(|this,_,_,_|{if let Some(s)=&this.session {s.trust(true);}})))));
        }
        let prompt = self
            .session
            .as_ref()
            .and_then(|s| s.state.lock().unwrap().authentication.clone());
        if self.shown_prompt != prompt {
            self.auth_fields = prompt
                .as_ref()
                .map(|p| {
                    p.prompts
                        .iter()
                        .map(|(label, echo)| {
                            cx.new(|cx| {
                                InputState::new(window, cx)
                                    .masked(!echo)
                                    .placeholder(label.clone())
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            self.shown_prompt = prompt.clone();
        }
        if let Some(prompt) = prompt {
            let mut form = div()
                .m_3()
                .p_3()
                .border_1()
                .border_color(rgb(p.warning))
                .flex()
                .flex_col()
                .gap_2()
                .child(prompt.hop.label())
                .child(prompt.instructions);
            for (field, (label, _)) in self.auth_fields.iter().zip(prompt.prompts.iter()) {
                form = form
                    .child(div().text_size(px(11.)).child(label.clone()))
                    .child(Input::new(field).small());
            }
            content = content.child(form.child(
                action("answer-auth", text("Submit response", "提交认证")).on_click(cx.listener(
                    |this, _, window, cx| {
                        let answers = this
                            .auth_fields
                            .iter()
                            .map(|field| Secret::new(field.read(cx).value().to_string()))
                            .collect();
                        if let Some(session) = &this.session {
                            session.answer_auth(answers);
                        }
                        for field in &this.auth_fields {
                            field.update(cx, |s, cx| s.set_value("", window, cx));
                        }
                        cx.notify();
                    },
                )),
            ));
        }
        content = content.child(
            action(
                "network-config",
                if self.show_network {
                    text("Hide SSH networking", "收起 SSH 网络配置")
                } else {
                    text(
                        "SSH networking / jump hosts / forwarding",
                        "SSH 网络 / 跳板 / 转发",
                    )
                },
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.show_network = !this.show_network;
                cx.notify();
            })),
        );
        if self.show_network {
            use helixlane::workspace::TransportPolicy;
            let policy = self.profile.transport_policy;
            let mut network = div().px_3().py_2().flex().flex_col().gap_2().child(
                div().text_size(px(11.)).child(text(
                    "Transport policy (applies on Connect)",
                    "传输策略（下次连接生效）",
                )),
            );
            let mut policies = row().gap_2().flex_wrap();
            for (id, label, value) in [
                (
                    "shared",
                    text("Shared channels", "共享通道"),
                    TransportPolicy::Shared,
                ),
                (
                    "independent",
                    text("Independent connection", "独立连接"),
                    TransportPolicy::Independent,
                ),
                (
                    "single",
                    text("Single channel", "单通道"),
                    TransportPolicy::SingleChannel,
                ),
            ] {
                policies = policies.child(
                    action(
                        id,
                        format!("{}{}", if policy == value { "✓ " } else { "" }, label),
                    )
                    .disabled(busy || connected)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.profile.transport_policy = value;
                        cx.notify();
                    })),
                );
            }
            network=network.child(policies).child(div().text_size(px(11.)).child(text("Single-channel hosts refuse extra channels. Close the occupied channel or explicitly connect with Independent policy.","单通道主机拒绝额外通道。请关闭占用通道，或明确选择独立连接。")));
            if !busy && !connected {
                let mut jumps = row().gap_2().flex_wrap().child(
                    action("no-jump", text("Direct / no jump", "直连 / 无跳板")).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.profile.jump_resource = None;
                            cx.notify();
                        }),
                    ),
                );
                for resource in self
                    .settings
                    .borrow()
                    .workspace
                    .resources
                    .iter()
                    .filter(|r| !r.profiles.iter().any(|p| p.id == self.profile.id))
                {
                    let id = resource.id;
                    let label = format!(
                        "{}{} #{}",
                        if self.profile.jump_resource == Some(id) {
                            "✓ "
                        } else {
                            ""
                        },
                        resource.name,
                        id
                    );
                    jumps = jumps.child(
                        Button::new(("jump", id))
                            .ghost()
                            .small()
                            .label(label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.profile.jump_resource = Some(id);
                                cx.notify();
                            })),
                    );
                }
                network = network
                    .child(div().text_size(px(11.)).child(text(
                        "Jump resource (its own jump chain is included)",
                        "跳板资源（包含其自身跳板链）",
                    )))
                    .child(jumps);
            }
            if let Some(session) = &self.session {
                let route = session
                    .profile
                    .route_profiles()
                    .unwrap_or_default()
                    .iter()
                    .map(|p| format!("{}@{}:{}", p.username, p.host, p.port))
                    .collect::<Vec<_>>()
                    .join(" → ");
                network = network.child(div().text_size(px(11.)).child(route));
                if connected {
                    let labels = [
                        text("Local forward", "本地转发"),
                        text("Remote forward", "远程转发"),
                        text("SOCKS5 CONNECT", "SOCKS5 CONNECT"),
                    ];
                    network = network
                        .child(
                            action("forward-mode", labels[self.forward_mode as usize]).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.forward_mode = (this.forward_mode + 1) % 3;
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(Input::new(&self.forward_bind).small());
                    if self.forward_mode != 2 {
                        network = network.child(
                            row()
                                .gap_2()
                                .child(Input::new(&self.forward_host).small())
                                .child(Input::new(&self.forward_port).small()),
                        );
                    }
                    network = network.child(
                        action("start-forward", text("Start forwarding", "启动转发")).on_click(
                            cx.listener(|this, _, _, cx| {
                                use helixlane::ssh::forward::{Destination, ForwardSpec};
                                if let Some(session) = &this.session {
                                    let spec =
                                        this.forward_bind.read(cx).value().parse().ok().and_then(
                                            |bind| {
                                                if this.forward_mode == 2 {
                                                    return Some(ForwardSpec::Socks { bind });
                                                }
                                                let port = this
                                                    .forward_port
                                                    .read(cx)
                                                    .value()
                                                    .parse::<u16>()
                                                    .ok()?;
                                                let target = Destination {
                                                    host: this
                                                        .forward_host
                                                        .read(cx)
                                                        .value()
                                                        .to_string(),
                                                    port,
                                                };
                                                Some(if this.forward_mode == 0 {
                                                    ForwardSpec::Local { bind, target }
                                                } else {
                                                    ForwardSpec::Remote { bind, target }
                                                })
                                            },
                                        );
                                    match spec {
                                        Some(spec) => {
                                            this.owned_forwards.push(
                                                Service::global().forward(session.clone(), spec),
                                            );
                                        }
                                        _ => {
                                            let mut state = session.state.lock().unwrap();
                                            state.input_notice = Some(
                                                "Invalid forwarding address / 转发地址无效".into(),
                                            );
                                            state.revision += 1;
                                        }
                                    }
                                }
                                cx.notify();
                            }),
                        ),
                    );
                }
                for job in session.forwards() {
                    use helixlane::ssh::forward::{ForwardSpec, ForwardStatus};
                    let kind = match &job.spec {
                        ForwardSpec::Local { .. } => text("Local", "本地"),
                        ForwardSpec::Remote { .. } => text("Remote", "远程"),
                        ForwardSpec::Socks { .. } => "SOCKS5",
                    };
                    let destination = match &job.spec {
                        ForwardSpec::Local { target, .. } | ForwardSpec::Remote { target, .. } => {
                            format!(" → {}:{}", target.host, target.port)
                        }
                        _ => String::new(),
                    };
                    let state = match &*job.status.lock().unwrap() {
                        ForwardStatus::Starting => text("Starting…", "启动中…").to_string(),
                        ForwardStatus::Listening(address) => {
                            format!("{} {address}", text("Listening", "监听中"))
                        }
                        ForwardStatus::Stopped => text("Stopped", "已停止").into(),
                        ForwardStatus::Failed(error) => {
                            format!("{}: {}", text("Failed", "失败"), language.ssh_error(error))
                        }
                    };
                    let status = format!("#{} {kind} · {state}{destination}", job.id);
                    if matches!(job.spec, ForwardSpec::Remote { .. }) {
                        network = network.child(div().text_size(px(11.)).text_color(rgb(p.muted)).child(text(
                            "If remote Stop is not confirmed, this shared SSH connection closes, ending its other work. Independent connections stay open.",
                            "若远程停止未获确认，此共享 SSH 连接将关闭，并结束其上的其他任务。独立连接保持打开。",
                        )));
                    }
                    network = network.child(
                        row()
                            .gap_2()
                            .child(div().flex_1().text_size(px(11.)).child(status))
                            .child(
                                Button::new(("stop-forward", job.id))
                                    .ghost()
                                    .small()
                                    .label(text("Stop", "停止"))
                                    .on_click(move |_, _, _| job.stop()),
                            ),
                    );
                }
            }
            content = content.child(network);
        }
        if self.transport_only {
            return content.into_any_element();
        }
        let query = self.search.read(cx).value().to_string();
        if self.session.is_some() {
            content=content.child(row().px_3().py_1().gap_2().child(div().w(px(230.)).child(Input::new(&self.search).small())).child(div().text_size(px(10.)).text_color(rgb(p.muted)).child(if cfg!(target_os="macos") {text("⌘C / V / F · drag to select · sessions stay open when views close","⌘C / V / F · 拖动选择 · 关闭视图后会话继续运行")} else {text("Ctrl+Shift+C / V / F · drag to select · sessions stay open when views close","Ctrl+Shift+C / V / F · 拖动选择 · 关闭视图后会话继续运行")})));
        }
        let bounds = self.bounds.clone();
        let session = self.session.clone();
        let layout_bounds = bounds.clone();
        let layout_session = session.clone();
        let input_entity = cx.entity();
        let input_focus = self.focus.clone();
        let mut screen = div()
            .id("terminal-screen")
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus, cx);
                    cx.stop_propagation();
                    this.selecting = Some(this.position(event.position));
                    if let Some(session) = &this.session {
                        let point = this.selecting.unwrap();
                        session.state.lock().unwrap().terminal.select(point, point);
                    }
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if event.pressed_button != Some(MouseButton::Left) {
                    this.selecting = None;
                    return;
                }
                if let Some(start) = this.selecting
                    && let Some(session) = &this.session
                {
                    let end = this.position(event.position);
                    session.state.lock().unwrap().terminal.select(start, end);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.selecting = None;
                    cx.notify();
                }),
            )
            .on_scroll(cx.listener(|this, event: &ScrollEvent, _, cx| {
                if let Some(session) = &this.session {
                    let lines = match event.delta {
                        ScrollDelta::Lines(delta) => delta.y as i32,
                        ScrollDelta::Pixels(delta) => (f32::from(delta.y) / 20.) as i32,
                    };
                    session.state.lock().unwrap().terminal.scroll(lines);
                    cx.notify();
                }
            }))
            .child(
                canvas(
                    move |bounds, _, _| {
                        *layout_bounds.borrow_mut() = bounds;
                        if let Some(session) = &layout_session {
                            let cols = (f32::from(bounds.size.width) / 8.4).floor().max(2.) as u32;
                            let rows = (f32::from(bounds.size.height) / 20.).floor().max(1.) as u32;
                            let state = session.state.lock().unwrap();
                            if state.status == Status::Connected
                                && state.terminal_status == TerminalStatus::Running
                                && state.terminal.dimensions() != (cols as usize, rows as usize)
                            {
                                drop(state);
                                session.resize(cols, rows);
                            }
                        }
                    },
                    move |bounds, _, window, cx| {
                        window.handle_input(
                            &input_focus,
                            ElementInputHandler::new(bounds, input_entity),
                            cx,
                        );
                    },
                )
                .absolute()
                .size_full(),
            );
        if let Some(session) = session {
            let state = session.state.lock().unwrap();
            let cursor = state.terminal.cursor();
            let cells = state.terminal.cells();
            let mut grid = div()
                .absolute()
                .top_0()
                .left_0()
                .font_family("monospace")
                .text_size(px(14.))
                .line_height(px(20.));
            for (y, cells) in cells.into_iter().enumerate() {
                let highlighted = !query.is_empty()
                    && cells
                        .iter()
                        .map(|c| c.text.as_str())
                        .collect::<String>()
                        .contains(&query);
                let mut line = row().h(px(20.)).items_start();
                let mut runs: Vec<(String, usize, u32, u32, bool)> = Vec::new();
                for cell in cells {
                    let x = cell.column;
                    let mut fg = color(cell.fg, p.text, p.editor);
                    let mut bg = color(cell.bg, p.text, p.editor);
                    if cell.inverse {
                        std::mem::swap(&mut fg, &mut bg);
                    }
                    if cursor == Some((x, y)) && terminal_running {
                        bg = p.accent;
                        fg = p.editor;
                    } else if highlighted || cell.selected {
                        bg = p.selected;
                    }
                    let width = if cell.wide { 2 } else { 1 };
                    if let Some((text, columns, last_fg, last_bg, bold)) = runs.last_mut()
                        && *last_fg == fg
                        && *last_bg == bg
                        && *bold == cell.bold
                    {
                        text.push_str(&cell.text);
                        *columns += width;
                    } else {
                        runs.push((cell.text, width, fg, bg, cell.bold));
                    }
                }
                for (text, columns, fg, bg, bold) in runs {
                    line = line.child(
                        div()
                            .w(px(8.4 * columns as f32))
                            .flex_none()
                            .h(px(20.))
                            .text_color(rgb(fg))
                            .bg(rgb(bg))
                            .when(bold, |d| d.font_weight(FontWeight::BOLD))
                            .child(text),
                    );
                }
                grid = grid.child(line);
            }
            screen = screen.child(grid);
            if !self.preedit.is_empty()
                && let Some((col, row)) = cursor
            {
                screen = screen.child(
                    div()
                        .absolute()
                        .left(px(col as f32 * 8.4))
                        .top(px(row as f32 * 20.))
                        .bg(rgb(p.editor))
                        .text_color(rgb(p.text))
                        .font_family("monospace")
                        .text_size(px(14.))
                        .border_b_1()
                        .border_color(rgb(p.accent))
                        .child(self.preedit.clone()),
                );
            }
        } else {
            screen=screen.child(div().p_5().text_size(px(12.)).text_color(rgb(p.muted)).child(text("Connect to open an interactive SSH shell. Host identity must be approved on first contact.","连接后可打开交互式 SSH 终端。首次连接必须验证主机身份。")));
        }
        content.child(screen).into_any_element()
    }
}
fn color(color: Color, foreground: u32, background: u32) -> u32 {
    const ANSI: [u32; 16] = [
        0x202125, 0xe66d75, 0x8fc77c, 0xe5c07b, 0x78a9f8, 0xc792ea, 0x7fcac3, 0xcdd0d8, 0x767c88,
        0xff8a92, 0xaddd96, 0xffdda0, 0x9bc2ff, 0xe1acff, 0x9de4dc, 0xffffff,
    ];
    match color {
        Color::Spec(c) => (u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b),
        Color::Named(NamedColor::Foreground) => foreground,
        Color::Named(NamedColor::Background) => background,
        Color::Named(c) => ANSI[(c as usize).min(15)],
        Color::Indexed(i) if i < 16 => ANSI[i as usize],
        Color::Indexed(i) if i >= 232 => {
            let gray = 8 + u32::from(i - 232) * 10;
            gray * 0x010101
        }
        Color::Indexed(i) => {
            let n = i - 16;
            let component = |v: u8| if v == 0 { 0 } else { 55 + 40 * u32::from(v) };
            (component(n / 36) << 16) | (component((n / 6) % 6) << 8) | component(n % 6)
        }
    }
}
