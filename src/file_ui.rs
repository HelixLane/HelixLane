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
        jobs::{JobScope, delete_tree},
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
// A launch navigates the real browser and retires every older path operation.
fn request_remote_path(state: &Arc<Mutex<State>>, path: String) {
    let mut s = state.lock().unwrap();
    s.generation += 1;
    s.remote.navigate(path);
    s.canonical_path_pending = None;
    s.busy = false;
    s.attempted = None;
    s.error = None;
    s.delete = false;
    s.pending = None;
    s.remote_active = true;
    drop(s);
    refresh_state(state, true);
}
struct InitialListing {
    generation: u64,
    path: String,
    revision: u64,
}
fn begin_initial_listing(s: &mut State) -> InitialListing {
    s.busy = true;
    s.attempted = s.session_id;
    InitialListing {
        generation: s.generation,
        path: s.remote.path.clone(),
        revision: s.remote.refresh_ticket(),
    }
}
fn finish_initial_listing(
    s: &mut State,
    ticket: InitialListing,
    result: Result<(Endpoint, Vec<helixlane::sftp::Entry>, String), String>,
) {
    if s.generation != ticket.generation
        || s.remote.path != ticket.path
        || !s.remote.accepts(ticket.revision)
    {
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
}
fn refresh_state(state: &Arc<Mutex<State>>, remote: bool) {
    let (endpoint, path, generation, ticket) = {
        let mut s = state.lock().unwrap();
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
    let state = state.clone();
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
fn file_views() -> &'static Mutex<std::collections::BTreeMap<u64, std::sync::Weak<Mutex<State>>>> {
    static VIEWS: std::sync::OnceLock<
        Mutex<std::collections::BTreeMap<u64, std::sync::Weak<Mutex<State>>>>,
    > = std::sync::OnceLock::new();
    VIEWS.get_or_init(|| Mutex::new(Default::default()))
}
pub struct FilePane {
    jobs: JobScope,
    polling: Option<Task<()>>,
    window_handle: Option<AnyWindowHandle>,
    local_scroll: ScrollHandle,
    remote_scroll: ScrollHandle,
    restore_scroll: Option<helixlane::workspace::ViewPageState>,
    id: u64,
    visible: bool,
    window_visible: bool,
    last_refresh: std::time::Instant,
    visibility: Subscription,
    ticket: Option<helixlane::workspace::TargetTicket>,
    binding_dirty: bool,
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
        let page = {
            let settings = settings.borrow();
            settings
                .workspace
                .views
                .iter()
                .find(|v| v.id == id && v.page.resource == settings.workspace.target(id))
                .map(|v| v.page.clone())
                .unwrap_or_default()
        };
        let local = page
            .local_path
            .clone()
            .unwrap_or_else(helixlane::sftp::local_home);
        let remote = page.remote_path.clone().unwrap_or_else(|| ".".into());
        let visibility = cx.observe_window_visibility(window, |this, visible, _, _| {
            if visible.is_visible() && !this.window_visible {
                this.last_refresh = std::time::Instant::now() - Duration::from_secs(6);
            }
            this.window_visible = visible.is_visible()
        });
        let ticket = settings.borrow().workspace.ticket(id);
        let local_path = cx.new(|cx| InputState::new(window, cx).default_value(local.clone()));
        let remote_path = cx.new(|cx| InputState::new(window, cx).default_value(remote.clone()));
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

        let mut this = Self {
            jobs: JobScope::default(),
            polling: None,
            window_handle: None,
            local_scroll: ScrollHandle::new(),
            remote_scroll: ScrollHandle::new(),
            restore_scroll: Some(page.clone()),
            id,
            visible: true,
            window_visible: window.is_visible(),
            last_refresh: std::time::Instant::now(),
            visibility,
            ticket,
            binding_dirty: false,
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
                local: {
                    let mut b = Browser::new(local.clone());
                    b.hidden = page.browser_hidden[0];
                    b.sort = page.browser_sort[0];
                    b
                },
                remote: {
                    let mut b = Browser::remote(remote);
                    b.hidden = page.browser_hidden[1];
                    b.sort = page.browser_sort[1];
                    b
                },
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
        this.start_polling(window, cx);
        file_views()
            .lock()
            .unwrap()
            .insert(id, Arc::downgrade(&this.state));
        this.refresh(false);
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
    pub fn page_state(&self) -> helixlane::workspace::ViewPageState {
        let state = self.state.lock().unwrap();
        let local = self.local_scroll.offset();
        let remote = self.remote_scroll.offset();
        helixlane::workspace::ViewPageState {
            resource: self.ticket.map(|t| t.resource_id),
            local_path: Some(state.local.path.clone()),
            remote_path: Some(state.remote.path.clone()),
            browser_hidden: [state.local.hidden, state.remote.hidden],
            browser_sort: [state.local.sort, state.remote.sort],
            local_scroll: [local.x.into(), local.y.into()],
            remote_scroll: [remote.x.into(), remote.y.into()],
        }
    }
    fn persist_page(&self, cx: &App) {
        let page = self.page_state();
        if let Some(view) = self
            .settings
            .borrow_mut()
            .workspace
            .views
            .iter_mut()
            .find(|v| v.id == self.id)
        {
            view.page = page;
        }
        if let Some(owner) = cx.try_global::<crate::application::ApplicationOwner>() {
            let _ = owner.save();
        }
    }
    pub fn cancel_owned_jobs(&mut self, cx: &mut Context<Self>) {
        self.jobs.cancel();
        self.auth.update(cx, |a, _| a.cancel_owned_jobs());
    }
    pub fn rebind_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.is_visible() && !self.window_visible {
            self.last_refresh = std::time::Instant::now() - Duration::from_secs(6);
        }
        self.window_visible = window.is_visible();
        self.visibility = cx.observe_window_visibility(window, |this, visible, _, _| {
            if visible.is_visible() && !this.window_visible {
                this.last_refresh = std::time::Instant::now() - Duration::from_secs(6);
            }
            this.window_visible = visible.is_visible()
        });
        for field in [
            &mut self.local_path,
            &mut self.remote_path,
            &mut self.name,
            &mut self.editor,
        ] {
            let value = field.read(cx).value();
            *field = cx.new(|cx| InputState::new(window, cx).default_value(value));
        }
        self.address_subscriptions = vec![
            cx.subscribe_in(
                &self.local_path,
                window,
                |this, _, event: &InputEvent, w, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.navigate_address(false, w, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &self.remote_path,
                window,
                |this, _, event: &InputEvent, w, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.navigate_address(true, w, cx);
                    }
                },
            ),
        ];
        self.auth
            .update(cx, |auth, cx| auth.rebind_window(window, cx));
        self.start_polling(window, cx);
    }
    pub fn launch(
        &mut self,
        request: helixlane::launch::OpenRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(path) = request.remote_path {
            request_remote_path(&self.state, path.clone());
            self.set_path(true, path, window, cx);
        }
        self.auth
            .update(cx, |auth, cx| auth.launch(request.secret, window, cx));
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
        self.bind_background(profile.clone(), cx);
        self.auth.update(cx, |a, cx| a.bind(profile, window, cx));
        if self.binding_dirty {
            self.binding_dirty = false;
            let path = self.state.lock().unwrap().remote.path.clone();
            self.remote_path
                .update(cx, |s, cx| s.set_value(path, window, cx));
        }
    }
    fn sync_background_binding(&mut self, cx: &mut Context<Self>) {
        let target = self
            .settings
            .borrow()
            .workspace
            .capture_task_target(self.id);
        if let Some(target) = target {
            self.bind_background(target.profile, cx);
        } else if self.ticket.take().is_some() {
            self.invalidate_binding(false);
            self.auth.update(cx, |a, cx| a.pause_binding(cx));
        }
    }
    fn bind_background(&mut self, profile: ConnectionProfile, cx: &mut Context<Self>) {
        let ticket = self.settings.borrow().workspace.ticket(self.id);
        if profile == self.profile && ticket == self.ticket {
            return;
        }
        let preserve_path = self.ticket == ticket;
        self.ticket = ticket;
        self.profile = profile.clone();
        self.auth.update(cx, |a, cx| a.bind_background(profile, cx));
        self.invalidate_binding(preserve_path);
    }
    fn invalidate_binding(&mut self, preserve_path: bool) {
        self.binding_dirty = true;
        let mut s = self.state.lock().unwrap();
        s.generation += 1;
        s.endpoint = None;
        s.canonical_path_pending = None;
        s.session_id = None;
        s.attempted = None;
        let path = if preserve_path {
            s.remote.path.clone()
        } else {
            ".".into()
        };
        let (hidden, sort) = (s.remote.hidden, s.remote.sort);
        s.remote = Browser::remote(path);
        s.remote.hidden = hidden;
        s.remote.sort = sort;
        s.busy = false;
        s.error = None;
        s.delete = false;
        s.form = None;
        s.pending = None;
    }
    pub fn set_visible(&mut self, visible: bool) {
        if visible && !self.visible {
            self.last_refresh = std::time::Instant::now() - Duration::from_secs(6);
        }
        self.visible = visible;
    }
    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.local_path.read(cx).focus_handle(cx)
    }
    fn poll(&mut self, cx: &mut Context<Self>) {
        self.sync_background_binding(cx);
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
            let ticket = begin_initial_listing(&mut s);
            let state = self.state.clone();
            drop(s);
            tasks::runtime().spawn(async move {
                let result = async {
                    let endpoint = Endpoint::connect(session).await?;
                    let path = endpoint.canonicalize(&ticket.path).await?;
                    let entries = endpoint.list(&path).await?;
                    Ok::<_, String>((endpoint, entries, path))
                }
                .await;
                finish_initial_listing(&mut state.lock().unwrap(), ticket, result);
            });
        }
    }
    fn refresh(&self, remote: bool) {
        refresh_state(&self.state, remote);
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
        let origin = TransferManager::global().origin(self.id);
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
                        TransferManager::global().submit_captured(spec, &origin);
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
        let origin = TransferManager::global().origin(self.id);
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
                        TransferManager::global().submit_captured(spec, &origin);
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
            let name = settings
                .workspace
                .target(*id)
                .and_then(|id| settings.workspace.resources.iter().find(|r| r.id == id))
                .map(|r| r.name.clone())
                .unwrap_or_else(|| endpoint.label());
            targets.push((name, endpoint, state.remote.path.clone()));
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
        let jobs = self.jobs.clone();
        tasks::runtime().spawn(async move {
            let result = jobs
                .run(async {
                    match op.as_str() {
                        "mkdir" => {
                            if value.is_empty()
                                || value.contains('/')
                                || value == "."
                                || value == ".."
                            {
                                return Err("Invalid directory name".into());
                            }
                            jobs.check()?;
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
                            jobs.check()?;
                            endpoint.rename(&selected[0], &to).await?
                        }
                        "chmod" => {
                            let mode = u32::from_str_radix(value.trim(), 8)
                                .map_err(|_| "Enter octal permissions, e.g. 644".to_string())?;
                            jobs.for_each(selected, |p| {
                                let endpoint = endpoint.clone();
                                async move { endpoint.chmod(&p, mode).await }
                            })
                            .await?;
                        }
                        "delete" => {
                            jobs.for_each(selected, |p| {
                                let endpoint = endpoint.clone();
                                let scope = jobs.clone();
                                async move { delete_tree(&endpoint, &p, &scope).await }
                            })
                            .await?;
                        }
                        _ => {}
                    }
                    Ok::<(), String>(())
                })
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
                Err(e) => {
                    s.notice = None;
                    s.error = Some(e);
                }
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
        let jobs = self.jobs.clone();
        tasks::runtime().spawn(async move {
            let result = jobs
                .run(EditLease::download(
                    endpoint,
                    path,
                    &std::env::temp_dir().join("helixlane-edits"),
                ))
                .await
                .and_then(|lease| {
                    let lease = Arc::new(lease);
                    let result = jobs.start(|| {
                        lease.open(&program)?;
                        EditorManager::global().register(lease.clone());
                        Ok(())
                    });
                    if result.is_err() {
                        let _ = std::fs::remove_file(&lease.local);
                    }
                    result
                });
            let mut s = state.lock().unwrap();
            if s.generation != generation {
                return;
            }
            match result {
                Ok(()) => {
                    s.notice = Some("External editor opened".into());
                }
                Err(e) => {
                    s.notice = None;
                    s.error = Some(e);
                }
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
                        drop(s);
                        this.persist_page(cx);
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
                            drop(s);
                            this.persist_page(cx);
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
        if remote {
            request_remote_path(&self.state, path.clone());
            self.set_path(true, path, w, cx);
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
        self.persist_page(cx);
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
            .track_scroll(if remote {
                &self.remote_scroll
            } else {
                &self.local_scroll
            })
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
impl Render for FilePane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(page) = self.restore_scroll.take() {
            let state = self.state.lock().unwrap();
            if state.local.entries.is_empty()
                || (page.remote_path.is_some() && state.remote.entries.is_empty())
            {
                self.restore_scroll = Some(page);
            } else {
                if page.local_scroll.iter().all(|v| v.is_finite()) {
                    self.local_scroll
                        .set_offset(point(px(page.local_scroll[0]), px(page.local_scroll[1])));
                }
                if page.remote_scroll.iter().all(|v| v.is_finite()) {
                    self.remote_scroll
                        .set_offset(point(px(page.remote_scroll[0]), px(page.remote_scroll[1])));
                }
            }
        }
        let canonical = self.state.lock().unwrap().canonical_path_pending.take();
        if let Some(path) = canonical {
            self.remote_path
                .update(cx, |s, cx| s.set_value(path, window, cx));
        }
        let _ = (&self.visibility, &self.address_subscriptions);
        let p = Palette::current(cx);
        let lang = self.settings.borrow().language;
        let zh = lang == Language::Chinese;
        let t = move |en, cn| if zh { cn } else { en };
        let mut content = div()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.settings
                        .borrow_mut()
                        .workspace
                        .activate_content_view(this.id);
                    cx.refresh_windows();
                    cx.stop_propagation();
                }),
            )
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
                    .child({
                        let owner = cx.entity().downgrade();
                        btn("file-actions", t("Actions…", "操作…")).dropdown_menu(
                            move |mut menu, _, _| {
                                for (op, label) in [
                                    ("mkdir", t("New folder…", "新建目录…")),
                                    ("rename", t("Rename…", "重命名…")),
                                    ("chmod", t("Permissions…", "权限…")),
                                    ("edit", t("External edit…", "外部编辑…")),
                                    ("delete", t("Delete selected…", "删除所选项…")),
                                ] {
                                    let owner = owner.clone();
                                    menu = menu.item(PopupMenuItem::new(label).on_click(
                                        move |_, w, cx| {
                                            let _ = owner
                                                .update(cx, |this, cx| this.open_form(op, w, cx));
                                        },
                                    ));
                                }
                                menu = menu.separator().label(t("Transfer conflicts", "传输冲突"));
                                for (c, label) in [
                                    (Conflict::Overwrite, t("Overwrite", "覆盖")),
                                    (Conflict::Skip, t("Skip", "跳过")),
                                    (Conflict::Rename, t("Rename", "重命名")),
                                ] {
                                    let owner = owner.clone();
                                    menu = menu.item(
                                        PopupMenuItem::new(label)
                                            .checked(
                                                std::mem::discriminant(&c)
                                                    == std::mem::discriminant(&conflict),
                                            )
                                            .on_click(move |_, _, cx| {
                                                let _ = owner.update(cx, |this, cx| {
                                                    this.state.lock().unwrap().conflict = c;
                                                    cx.notify();
                                                });
                                            }),
                                    );
                                }
                                menu
                            },
                        )
                    }),
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

#[cfg(test)]
mod launch_path_tests {
    use super::*;
    fn state(path: String, endpoint: Option<Endpoint>) -> Arc<Mutex<State>> {
        Arc::new(Mutex::new(State {
            generation: 0,
            session_id: Some(1),
            attempted: None,
            form: None,
            pending: None,
            canonical_path_pending: None,
            endpoint,
            local: Browser::new(".".into()),
            remote: Browser::remote(path),
            busy: false,
            error: None,
            notice: None,
            conflict: Conflict::Rename,
            delete: false,
            remote_active: false,
        }))
    }
    fn dirs(name: &str) -> (std::path::PathBuf, String, String) {
        let root = std::env::temp_dir().join(format!("helix-pane-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("A")).unwrap();
        std::fs::create_dir(root.join("B")).unwrap();
        std::fs::write(root.join("A/A.txt"), b"A").unwrap();
        std::fs::write(root.join("B/B.txt"), b"B").unwrap();
        (
            root.clone(),
            root.join("A").display().to_string(),
            root.join("B").display().to_string(),
        )
    }
    #[::core::prelude::v1::test]
    fn connected_launch_navigates_browser_and_real_entries() {
        let (root, a, b) = dirs("connected");
        let state = state(a.clone(), Some(Endpoint::Local));
        tasks::runtime().block_on(async {
            state.lock().unwrap().remote.entries = Endpoint::Local.list(&a).await.unwrap();
        });
        request_remote_path(&state, b.clone());
        tasks::runtime().block_on(async {
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    let ready = {
                        let s = state.lock().unwrap();
                        s.remote.path == b
                            && s.remote.entries.iter().any(|entry| entry.name == "B.txt")
                    };
                    if ready {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        });
        let s = state.lock().unwrap();
        assert_eq!(s.remote.path, b);
        assert!(!s.remote.entries.iter().any(|entry| entry.name == "A.txt"));
        drop(s);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[::core::prelude::v1::test]
    fn newer_path_invalidates_in_flight_initial_listing() {
        let (root, a, b) = dirs("initial");
        let state = state(a.clone(), None);
        let generation = begin_initial_listing(&mut state.lock().unwrap());
        let old_entries = tasks::runtime().block_on(Endpoint::Local.list(&a)).unwrap();
        request_remote_path(&state, b.clone());
        finish_initial_listing(
            &mut state.lock().unwrap(),
            generation,
            Ok((Endpoint::Local, old_entries, a)),
        );
        {
            let s = state.lock().unwrap();
            assert_eq!(s.remote.path, b);
            assert!(s.endpoint.is_none());
            assert!(s.attempted.is_none());
            assert!(!s.busy);
        }
        let entries = tasks::runtime().block_on(Endpoint::Local.list(&b)).unwrap();
        let latest = begin_initial_listing(&mut state.lock().unwrap());
        finish_initial_listing(
            &mut state.lock().unwrap(),
            latest,
            Ok((Endpoint::Local, entries, b.clone())),
        );
        let s = state.lock().unwrap();
        assert_eq!(s.remote.path, b);
        assert_eq!(s.remote.entries[0].name, "B.txt");
        drop(s);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(all(test, feature = "shell-tests"))]
mod windowless_binding_tests {
    use super::FilePane;
    use gpui_kit as gpui;
    use helixlane::{
        settings::Settings,
        workspace::{RunPolicy, TargetMode, ToolKind},
    };
    use std::{cell::RefCell, rc::Rc};

    #[gpui::test]
    fn windowless_sftp_retargets_and_rejects_late_results_after_source_close(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_quit_mode(gpui::QuitMode::Explicit);
        });
        let mut settings = Settings::demo_fixture();
        let owned = settings.workspace.create_window(Some(1));
        let id = settings
            .workspace
            .open_in(owned, ToolKind::Sftp, TargetMode::Follow);
        settings
            .workspace
            .views
            .iter_mut()
            .find(|v| v.id == id)
            .unwrap()
            .run_policy = RunPolicy::Background;
        let profile = settings.workspace.capture_task_target(id).unwrap().profile;
        let settings = Rc::new(RefCell::new(settings));
        let window = cx.add_window(|w, cx| FilePane::new(id, profile, settings.clone(), w, cx));
        let pane = window.root(cx).unwrap();
        cx.update(|cx| {
            window.update(cx, |_, w, _| w.remove_window()).unwrap();
            let old = pane.update(cx, |p, _| {
                super::begin_initial_listing(&mut p.state.lock().unwrap())
            });
            settings.borrow_mut().workspace.activate_resource(2);
            pane.update(cx, |p, cx| {
                p.poll(cx);
                assert_eq!(
                    p.ticket.unwrap().resource_id,
                    2,
                    "binding must not require a native window"
                );
                assert_eq!(
                    p.profile.host,
                    settings.borrow().workspace.resources[1].profiles[0].host
                );
                assert!(
                    p.auth.read(cx).shared_session().is_none(),
                    "no automatic authentication or prior target session"
                );
            });
            settings.borrow_mut().workspace.activate_resource(1);
            settings.borrow_mut().workspace.close_window(1);
            pane.update(cx, |p, cx| {
                p.poll(cx);
                assert!(p.ticket.is_none());
                let mut state = p.state.lock().unwrap();
                super::finish_initial_listing(
                    &mut state,
                    old,
                    Ok((helixlane::sftp::Endpoint::Local, vec![], "/stale-A".into())),
                );
                assert!(state.endpoint.is_none());
                assert_ne!(state.remote.path, "/stale-A");
                assert!(!state.busy);
            });
        });
    }
}

#[cfg(all(test, feature = "shell-tests"))]
mod scoped_file_jobs_tests {
    use super::{FilePane, MutationTarget};
    use gpui_kit as gpui;
    use helixlane::{
        settings::Settings,
        sftp::Endpoint,
        workspace::{TargetMode, ToolKind},
    };
    use std::{cell::RefCell, rc::Rc, time::Duration};

    #[gpui::test]
    fn scoped_file_job_survives_follow_retarget_hide_and_closed_presentation(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_quit_mode(gpui::QuitMode::Explicit);
        });
        let root = std::env::temp_dir().join(format!("helix-file-continue-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let captured = root.join("captured-A");
        let current = root.join("current-B");
        std::fs::write(&captured, b"A").unwrap();
        std::fs::write(&current, b"B").unwrap();
        let mut settings = Settings::demo_fixture();
        let id = settings.workspace.open(ToolKind::Sftp, TargetMode::Follow);
        let profile = settings.workspace.capture_task_target(id).unwrap().profile;
        let settings = Rc::new(RefCell::new(settings));
        let window = cx.add_window(|w, cx| FilePane::new(id, profile, settings.clone(), w, cx));
        let pane = window.root(cx).unwrap();
        let jobs = cx.update(|cx| pane.read(cx).jobs.clone());
        let (release, gate) = tokio::sync::oneshot::channel();
        let target = captured.display().to_string();
        let task = helixlane::tasks::runtime().spawn(async move {
            jobs.run(async move {
                gate.await.map_err(|e| e.to_string())?;
                Endpoint::Local.remove(&target).await
            })
            .await
        });
        cx.update(|cx| {
            settings.borrow_mut().workspace.activate_resource(2);
            let next = settings
                .borrow()
                .workspace
                .capture_task_target(id)
                .unwrap()
                .profile;
            pane.update(cx, |p, cx| {
                p.bind_background(next, cx);
                p.set_visible(false);
                assert!(p.jobs.check().is_ok());
            });
            // Continue removes the native presentation without invoking explicit cancel.
            window.update(cx, |_, w, _| w.remove_window()).unwrap();
        });
        release.send(()).unwrap();
        helixlane::tasks::runtime().block_on(task).unwrap().unwrap();
        assert!(!captured.exists());
        assert!(current.exists(), "submitted mutation was retargeted to B");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[gpui::test]
    fn scoped_file_cancel_reaches_mutations_and_leaves_other_view_jobs_alive(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_quit_mode(gpui::QuitMode::Explicit);
        });
        let root = std::env::temp_dir().join(format!("helix-file-cancel-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut settings = Settings::demo_fixture();
        let first = settings
            .workspace
            .open(ToolKind::Sftp, TargetMode::Fixed(1));
        let second = settings
            .workspace
            .open(ToolKind::Sftp, TargetMode::Fixed(1));
        let profile = settings
            .workspace
            .capture_task_target(first)
            .unwrap()
            .profile;
        let settings = Rc::new(RefCell::new(settings));
        let window =
            cx.add_window(|w, cx| FilePane::new(first, profile.clone(), settings.clone(), w, cx));
        let other = cx.add_window(|w, cx| FilePane::new(second, profile, settings, w, cx));
        let pane = window.root(cx).unwrap();
        let peer = other.root(cx).unwrap();
        let protected = root.join("cancelled-view-file");
        let independent = root.join("other-view-file");
        std::fs::write(&protected, b"keep").unwrap();
        std::fs::write(&independent, b"delete").unwrap();
        let states = cx.update(|cx| {
            for (pane, path, cancel) in [(&pane, &protected, true), (&peer, &independent, false)] {
                pane.update(cx, |p, cx| {
                    let mut state = p.state.lock().unwrap();
                    state.pending = Some(MutationTarget {
                        endpoint: Endpoint::Local,
                        path: root.display().to_string(),
                        selected: vec![path.display().to_string()],
                        generation: state.generation,
                        remote: false,
                    });
                    drop(state);
                    if cancel {
                        p.cancel_owned_jobs(cx);
                    }
                    p.operation("delete", String::new());
                });
            }
            [pane.read(cx).state.clone(), peer.read(cx).state.clone()]
        });
        helixlane::tasks::runtime().block_on(async {
            tokio::time::timeout(Duration::from_secs(3), async {
                while states.iter().any(|state| {
                    let state = state.lock().unwrap();
                    state.error.is_none() && state.notice.is_none()
                }) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
        });
        assert!(
            protected.exists(),
            "cancelled FilePane still executed a detached mutation"
        );
        assert!(
            !independent.exists(),
            "another view's mutation must continue"
        );
        assert!(
            states[0]
                .lock()
                .unwrap()
                .error
                .as_deref()
                .unwrap()
                .contains("Cancelled")
        );
        cx.update(|cx| {
            pane.update(cx, |p, _| {
                p.state.lock().unwrap().error = None;
                p.edit("helixlane-editor-must-not-be-launched".into());
            })
        });
        helixlane::tasks::runtime().block_on(async {
            tokio::time::timeout(Duration::from_secs(3), async {
                while states[0].lock().unwrap().error.is_none() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
        });
        assert_eq!(
            states[0].lock().unwrap().error.as_deref(),
            Some(helixlane::sftp::jobs::CANCELLED)
        );
        assert!(
            helixlane::sftp::editor::EditorManager::global()
                .leases()
                .iter()
                .all(|lease| lease.remote != protected.display().to_string())
        );

        cx.update(|cx| {
            window.update(cx, |_, w, _| w.remove_window()).unwrap();
            other.update(cx, |_, w, _| w.remove_window()).unwrap();
        });
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(all(test, feature = "shell-tests"))]
mod visibility_tests {
    use super::FilePane;
    use gpui_kit as gpui;
    use helixlane::settings::Settings;
    use std::{cell::RefCell, rc::Rc, time::Duration};
    #[gpui::test]
    fn native_show_requests_one_immediate_refresh(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_quit_mode(gpui::QuitMode::Explicit);
        });
        let mut settings = Settings::demo_fixture();
        let id = settings.workspace.open(
            helixlane::workspace::ToolKind::Sftp,
            helixlane::workspace::TargetMode::Follow,
        );
        let profile = settings.workspace.capture_task_target(id).unwrap().profile;
        let settings = Rc::new(RefCell::new(settings));
        let window = cx.add_window(|w, cx| FilePane::new(id, profile, settings, w, cx));
        let pane = window.root(cx).unwrap();
        cx.simulate_window_visibility_change(window.into(), gpui::WindowVisibility::Hidden);
        pane.update(cx, |p, _| {
            assert!(!p.window_visible);
            p.last_refresh = std::time::Instant::now();
        });
        cx.simulate_window_visibility_change(window.into(), gpui::WindowVisibility::Visible);
        pane.update(cx, |p, _| {
            assert!(p.window_visible);
            assert!(p.last_refresh.elapsed() > Duration::from_secs(5));
            p.last_refresh = std::time::Instant::now();
        });
        cx.simulate_window_visibility_change(window.into(), gpui::WindowVisibility::Visible);
        pane.update(cx, |p, _| {
            assert!(p.last_refresh.elapsed() < Duration::from_secs(1))
        });
    }
}
