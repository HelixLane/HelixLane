//! Native file tool. Async results carry a binding generation; jobs retain captured endpoints.
use crate::{terminal_ui::TerminalPane, theme::*};
use gpui_kit::{
    assets::IconName,
    base::Disableable,
    component::{
        Sizable,
        button::{Button, ButtonVariants},
        input::{Input, InputEvent, InputState},
        menu::{DropdownMenu, PopupMenuItem},
    },
    prelude::FluentBuilder as _,
    *,
};
use helixlane::{
    i18n::Language,
    settings::Settings,
    sftp::{
        Endpoint, Kind,
        browser::{Browser, Sort},
        editor::{EditLease, EditorManager},
    },
    tasks::{self, Conflict, JobState, TransferManager, TransferSpec},
    workspace::ConnectionProfile,
};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};
#[derive(Clone)]
struct MutationTarget {
    endpoint: Endpoint,
    path: String,
    selected: Vec<String>,
    generation: u64,
    remote: bool,
}
struct State {
    generation: u64,
    session_id: Option<u64>,
    attempted: Option<u64>,
    form: Option<&'static str>,
    pending: Option<MutationTarget>,
    canonical_path_pending: Option<String>,
    endpoint: Option<Endpoint>,
    local: Browser,
    remote: Browser,
    busy: bool,
    error: Option<String>,
    notice: Option<String>,
    conflict: Conflict,
    delete: bool,
    remote_active: bool,
}
fn file_views() -> &'static Mutex<std::collections::BTreeMap<u64, std::sync::Weak<Mutex<State>>>> {
    static VIEWS: std::sync::OnceLock<
        Mutex<std::collections::BTreeMap<u64, std::sync::Weak<Mutex<State>>>>,
    > = std::sync::OnceLock::new();
    VIEWS.get_or_init(|| Mutex::new(Default::default()))
}
pub struct FilePane {
    id: u64,
    visible: bool,
    window_visible: bool,
    last_refresh: std::time::Instant,
    visibility: Subscription,
    ticket: Option<helixlane::workspace::TargetTicket>,
    settings: Rc<RefCell<Settings>>,
    profile: ConnectionProfile,
    auth: Entity<TerminalPane>,
    state: Arc<Mutex<State>>,
    local_path: Entity<InputState>,
    remote_path: Entity<InputState>,
    name: Entity<InputState>,
    editor: Entity<InputState>,
    address_subscriptions: Vec<Subscription>,
}
fn btn(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Button {
    Button::new(id).ghost().small().label(label)
}
impl FilePane {
    pub fn new(
        id: u64,
        profile: ConnectionProfile,
        settings: Rc<RefCell<Settings>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let auth = cx.new(|cx| {
            let mut a = TerminalPane::new(id, profile.clone(), settings.clone(), window, cx);
            a.transport_only();
            a
        });
        let local = helixlane::sftp::local_home();
        let visibility = cx.observe_window_visibility(window, |this, visible, _, _| {
            this.window_visible = visible.is_visible()
        });
        let ticket = settings.borrow().workspace.ticket(id);
        let local_path = cx.new(|cx| InputState::new(window, cx).default_value(local.clone()));
        let remote_path = cx.new(|cx| InputState::new(window, cx).default_value("."));
        let address_subscriptions = vec![
            cx.subscribe_in(&local_path, window, |this, _, event: &InputEvent, w, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.navigate_address(false, w, cx);
                }
            }),
            cx.subscribe_in(
                &remote_path,
                window,
                |this, _, event: &InputEvent, w, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.navigate_address(true, w, cx);
                    }
                },
            ),
        ];

        let this = Self {
            id,
            visible: true,
            window_visible: window.is_visible(),
            last_refresh: std::time::Instant::now(),
            visibility,
            ticket,
            profile,
            auth,
            settings: settings.clone(),
            state: Arc::new(Mutex::new(State {
                generation: 0,
                session_id: None,
                attempted: None,
                form: None,
                pending: None,
                canonical_path_pending: None,
                endpoint: None,
                local: Browser::new(local.clone()),
                remote: Browser::remote(".".into()),
                busy: false,
                error: None,
                notice: None,
                conflict: Conflict::Rename,
                delete: false,
                remote_active: false,
            })),
            local_path,
            remote_path,
            address_subscriptions,
            name: cx.new(|cx| {
                InputState::new(window, cx).placeholder(
                    if settings.borrow().language == Language::Chinese {
                        "名称 / 八进制权限"
                    } else {
                        "Name / octal permissions"
                    },
                )
            }),
            editor: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(
                        std::env::var("VISUAL")
                            .or_else(|_| std::env::var("EDITOR"))
                            .unwrap_or_default(),
                    )
                    .placeholder(if settings.borrow().language == Language::Chinese {
                        "外部编辑器可执行文件"
                    } else {
                        "External editor executable"
                    })
            }),
        };
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                let alive = cx
                    .update(|window, cx| {
                        this.update(cx, |this, cx| {
                            this.sync_binding(window, cx);
                            this.poll(cx);
                            cx.notify();
                        })
                    })
                    .map(|result| result.is_ok())
                    .unwrap_or(false);
                if !alive {
                    break;
                }
            }
        })
        .detach();
        file_views()
            .lock()
            .unwrap()
            .insert(id, Arc::downgrade(&this.state));
        this.refresh(false);
        this
    }
    pub fn launch(&mut self, request: helixlane::launch::OpenRequest, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path)=request.remote_path {
            if self.state.lock().unwrap().endpoint.is_some() { self.set_path(true,path,window,cx); }
            else {self.state.lock().unwrap().remote=Browser::remote(path.clone());self.remote_path.update(cx,|s,cx|s.set_value(path,window,cx));}
        }
        self.auth.update(cx,|auth,cx|auth.launch(request.secret,window,cx));
        cx.notify();
    }
    fn sync_binding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let profile = {
            let settings = self.settings.borrow();
            settings
                .workspace
                .capture_task_target(self.id)
                .map(|t| t.profile)
        };
        if let Some(profile) = profile {
            self.bind(profile, window, cx);
        }
    }
    pub fn bind(
        &mut self,
        profile: ConnectionProfile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ticket = self.settings.borrow().workspace.ticket(self.id);
        if profile == self.profile && ticket == self.ticket {
            return;
        }
        let preserve_path=self.ticket==ticket;
        self.ticket = ticket;
        self.profile = profile.clone();
        self.auth.update(cx, |a, cx| a.bind(profile, window, cx));
        let mut s = self.state.lock().unwrap();
        s.generation += 1;
        s.endpoint = None;
        s.session_id = None;
        s.attempted = None;
        let path=if preserve_path{s.remote.path.clone()}else{".".into()};
        s.remote = Browser::remote(path.clone());
        s.busy = false;
        s.error = None;
        s.delete = false;
        s.form = None;
        s.pending = None;
        drop(s);
        self.remote_path
            .update(cx, |s, cx| s.set_value(path, window, cx));
    }
    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }
    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.local_path.read(cx).focus_handle(cx)
    }
    fn poll(&mut self, cx: &mut Context<Self>) {
        let session = self.auth.read(cx).shared_session();
        let should = {
            let settings = self.settings.borrow();
            settings
                .workspace
                .should_refresh(self.id, self.visible, !self.window_visible)
        };
        let selected_id = self.auth.read(cx).session_identity();
        let mut s = self.state.lock().unwrap();
        if s.session_id != selected_id {
            s.session_id = selected_id;
            s.generation += 1;
            s.attempted = None;
            s.endpoint = None;
            s.remote.entries.clear();
            s.busy = false;
        }
        if session.is_none() && s.endpoint.is_some() {
            s.endpoint = None;
            s.remote.entries.clear();
        }
        if should && s.endpoint.is_some() && self.last_refresh.elapsed() > Duration::from_secs(5) {
            self.last_refresh = std::time::Instant::now();
            drop(s);
            self.refresh(true);
            self.refresh(false);
            return;
        }
        if s.endpoint.is_none()
            && !s.busy
            && should
            && s.attempted != selected_id
            && let Some(session) = session
        {
            s.busy = true;
            s.attempted = selected_id;
            let generation = s.generation;
            let state = self.state.clone();
            let initial_remote_path=s.remote.path.clone();
            drop(s);
            tasks::runtime().spawn(async move {
                let result = async {
                    let endpoint = Endpoint::connect(session).await?;
                    let path = endpoint.canonicalize(&initial_remote_path).await?;
                    let entries = endpoint.list(&path).await?;
                    Ok::<_, String>((endpoint, entries, path))
                }
                .await;
                let mut s = state.lock().unwrap();
                if s.generation != generation {
                    return;
                }
                s.busy = false;
                match result {
                    Ok((e, entries, path)) => {
                        s.remote.path = path.clone();
                        s.canonical_path_pending = Some(path);
                        s.endpoint = Some(e);
                        s.remote.entries = entries;
                        s.notice = None;
                    }
                    Err(e) => s.error = Some(e),
                }
            });
        }
    }
    fn refresh(&self, remote: bool) {
        let (endpoint, path, generation, ticket) = {
            let mut s = self.state.lock().unwrap();
            let endpoint = if remote {
                let Some(e) = s.endpoint.clone() else {
                    s.attempted = None;
                    return;
                };
                e
            } else {
                Endpoint::Local
            };
            let ticket = if remote {
                s.remote.refresh_ticket()
            } else {
                s.local.refresh_ticket()
            };
            (
                endpoint,
                if remote {
                    s.remote.path.clone()
                } else {
                    s.local.path.clone()
                },
                s.generation,
                ticket,
            )
        };
        let state = self.state.clone();
        tasks::runtime().spawn(async move {
            let result = endpoint.list(&path).await;
            let mut s = state.lock().unwrap();
            if s.generation != generation {
                return;
            }
            let browser = if remote { &mut s.remote } else { &mut s.local };
            if browser.path != path || !browser.accepts(ticket) {
                return;
            }
            match result {
                Ok(entries) => browser.entries = entries,
                Err(e) => s.error = Some(e),
            }
        });
    }
    fn transfer(&self, upload: bool) {
        let state = self.state.clone();
        let (source, target, from, to, conflict, generation) = {
            let s = state.lock().unwrap();
            let Some(remote) = s.endpoint.clone() else {
                return;
            };
            let (browser, to) = if upload {
                (&s.local, s.remote.path.clone())
            } else {
                (&s.remote, s.local.path.clone())
            };
            let paths = browser.selected.iter().cloned().collect::<Vec<_>>();
            (
                if upload {
                    Endpoint::Local
                } else {
                    remote.clone()
                },
                if upload { remote } else { Endpoint::Local },
                paths,
                to,
                s.conflict,
                s.generation,
            )
        };
        tasks::runtime().spawn(async move {
            for path in from {
                match TransferSpec::into_directory(
                    source.clone(),
                    path.clone(),
                    target.clone(),
                    &to,
                    conflict,
                )
                .await
                {
                    Ok(spec) => {
                        TransferManager::global().submit(spec);
                        state.lock().unwrap().notice = Some("Submitted".into());
                    }
                    Err(e) => {
                        let mut s = state.lock().unwrap();
                        if s.generation == generation {
                            s.error = Some(e)
                        }
                    }
                }
            }
        });
    }
    fn open_form(&mut self, op: &'static str, w: &mut Window, cx: &mut Context<Self>) {
        let mut state = self.state.lock().unwrap();
        let remote = if op == "edit" {
            true
        } else {
            state.remote_active
        };
        let endpoint = if remote {
            let Some(endpoint) = state.endpoint.clone() else {
                state.error = Some("Connection not ready".into());
                return;
            };
            endpoint
        } else {
            Endpoint::Local
        };
        let browser = if remote { &state.remote } else { &state.local };
        let selected = browser.selected.iter().cloned().collect::<Vec<_>>();
        if op != "mkdir" && selected.is_empty() {
            state.error = Some("Select an entry first".into());
            return;
        }
        if matches!(op, "rename" | "edit") && selected.len() != 1 {
            state.error = Some("Select one entry".into());
            return;
        }
        let value = if op == "rename" {
            endpoint
                .basename(&selected[0])
                .unwrap_or_default()
                .to_string()
        } else if op == "chmod" {
            "644".into()
        } else {
            String::new()
        };
        state.pending = Some(MutationTarget {
            endpoint,
            path: browser.path.clone(),
            selected,
            generation: state.generation,
            remote,
        });
        state.form = Some(op);
        state.delete = op == "delete";
        state.error = None;
        drop(state);
        if op != "edit" {
            self.name.update(cx, |s, cx| s.set_value(value, w, cx));
        }
        cx.notify();
    }
    fn cross_copy(&self, destination: Endpoint, to: String, generation: u64, conflict: Conflict) {
        let (source, paths) = {
            let mut state = self.state.lock().unwrap();
            if state.generation != generation {
                state.error = Some("Target changed; reopen the action".into());
                return;
            }
            let Some(source) = state.endpoint.clone() else {
                return;
            };
            (
                source,
                state.remote.selected.iter().cloned().collect::<Vec<_>>(),
            )
        };
        let state = self.state.clone();
        tasks::runtime().spawn(async move {
            for from in paths {
                match TransferSpec::into_directory(
                    source.clone(),
                    from.clone(),
                    destination.clone(),
                    &to,
                    conflict,
                )
                .await
                {
                    Ok(spec) => {
                        TransferManager::global().submit(spec);
                    }
                    Err(e) => {
                        let mut s = state.lock().unwrap();
                        if s.generation == generation {
                            s.error = Some(e)
                        }
                    }
                }
            }
        });
    }
    fn copy_menu(&self, language: Language, cx: &mut Context<Self>) -> impl IntoElement {
        let zh = language == Language::Chinese;
        let mut targets = vec![];
        let settings = self.settings.borrow();
        for (id, weak) in file_views().lock().unwrap().iter() {
            if *id == self.id {
                continue;
            }
            let Some(state) = weak.upgrade() else {
                continue;
            };
            let state = state.lock().unwrap();
            let Some(endpoint) = state.endpoint.clone() else {
                continue;
            };
            let profile = settings
                .workspace
                .target(*id)
                .and_then(|id| settings.workspace.resources.iter().find(|r| r.id == id))
                .and_then(|r| r.profiles.first());
            if !matches!((&endpoint,profile),(Endpoint::Remote{transport,..},Some(profile)) if transport.profile==*profile)
            {
                continue;
            }
            targets.push((*id, endpoint, state.remote.path.clone()));
        }
        let (generation, conflict) = {
            let state = self.state.lock().unwrap();
            (state.generation, state.conflict)
        };
        let owner = cx.entity().downgrade();
        btn(
            "copy-to-view",
            if zh {
                "复制到服务器…"
            } else {
                "Copy to server…"
            },
        )
        .dropdown_menu(move |mut menu, _, _| {
            if targets.is_empty() {
                return menu.label(if zh {
                    "请先在其他文件视图连接目标服务器"
                } else {
                    "Connect a destination in another file view"
                });
            }
            for (id, endpoint, path) in &targets {
                let endpoint = endpoint.clone();
                let path = path.clone();
                let weak = owner.clone();
                menu = menu.item(
                    PopupMenuItem::new(format!("{} · {} · {}", id, endpoint.label(), path))
                        .on_click(move |_, _, cx| {
                            let _ = weak.update(cx, |this, _| {
                                this.cross_copy(
                                    endpoint.clone(),
                                    path.clone(),
                                    generation,
                                    conflict,
                                )
                            });
                        }),
                );
            }
            menu
        })
    }
    fn operation(&self, op: &str, value: String) {
        let state = self.state.clone();
        let captured = {
            let mut s = state.lock().unwrap();
            let Some(target) = s.pending.clone() else {
                return;
            };
            if target.generation != s.generation {
                s.error = Some("Target changed; reopen the action".into());
                return;
            }
            target
        };
        let MutationTarget {
            endpoint,
            path,
            selected,
            generation,
            ..
        } = captured;
        let op = op.to_string();
        tasks::runtime().spawn(async move {
            let result = async {
                match op.as_str() {
                    "mkdir" => {
                        if value.is_empty() || value.contains('/') || value == "." || value == ".."
                        {
                            return Err("Invalid directory name".into());
                        }
                        endpoint
                            .mkdir(&endpoint.checked_child_path(&path, &value)?)
                            .await?
                    }
                    "rename" => {
                        if selected.len() != 1
                            || value.is_empty()
                            || value.contains('/')
                            || value == "."
                            || value == ".."
                        {
                            return Err("Select one entry and enter a valid name".into());
                        }
                        let to = endpoint.checked_child_path(&path, &value)?;
                        if endpoint.stat(&to).await?.is_some() {
                            return Err("Target exists".into());
                        }
                        endpoint.rename(&selected[0], &to).await?
                    }
                    "chmod" => {
                        let mode = u32::from_str_radix(value.trim(), 8)
                            .map_err(|_| "Enter octal permissions, e.g. 644".to_string())?;
                        for p in selected {
                            endpoint.chmod(&p, mode).await?;
                        }
                    }
                    "delete" => {
                        for p in selected {
                            delete_tree(&endpoint, &p).await?;
                        }
                    }
                    _ => {}
                }
                Ok::<(), String>(())
            }
            .await;
            let mut s = state.lock().unwrap();
            if s.generation != generation {
                return;
            }
            match result {
                Ok(()) => {
                    s.notice = Some("Completed".into());
                    s.error = None;
                }
                Err(e) => s.error = Some(e),
            }
        });
    }
    fn edit(&self, program: String) {
        let state = self.state.clone();
        let (endpoint, path, generation) = {
            let mut s = state.lock().unwrap();
            let Some(target) = s.pending.clone() else {
                return;
            };
            if target.generation != s.generation {
                s.error = Some("Target changed; reopen the action".into());
                return;
            }
            if target.selected.len() != 1 {
                return;
            }
            (
                target.endpoint,
                target.selected[0].clone(),
                target.generation,
            )
        };
        tasks::runtime().spawn(async move {
            let result = async {
                let lease = EditLease::download(
                    endpoint,
                    path,
                    &std::env::temp_dir().join("helixlane-edits"),
                )
                .await?;
                lease.open(&program)?;
                Ok::<_, String>(Arc::new(lease))
            }
            .await;
            let mut s = state.lock().unwrap();
            if s.generation != generation {
                if let Ok(lease) = result {
                    EditorManager::global().register(lease)
                }
                return;
            }
            match result {
                Ok(lease) => {
                    EditorManager::global().register(lease);
                    s.notice = Some("External editor opened".into());
                }
                Err(e) => s.error = Some(e),
            }
        });
    }
    fn toolbar(&self, remote: bool, lang: Language, cx: &mut Context<Self>) -> AnyElement {
        let zh = lang == Language::Chinese;
        let tiny = |id: &'static str, name: IconName, label: &'static str| {
            Button::new((id, remote as usize))
                .ghost()
                .xsmall()
                .icon(icon(name))
                .tooltip(label)
                .w(px(22.))
                .h(px(22.))
        };
        let owner = cx.entity().downgrade();
        let (hidden, sort) = {
            let state = self.state.lock().unwrap();
            let b = if remote { &state.remote } else { &state.local };
            (b.hidden, b.sort)
        };
        let menu = tiny(
            "browser-options",
            IconName::Ellipsis,
            if zh {
                "排序、隐藏文件和快捷路径"
            } else {
                "Sort, hidden files and shortcuts"
            },
        )
        .dropdown_menu(move |mut menu, _, _| {
            let weak = owner.clone();
            menu = menu.item(
                PopupMenuItem::new(if zh {
                    "显示隐藏文件"
                } else {
                    "Show hidden files"
                })
                .checked(hidden)
                .on_click(move |_, _, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        let mut s = this.state.lock().unwrap();
                        let b = if remote { &mut s.remote } else { &mut s.local };
                        b.hidden = !b.hidden;
                        cx.notify();
                    });
                }),
            );
            menu = menu.separator();
            for (choice, label) in [
                (
                    Sort::Name,
                    if zh {
                        "按名称排序"
                    } else {
                        "Sort by name"
                    },
                ),
                (
                    Sort::Size,
                    if zh {
                        "按大小排序"
                    } else {
                        "Sort by size"
                    },
                ),
                (
                    Sort::Modified,
                    if zh {
                        "按修改时间排序"
                    } else {
                        "Sort by modified"
                    },
                ),
            ] {
                let weak = owner.clone();
                menu = menu.item(PopupMenuItem::new(label).checked(choice == sort).on_click(
                    move |_, _, cx| {
                        let _ = weak.update(cx, |this, cx| {
                            let mut s = this.state.lock().unwrap();
                            if remote {
                                s.remote.sort = choice
                            } else {
                                s.local.sort = choice
                            }
                            cx.notify();
                        });
                    },
                ));
            }
            menu = menu.separator();
            for (label, path) in [
                (
                    if zh { "主目录" } else { "Home" },
                    if remote {
                        ".".into()
                    } else {
                        helixlane::sftp::local_home()
                    },
                ),
                (if zh { "根目录" } else { "Root" }, "/".into()),
                (
                    if zh {
                        "临时目录"
                    } else {
                        "Temporary files"
                    },
                    "/tmp".into(),
                ),
            ] {
                let weak = owner.clone();
                menu = menu.item(PopupMenuItem::new(label).on_click(move |_, w, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        let mut s = this.state.lock().unwrap();
                        s.delete = false;
                        let b = if remote { &mut s.remote } else { &mut s.local };
                        b.navigate(path.clone());
                        drop(s);
                        this.set_path(remote, path.clone(), w, cx);
                        this.refresh(remote);
                    });
                }));
            }
            menu
        });
        let navigation = row()
            .gap_1()
            .min_w_0()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(rgb(Palette::current(cx).muted))
                    .child(if remote {
                        if zh { "远程" } else { "Remote" }
                    } else {
                        if zh { "本地" } else { "Local" }
                    }),
            )
            .children(
                [
                    (
                        0usize,
                        IconName::ArrowLeft,
                        if zh { "返回" } else { "Back" },
                    ),
                    (1, IconName::ArrowRight, if zh { "前进" } else { "Forward" }),
                    (2, IconName::ArrowUp, if zh { "上级" } else { "Parent" }),
                ]
                .into_iter()
                .map(|(kind, icon_name, label)| {
                    tiny(
                        match kind {
                            0 => "back",
                            1 => "forward",
                            _ => "up",
                        },
                        icon_name,
                        label,
                    )
                    .on_click(cx.listener(move |this, _, w, cx| {
                        let mut s = this.state.lock().unwrap();
                        s.delete = false;
                        let b = if remote { &mut s.remote } else { &mut s.local };
                        match kind {
                            0 => b.back(),
                            1 => b.forward(),
                            _ => b.up(),
                        };
                        let path = b.path.clone();
                        drop(s);
                        this.set_path(remote, path, w, cx);
                        this.refresh(remote);
                    }))
                }),
            )
            .child(
                tiny(
                    "refresh",
                    IconName::RefreshCw,
                    if zh { "刷新" } else { "Refresh" },
                )
                .on_click(cx.listener(move |this, _, _, _| this.refresh(remote))),
            )
            .child(menu);
        let input = if remote {
            &self.remote_path
        } else {
            &self.local_path
        };
        let address = Input::new(input)
            .small()
            .aria_label(if remote {
                if zh { "远程地址" } else { "Remote address" }
            } else {
                if zh { "本地地址" } else { "Local address" }
            })
            .suffix(
                tiny(
                    "go",
                    IconName::ArrowRight,
                    if zh { "前往地址" } else { "Go to address" },
                )
                .on_click(cx.listener(move |this, _, w, cx| {
                    this.navigate_address(remote, w, cx);
                })),
            );
        div()
            .flex()
            .flex_col()
            .min_w_0()
            .gap_1()
            .child(navigation)
            .child(address)
            .into_any_element()
    }
    fn navigate_address(&mut self, remote: bool, w: &mut Window, cx: &mut Context<Self>) {
        let path = if remote {
            self.remote_path.read(cx)
        } else {
            self.local_path.read(cx)
        }
        .value()
        .to_string();
        if path.is_empty() || path.contains('\0') {
            self.state.lock().unwrap().error = Some("Invalid path".into());
            cx.notify();
            return;
        }
        let mut state = self.state.lock().unwrap();
        state.remote_active = remote;
        state.delete = false;
        let browser = if remote {
            &mut state.remote
        } else {
            &mut state.local
        };
        browser.navigate(path.clone());
        drop(state);
        self.set_path(remote, path, w, cx);
        self.refresh(remote);
    }
    fn set_path(&self, remote: bool, path: String, w: &mut Window, cx: &mut Context<Self>) {
        let field = if remote {
            &self.remote_path
        } else {
            &self.local_path
        };
        field.update(cx, |s, cx| s.set_value(path, w, cx));
    }
    fn pane(
        &self,
        remote: bool,
        language: Language,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let zh = language == Language::Chinese;
        let (entries, selected, active, connected) = {
            let state = self.state.lock().unwrap();
            let browser = if remote { &state.remote } else { &state.local };
            (
                browser.visible(),
                browser.selected.clone(),
                state.remote_active == remote,
                state.endpoint.is_some(),
            )
        };
        let mut list = div()
            .id(("file-list", remote as usize))
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .min_w_0();
        if entries.is_empty() {
            list = list.child(div().p_2().text_color(rgb(p.muted)).child(
                if remote && !connected {
                    if zh {
                        "连接后浏览远程文件"
                    } else {
                        "Connect to browse remote files"
                    }
                } else {
                    if zh {
                        "目录为空"
                    } else {
                        "Directory is empty"
                    }
                },
            ));
        }
        for (index, entry) in entries.into_iter().enumerate() {
            let path = entry.path.clone();
            let is_selected = selected.contains(&path);
            let directory = entry.kind == Kind::Directory;
            let mut item = row()
                .id((if remote { "remote-file" } else { "local-file" }, index))
                .min_w_0()
                .gap_1()
                .px_1()
                .h(px(26.))
                .bg(rgb(if is_selected { p.selected } else { p.panel }))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    let mut state = this.state.lock().unwrap();
                    state.remote_active = remote;
                    state.delete = false;
                    let browser = if remote {
                        &mut state.remote
                    } else {
                        &mut state.local
                    };
                    if !browser.selected.remove(&path) {
                        browser.selected.insert(path.clone());
                    }
                    cx.notify();
                }))
                .child(div().flex_1().min_w_0().truncate().child(format!(
                    "{} {}",
                    match entry.kind {
                        Kind::Directory => "▸",
                        Kind::Symlink => "↗",
                        _ => "·",
                    },
                    entry.name
                )))
                .child(
                    div()
                        .flex_none()
                        .text_size(px(10.))
                        .text_color(rgb(p.muted))
                        .child(format!("{} {:o}", entry.size, entry.permissions & 0o7777)),
                );
            if directory {
                item = item.child(
                    Button::new(("open-directory", index))
                        .ghost()
                        .xsmall()
                        .icon(icon(IconName::ChevronRight))
                        .tooltip(if zh { "打开目录" } else { "Open directory" })
                        .w(px(20.))
                        .h(px(20.))
                        .on_click(cx.listener(move |this, _, w, cx| {
                            cx.stop_propagation();
                            let mut state = this.state.lock().unwrap();
                            state.delete = false;
                            let browser = if remote {
                                &mut state.remote
                            } else {
                                &mut state.local
                            };
                            browser.navigate(entry.path.clone());
                            drop(state);
                            this.set_path(remote, entry.path.clone(), w, cx);
                            this.refresh(remote);
                        })),
                )
            }
            list = list.child(item);
        }
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .border_1()
            .border_color(rgb(if active { p.accent } else { p.border }))
            .rounded(px(7.))
            .p_1()
            .gap_1()
            .child(self.toolbar(remote, language, cx))
            .child(list)
            .into_any_element()
    }
}
impl Drop for FilePane {
    fn drop(&mut self) {
        let mut views = file_views().lock().unwrap();
        if views
            .get(&self.id)
            .and_then(std::sync::Weak::upgrade)
            .is_some_and(|state| Arc::ptr_eq(&state, &self.state))
        {
            views.remove(&self.id);
        }
    }
}
fn delete_tree<'a>(
    e: &'a Endpoint,
    p: &'a str,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
    Box::pin(async move {
        if e.stat(p).await?.is_some_and(|e| e.kind == Kind::Directory) {
            for child in e.list(p).await? {
                delete_tree(e, &child.path).await?
            }
        }
        e.remove(p).await
    })
}
impl Render for FilePane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let canonical = self.state.lock().unwrap().canonical_path_pending.take();
        if let Some(path) = canonical {
            self.remote_path
                .update(cx, |s, cx| s.set_value(path, window, cx));
        }
        let _ = (&self.visibility, &self.address_subscriptions);
        let p = Palette::current(cx);
        let lang = self.settings.borrow().language;
        let zh = lang == Language::Chinese;
        let t = |en, cn| if zh { cn } else { en };
        let mut content = div()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .gap_2()
            .p_2()
            .child(div().flex_none().child(self.auth.clone()));
        let state = self.state.lock().unwrap();
        if let Some(e) = &state.error {
            content = content.child(div().text_color(rgb(p.warning)).child(format!(
                "{}: {}",
                t("Error", "错误"),
                lang.file_error(e)
            )));
        }
        if let Some(notice) = &state.notice {
            content = content.child(div().text_color(rgb(p.muted)).text_size(px(11.)).child(
                match notice.as_str() {
                    "Submitted" => t(
                        "Transfer submitted. Track it in Transfers.",
                        "传输已提交，可在传输管理器中查看。",
                    ),
                    "Completed" => t("Operation completed.", "操作已完成。"),
                    "External editor opened" => t(
                        "Editor opened. Saved changes require upload confirmation below.",
                        "编辑器已打开。保存更改后需在下方确认回传。",
                    ),
                    _ => "",
                },
            ));
        }
        if state.busy {
            content = content.child(t("Opening SFTP…", "正在打开 SFTP…"));
        }
        let conflict = state.conflict;
        let delete = state.delete;
        let form = state.form;
        let target_description = state.pending.as_ref().map(|target| {
            format!(
                "{} · {}:{}{}",
                if target.remote {
                    t("Remote", "远程")
                } else {
                    t("Local", "本地")
                },
                target.endpoint.label(),
                target.path,
                if target.selected.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", target.selected.join(", "))
                }
            )
        });
        let leases = EditorManager::global().leases().into_iter().filter(|lease|matches!(&lease.endpoint,Endpoint::Remote{transport,..} if transport.profile==self.profile)).collect::<Vec<_>>();
        drop(state);
        content = content
            .child(
                row()
                    .gap_2()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .child(self.pane(false, lang, p, cx))
                    .child(self.pane(true, lang, p, cx)),
            )
            .child(
                row()
                    .flex_wrap()
                    .gap_1()
                    .child(
                        btn("upload", t("Upload →", "上传 →"))
                            .on_click(cx.listener(|this, _, _, _| this.transfer(true))),
                    )
                    .child(
                        btn("download", t("← Download", "← 下载"))
                            .on_click(cx.listener(|this, _, _, _| this.transfer(false))),
                    )
                    .child(self.copy_menu(lang, cx))
                    .child(
                        div()
                            .text_color(rgb(p.muted))
                            .child(t("Conflict:", "冲突：")),
                    )
                    .children(
                        [
                            (Conflict::Overwrite, t("Overwrite", "覆盖")),
                            (Conflict::Skip, t("Skip", "跳过")),
                            (Conflict::Rename, t("Rename", "重命名")),
                        ]
                        .into_iter()
                        .enumerate()
                        .map(|(i, (c, label))| {
                            btn(
                                ("conflict", i),
                                format!(
                                    "{}{}",
                                    if std::mem::discriminant(&c)
                                        == std::mem::discriminant(&conflict)
                                    {
                                        "✓ "
                                    } else {
                                        ""
                                    },
                                    label
                                ),
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.state.lock().unwrap().conflict = c;
                                    cx.notify();
                                },
                            ))
                        }),
                    ),
            );
        content = content.child(
            row()
                .flex_wrap()
                .gap_1()
                .children(
                    [
                        ("mkdir", t("New folder", "新建目录")),
                        ("rename", t("Rename", "重命名")),
                        ("chmod", t("Permissions", "权限")),
                    ]
                    .into_iter()
                    .map(|(op, label)| {
                        btn(op, label)
                            .on_click(cx.listener(move |this, _, w, cx| this.open_form(op, w, cx)))
                    }),
                )
                .child(
                    btn(
                        "delete",
                        if delete {
                            t("Confirm delete selected", "确认删除所选项")
                        } else {
                            t("Delete…", "删除…")
                        },
                    )
                    .on_click(cx.listener(|this, _, w, cx| this.open_form("delete", w, cx))),
                ),
        );
        content = content.child(
            btn("external-edit", t("External edit…", "外部编辑…"))
                .on_click(cx.listener(|this, _, w, cx| this.open_form("edit", w, cx))),
        );
        if let Some(op) = form {
            let label = match op {
                "mkdir" => t("Folder name", "目录名称"),
                "rename" => t("New name", "新名称"),
                "chmod" => t("Octal permissions (e.g. 644)", "八进制权限（例如 644）"),
                "delete" => t(
                    "Delete selected files and directories?",
                    "删除所选文件和目录？",
                ),
                _ => t("External editor executable", "外部编辑器可执行文件"),
            };
            let mut form = div()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .border_1()
                .border_color(rgb(p.border))
                .rounded(px(6.))
                .child(
                    div()
                        .text_size(px(11.))
                        .child(target_description.unwrap_or_default()),
                )
                .child(div().text_color(rgb(p.muted)).child(label));
            let mut actions = row().gap_2().min_w_0();
            if op != "delete" {
                actions = actions.child(
                    div().flex_1().min_w_0().child(
                        Input::new(if op == "edit" {
                            &self.editor
                        } else {
                            &self.name
                        })
                        .small(),
                    ),
                );
            }
            actions = actions
                .child(
                    btn("confirm-file-action", t("Confirm", "确认")).on_click(cx.listener(
                        move |this, _, _, cx| {
                            if op == "edit" {
                                this.edit(this.editor.read(cx).value().to_string());
                            } else {
                                this.operation(op, this.name.read(cx).value().to_string());
                            }
                            let mut s = this.state.lock().unwrap();
                            s.form = None;
                            s.delete = false;
                            cx.notify();
                        },
                    )),
                )
                .child(
                    btn("cancel-file-action", t("Cancel", "取消")).on_click(cx.listener(
                        |this, _, _, cx| {
                            let mut s = this.state.lock().unwrap();
                            s.form = None;
                            s.pending = None;
                            s.delete = false;
                            cx.notify();
                        },
                    )),
                );
            form = form.child(actions);
            content = content.child(form);
        }
        for (i, lease) in leases.into_iter().enumerate() {
            if let Some(error) = lease.last_error.lock().unwrap().as_ref() {
                content = content.child(div().text_color(rgb(p.warning)).text_size(px(11.)).child(
                    format!(
                        "{}: {} · {}:{}",
                        t("Editor error", "编辑器错误"),
                        lang.file_error(error),
                        lease.endpoint.label(),
                        lease.remote
                    ),
                ));
            }
            content = content.child(
                row()
                    .gap_1()
                    .child(div().flex_1().text_size(px(10.)).child(format!(
                        "{} → {}",
                        format_args!(
                            "{}:{} · {}",
                            lease.endpoint.label(),
                            lease.remote,
                            if lease.dirty.load(std::sync::atomic::Ordering::Acquire) {
                                t("Saved changes ready", "已检测到保存更改")
                            } else {
                                t("Watching editor saves", "正在检测编辑器保存")
                            }
                        ),
                        lease.local.display()
                    )))
                    .child(
                        btn(
                            ("upload-edit", i),
                            t("Check save and confirm upload", "检查保存并确认回传"),
                        )
                        .on_click(cx.listener(move |this, _, _, _| {
                            let lease = lease.clone();
                            let state = this.state.clone();
                            tasks::runtime().spawn(async move {
                                let result = async {
                                    if !lease.saved().await? {
                                        return Err("No saved changes detected".into());
                                    }
                                    lease
                                        .upload_confirmed(TransferManager::global().clone())
                                        .await?;
                                    Ok::<(), String>(())
                                }
                                .await;
                                if let Err(e) = result {
                                    state.lock().unwrap().error = Some(e)
                                }
                            });
                        })),
                    ),
            );
        }
        content.into_any_element()
    }
}

pub fn transfers(language: Language, cx: &mut Context<impl Sized>) -> AnyElement {
    let zh = language == Language::Chinese;
    let palette = Palette::current(cx);
    let jobs = TransferManager::global().jobs();
    let mut content = div()
        .id("transfers-list")
        .overflow_y_scroll()
        .size_full()
        .p_3()
        .flex()
        .flex_col()
        .gap_2();
    if jobs.is_empty() {
        content = content.child(language.text("no_tasks"));
    }
    for job in jobs {
        let id = job.id;
        let state = match &job.state {
            JobState::Queued => if zh { "等待中" } else { "Queued" }.into(),
            JobState::Running => if zh { "传输中" } else { "Running" }.into(),
            JobState::Complete => if zh { "完成" } else { "Complete" }.into(),
            JobState::Skipped => if zh { "已跳过" } else { "Skipped" }.into(),
            JobState::Cancelled => if zh { "已取消" } else { "Cancelled" }.into(),
            JobState::Failed(e) => format!(
                "{}: {}",
                if zh { "失败" } else { "Failed" },
                language.file_error(e)
            ),
        };
        let progress = if job.total == 0 {
            0.0
        } else {
            (job.bytes as f32 / job.total as f32).clamp(0., 1.)
        };
        content = content.child(
            div()
                .p_2()
                .border_1()
                .rounded(px(6.))
                .flex()
                .flex_col()
                .gap_1()
                .child(format!("#{} {} → {}", id, job.source, job.target))
                .child(format!(
                    "{} / {} B · {} · {} {} · {} {}",
                    job.bytes,
                    job.total,
                    state,
                    if zh { "尝试" } else { "attempts" },
                    job.attempts,
                    if zh { "跳过链接" } else { "skipped links" },
                    job.skipped_links
                ))
                .child(
                    div()
                        .w_full()
                        .h(px(3.))
                        .bg(rgb(palette.border))
                        .rounded(px(2.))
                        .child(div().h_full().w(relative(progress)).bg(rgb(palette.accent))),
                )
                .children(job.staging.map(|p| {
                    div().child(format!(
                        "{}: {p} · {} B",
                        if zh { "本机中转" } else { "Local staging" },
                        job.staging_bytes
                    ))
                }))
                .child(
                    row()
                        .gap_2()
                        .child(
                            btn(("cancel-job", id), if zh { "取消" } else { "Cancel" })
                                .on_click(move |_, _, _| TransferManager::global().cancel(id)),
                        )
                        .when(
                            matches!(job.state, JobState::Failed(_) | JobState::Cancelled),
                            |d| {
                                d.child(
                                    btn(
                                        ("retry-job", id),
                                        if zh {
                                            "重新验证并重试"
                                        } else {
                                            "Revalidate and retry"
                                        },
                                    )
                                    .on_click(
                                        move |_, _, _| {
                                            let _ = TransferManager::global().retry(id);
                                        },
                                    ),
                                )
                            },
                        ),
                ),
        );
    }
    let _ = cx;
    content.into_any_element()
}

pub struct TransfersPane {
    settings: Rc<RefCell<Settings>>,
    error: Arc<Mutex<Option<String>>>,
}
impl TransfersPane {
    pub fn new(settings: Rc<RefCell<Settings>>, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            settings,
            error: Arc::new(Mutex::new(None)),
        }
    }
}
impl Render for TransfersPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let language = self.settings.borrow().language;
        let zh = language == Language::Chinese;
        let mut content = div().size_full().min_h_0().flex().flex_col();
        if let Some(e) = self
            .error
            .lock()
            .unwrap()
            .clone()
            .or_else(|| TransferManager::global().persistence_error())
        {
            content = content.child(div().p_2().child(format!(
                "{}: {}",
                if zh { "错误" } else { "Error" },
                language.file_error(&e)
            )));
        }
        for recovery in TransferManager::global().recoveries() {
            let identity = recovery.id.clone();
            let dismiss_identity = identity.clone();
            let claimed = recovery.claimed;
            content = content.child(
                div()
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(format!(
                        "{}: {} → {}",
                        if zh {
                            "待恢复传输"
                        } else {
                            "Recoverable transfer"
                        },
                        recovery.job.source,
                        recovery.job.target
                    ))
                    .child(
                        row()
                            .gap_2()
                            .child(
                                btn(
                                    SharedString::from(format!("recover-{identity}")),
                                    if claimed {
                                        if zh {
                                            "正在验证…"
                                        } else {
                                            "Validating…"
                                        }
                                    } else if zh {
                                        "重新验证并恢复"
                                    } else {
                                        "Revalidate and recover"
                                    },
                                )
                                .disabled(claimed)
                                .on_click(cx.listener(
                                    move |this, _, _, _| {
                                        let identity = identity.clone();
                                        let error = this.error.clone();
                                        tasks::runtime().spawn(async move {
                                            if let Err(e) =
                                                TransferManager::global().recover(&identity).await
                                            {
                                                *error.lock().unwrap() = Some(e)
                                            }
                                        });
                                    },
                                )),
                            )
                            .child(
                                btn(
                                    SharedString::from(format!("dismiss-{dismiss_identity}")),
                                    if zh { "忽略" } else { "Dismiss" },
                                )
                                .disabled(claimed)
                                .on_click(cx.listener(
                                    move |this, _, _, _| {
                                        if let Err(e) = TransferManager::global()
                                            .dismiss_recovery(&dismiss_identity)
                                        {
                                            *this.error.lock().unwrap() = Some(e)
                                        }
                                    },
                                )),
                            ),
                    ),
            );
        }
        content
            .child(div().flex_1().min_h_0().child(transfers(language, cx)))
            .into_any_element()
    }
}
