use crate::theme::*;
use gpui_kit::component::{
    Sizable, TitleBar,
    button::{Button, ButtonVariants},
    dock::*,
    input::{Input, InputState},
    menu::{DropdownMenu, PopupMenu, PopupMenuItem},
    resizable::{resizable_panel, v_resizable},
};
use gpui_kit::{assets::IconName, prelude::FluentBuilder as _, *};
use helixlane::{
    i18n::Language,
    settings::{Settings, ThemePreference},
    workspace::*,
};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};
type Shared = Rc<RefCell<Settings>>;
type Panels = Rc<RefCell<HashMap<ViewId, Entity<ToolPanel>>>>;
pub struct HelixLane {
    settings: Shared,
    panels: Panels,
    dock: Entity<DockArea>,
    top: Entity<DockArea>,
    editing: Option<ResourceId>,
    fields: Vec<Entity<InputState>>,
    editor_error: Option<String>,
    error: Option<String>,
    by_type: bool,
    deleting: bool,
    show_resources: bool,
    subscriptions: Vec<Subscription>,
    mounted: HashSet<ViewId>,
    layout_save: LayoutSaveGate,
}
fn button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Button {
    Button::new(id).ghost().small().label(label)
}
impl HelixLane {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let loaded = Settings::load(&Settings::path());
        let error = loaded.as_ref().err().cloned();
        let mut settings = loaded.unwrap_or_default();
        apply(settings.theme, Some(window), cx);
        gpui_kit::component::set_locale(if settings.language == Language::Chinese {
            "zh-CN"
        } else {
            "en"
        });
        if settings.workspace.views.is_empty() {
            let id = settings
                .workspace
                .open(ToolKind::Sftp, TargetMode::Fixed(1));
            settings.workspace.layout.focus(id);
            let services = settings
                .workspace
                .open(ToolKind::Services, TargetMode::Follow);
            settings.workspace.move_view(services, Location::Bottom);
        }
        let shared = Rc::new(RefCell::new(settings));
        let panels: Panels = Default::default();
        let weak = cx.entity().downgrade();
        let state = shared.clone();
        let map = panels.clone();
        let owner = weak.clone();
        register_panel(cx, "helixlane.tool", move |context, _, cx| {
            let id = match context.info() {
                PanelInfo::Panel(v) => v.as_u64().unwrap_or_default(),
                _ => 0,
            };
            let p = cx.new(|cx| ToolPanel {
                id,
                settings: state.clone(),
                owner: owner.clone(),
                focus: cx.focus_handle(),
                terminal: None,
                files: None,
                tmux: None,
                transfers: None,
                launch: None,
            });
            map.borrow_mut().insert(id, p.clone());
            panel_handle(p)
        });
        let dock = cx.new(|cx| {
            DockArea::new("workbench", Some(1), window, cx).with_renderer(DockSkin::new(cx))
        });
        let top = cx.new(|cx| {
            DockArea::new("top-tools", Some(1), window, cx).with_renderer(DockSkin::new(cx))
        });
        let mut this = Self {
            settings: shared,
            panels,
            dock,
            top,
            editing: None,
            fields: vec![],
            editor_error: None,
            error,
            by_type: false,
            deleting: false,
            show_resources: true,
            subscriptions: vec![],
            mounted: HashSet::new(),
            layout_save: LayoutSaveGate::default(),
        };
        let saved = this.settings.borrow().dock_layout.clone();
        let saved_top = this.settings.borrow().top_layout.clone();
        for (area, saved) in [(&this.dock, saved), (&this.top, saved_top)] {
            if let Some(saved) = saved {
                let restored = serde_json::from_value::<DockAreaState>(saved)
                    .ok()
                    .is_some_and(|state| {
                        area.update(cx, |d, cx| d.load(state, window, cx).is_ok())
                    });
                if !restored {
                    this.settings.borrow_mut().recovery = Some("recovered_layout".into());
                }
            }
        }
        // Restoration can succeed in one region and fail in another. Reconcile every
        // model instance against the panels that actually made it into either tree.
        let live = this.live_locations(cx);
        let missing = this.settings.borrow().workspace.unmounted_views(&live);
        for v in missing {
            this.mount(v.id, window, cx);
        }
        this.mounted = this.live_locations(cx).keys().copied().collect();
        this.persist_layout(cx);
        for area in [&this.dock, &this.top] {
            let sub = cx.subscribe(area, |this, _, _: &DockEvent, cx| {
                this.persist_layout(cx);
                cx.notify();
            });
            this.subscriptions.push(sub);
        }
        this.subscriptions
            .push(cx.observe_window_bounds(window, |this, _, cx| this.persist_layout(cx)));
        this.subscriptions
            .push(cx.observe_window_appearance(window, |this, window, cx| {
                if this.settings.borrow().theme == ThemePreference::System {
                    apply(ThemePreference::System, Some(window), cx);
                    cx.refresh_windows();
                }
            }));
        this
    }
    pub fn dispatch_launch(&mut self, request: helixlane::launch::OpenRequest, window: &mut Window, cx: &mut Context<Self>) {
        if request.target==helixlane::launch::Target::Focus {window.activate_window();cx.activate(true);return;}
        let opened=match request.apply(&mut self.settings.borrow_mut().workspace){Ok(opened)=>opened,Err(error)=>{self.error=Some(error);cx.notify();return;}};
        if !self.panels.borrow().contains_key(&opened.view_id) {self.mount(opened.view_id,window,cx);}
        if let Some(panel)=self.panels.borrow().get(&opened.view_id).cloned(){panel.update(cx,|panel,cx|{panel.launch=Some(request);cx.notify();});}
        self.focus_tool(opened.view_id,window,cx);
        self.persist_layout(cx);
        window.activate_window();cx.activate(true);cx.notify();
    }
    pub fn listen_launch(&mut self, server: helixlane::launch::ipc::Server, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window,async move |this,cx| {
            // Owning server here keeps the lock/endpoint alive exactly with the window.
            loop {cx.background_executor().timer(std::time::Duration::from_millis(50)).await;
                let alive=cx.update(|window,cx|this.update(cx,|this,cx|{
                    for request in server.requests.try_iter().take(helixlane::launch::ipc::QUEUE_CAPACITY){this.dispatch_launch(request,window,cx);}
                })).map(|r|r.is_ok()).unwrap_or(false);
                if !alive {break;}
            }
        }).detach();
    }
    fn text(&self, key: &str) -> &'static str {
        self.settings.borrow().language.text(key)
    }
    fn persist(&mut self) {
        self.error = self.settings.borrow().save(&Settings::path()).err();
    }
    fn persist_layout(&mut self, cx: &mut Context<Self>) {
        self.layout_save.request();
        cx.notify();
    }
    fn live_locations(&self, cx: &App) -> HashMap<ViewId, Location> {
        let mut found = HashMap::new();
        let panels = self.panels.borrow();
        for (area, placement, location) in [
            (&self.dock, DockPlacement::Center, Location::Center),
            (&self.dock, DockPlacement::Left, Location::Left),
            (&self.dock, DockPlacement::Right, Location::Right),
            (&self.dock, DockPlacement::Bottom, Location::Bottom),
            (&self.top, DockPlacement::Center, Location::Top),
        ] {
            if let Some(tree) = area.read(cx).layout(placement) {
                for physical in tree.panels() {
                    if let Some((id, _)) = panels
                        .iter()
                        .find(|(_, panel)| PanelId::from(panel.entity_id()) == physical)
                    {
                        found.insert(*id, location);
                    }
                }
            }
        }
        found
    }
    fn snapshot_layout(&mut self, cx: &mut Context<Self>) {
        let main = self.dock.read(cx).dump(cx);
        let top = self.top.read(cx).dump(cx);
        self.sync_layout(&main, &top);
        {
            let mut s = self.settings.borrow_mut();
            s.dock_layout = serde_json::to_value(main).ok();
            s.top_layout = serde_json::to_value(top).ok();
        }
        self.persist();
    }
    fn sync_layout(&mut self, main: &DockAreaState, top: &DockAreaState) {
        fn collect(state: &PanelState, location: Location, found: &mut HashMap<ViewId, Location>) {
            if state.panel_name == "helixlane.tool"
                && let PanelInfo::Panel(v) = &state.info
                && let Some(id) = v.as_u64()
            {
                found.insert(id, location);
            }
            for child in &state.children {
                collect(child, location, found);
            }
        }
        fn model(state: &PanelState) -> LayoutNode {
            match &state.info {
                PanelInfo::Tabs { active_index } => {
                    let views: Vec<_> = state
                        .children
                        .iter()
                        .filter_map(|child| match &child.info {
                            PanelInfo::Panel(value) => value.as_u64(),
                            _ => None,
                        })
                        .collect();
                    let active = views.get(*active_index).copied();
                    LayoutNode::Tabs { views, active }
                }
                PanelInfo::Stack { sizes, axis } => {
                    let mut nodes = state.children.iter().map(model).collect::<Vec<_>>();
                    if nodes.is_empty() {
                        return LayoutNode::default();
                    }
                    let mut result = nodes.pop().unwrap();
                    for (index, first) in nodes.into_iter().enumerate().rev() {
                        let total: f32 = sizes.iter().skip(index).map(|p| f32::from(*p)).sum();
                        let ratio = if total > 0. {
                            f32::from(sizes[index]) / total
                        } else {
                            0.5
                        };
                        result = LayoutNode::Split {
                            vertical: *axis == 1,
                            ratio: ratio.clamp(0.05, 0.95),
                            first: Box::new(first),
                            second: Box::new(result),
                        };
                    }
                    result
                }
                PanelInfo::Panel(value) => {
                    let views = value.as_u64().into_iter().collect::<Vec<_>>();
                    LayoutNode::Tabs {
                        active: views.first().copied(),
                        views,
                    }
                }
            }
        }
        let mut found = HashMap::new();
        collect(&main.center, Location::Center, &mut found);
        for (dock, location) in [
            (&main.left_dock, Location::Left),
            (&main.right_dock, Location::Right),
            (&main.bottom_dock, Location::Bottom),
        ] {
            if let Some(d) = dock {
                collect(d.panel(), location, &mut found);
            }
        }
        collect(&top.center, Location::Top, &mut found);
        let mut s = self.settings.borrow_mut();
        s.workspace.layout = model(&main.center);
        s.workspace.reconcile_view_locations(&found, &self.mounted);
        self.mounted = found.keys().copied().collect();
        self.panels
            .borrow_mut()
            .retain(|id, _| s.workspace.views.iter().any(|v| v.id == *id));
    }
    fn four_hosts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let resources: Vec<_> = self
            .settings
            .borrow()
            .workspace
            .resources
            .iter()
            .take(4)
            .map(|r| r.id)
            .collect();
        let mut groups = Vec::new();
        for resource in resources {
            let id = self
                .settings
                .borrow_mut()
                .workspace
                .open(ToolKind::Sftp, TargetMode::Fixed(resource));
            let owner = cx.entity().downgrade();
            let p = cx.new(|cx| ToolPanel {
                id,
                settings: self.settings.clone(),
                owner,
                focus: cx.focus_handle(),
                terminal: None,
                files: None,
                tmux: None,
                transfers: None,
                launch: None,
            });
            self.panels.borrow_mut().insert(id, p.clone());
            groups.push((id, DockLayout::tabs().panel_view(panel_handle(p), cx)));
        }
        for v in &mut self.settings.borrow_mut().workspace.views {
            if v.location == Location::Center && !groups.iter().any(|(id, _)| *id == v.id) {
                v.location = Location::Services;
            }
        }
        let mut left = DockLayout::v_split();
        let mut right = DockLayout::v_split();
        for (i, (_, group)) in groups.into_iter().enumerate() {
            if i < 2 {
                left = left.child(group, None);
            } else {
                right = right.child(group, None);
            }
        }
        self.dock.update(cx, |d, cx| {
            d.set_center(
                DockLayout::h_split().child(left, None).child(right, None),
                window,
                cx,
            )
        });
        self.persist_layout(cx);
        cx.refresh_windows();
    }
    fn mount(&mut self, id: ViewId, window: &mut Window, cx: &mut Context<Self>) {
        let location = self
            .settings
            .borrow()
            .workspace
            .views
            .iter()
            .find(|v| v.id == id)
            .map(|v| v.location)
            .unwrap_or(Location::Center);
        if location == Location::Services {
            return;
        }
        self.mount_new(id, location, window, cx);
    }
    fn mount_new(
        &mut self,
        id: ViewId,
        location: Location,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if location == Location::Services {
            return;
        }
        let existing = self.panels.borrow().get(&id).cloned();
        let owner = cx.entity().downgrade();
        let panel = existing.unwrap_or_else(|| {
            cx.new(|pcx| ToolPanel {
                id,
                settings: self.settings.clone(),
                owner,
                focus: pcx.focus_handle(),
                terminal: None,
                files: None,
                tmux: None,
                transfers: None,
                launch: None,
            })
        });
        self.panels.borrow_mut().insert(id, panel.clone());
        let area = if location == Location::Top {
            &self.top
        } else {
            &self.dock
        };
        area.update(cx, |d, cx| {
            d.add_panel_view(
                panel_handle(panel),
                placement(location),
                Some(px(220.)),
                window,
                cx,
            )
        });
    }
    fn open(
        &mut self,
        tool: ToolKind,
        mode: TargetMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.settings.borrow_mut().workspace.open(tool, mode);
        self.mount_new(id, Location::Center, window, cx);
        self.persist_layout(cx);
        cx.notify();
    }
    fn edit(&mut self, id: ResourceId, window: &mut Window, cx: &mut Context<Self>) {
        let resource = self
            .settings
            .borrow()
            .workspace
            .resources
            .iter()
            .find(|r| r.id == id)
            .cloned();
        if let Some(r) = resource {
            let p = &r.profiles[0];
            self.fields = [
                r.name,
                p.host.clone(),
                p.username.clone(),
                p.port.to_string(),
                r.group,
                r.tags.join(", "),
                r.notes,
            ]
            .into_iter()
            .map(|value| cx.new(|cx| InputState::new(window, cx).default_value(value)))
            .collect();
            self.editing = Some(id);
            self.editor_error = None;
            self.deleting = false;
            cx.notify();
        }
    }
    fn save_resource(&mut self, cx: &mut Context<Self>) {
        let values: Vec<String> = self
            .fields
            .iter()
            .map(|v| v.read(cx).value().to_string())
            .collect();
        let port = match validate_resource_form(&values[0], &values[1], &values[2], &values[3]) {
            Ok(port) => port,
            Err(key) => {
                self.editor_error = Some(self.text(key).into());
                cx.notify();
                return;
            }
        };
        if let Some(r) = self
            .settings
            .borrow_mut()
            .workspace
            .resources
            .iter_mut()
            .find(|r| Some(r.id) == self.editing)
        {
            r.name = values[0].trim().into();
            r.profiles[0].host = values[1].trim().into();
            r.profiles[0].username = values[2].trim().into();
            r.profiles[0].port = port;
            r.group = values[4].clone();
            r.tags = values[5]
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect();
            r.notes = values[6].clone();
        }
        self.editing = None;
        self.persist();
        cx.refresh_windows();
    }
    fn move_tool(
        &mut self,
        id: ViewId,
        location: Location,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let p = self.panels.borrow().get(&id).cloned();
        if let Some(p) = p {
            self.dock
                .update(cx, |d, cx| d.remove_panel(p.clone(), window, cx));
            self.top.update(cx, |d, cx| d.remove_panel(p, window, cx));
        }
        self.settings.borrow_mut().workspace.move_view(id, location);
        self.mount_new(id, location, window, cx);
        self.persist_layout(cx);
        cx.refresh_windows();
    }
    fn split_tool(
        &mut self,
        id: ViewId,
        vertical: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = self.panels.borrow().get(&id).cloned();
        let Some(p) = panel else { return };
        let pid = PanelId::from(p.entity_id());
        let target = self
            .dock
            .read(cx)
            .layout(DockPlacement::Center)
            .and_then(|tree| {
                tree.panels()
                    .find(|p| *p != pid)
                    .and_then(|other| tree.find_panel_node(other))
            });
        if let Some(target) = target {
            self.dock.update(cx, |d, cx| {
                d.split_at(
                    target,
                    pid,
                    if vertical {
                        gpui_kit::component::Placement::Bottom
                    } else {
                        gpui_kit::component::Placement::Right
                    },
                    window,
                    cx,
                )
            });
            self.settings.borrow_mut().workspace.split(id, vertical);
            self.persist_layout(cx);
            cx.refresh_windows();
        }
    }
    fn focus_tool(&mut self, id: ViewId, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .settings
            .borrow()
            .workspace
            .views
            .iter()
            .any(|v| v.id == id && v.location == Location::Services)
        {
            self.settings
                .borrow_mut()
                .workspace
                .move_view(id, Location::Center);
            self.mount_new(id, Location::Center, window, cx);
        }
        if let Some(p) = self.panels.borrow().get(&id) {
            let pid = PanelId::from(p.entity_id());
            self.dock
                .update(cx, |d, cx| d.select_panel(pid, window, cx));
            self.top.update(cx, |d, cx| d.select_panel(pid, window, cx));
        }
        cx.refresh_windows();
    }
    fn resources(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = Palette::current(cx);
        let state = self.settings.borrow();
        let lang = state.language;
        let selected = state.workspace.active_resource;
        let resources = state.workspace.resources.clone();
        drop(state);
        let mut content = island(p)
            .id("resources-scroll")
            .overflow_y_scroll()
            .p_3()
            .gap_2()
            .child(
                row()
                    .justify_between()
                    .child(div().text_size(px(13.)).child(lang.text("resources"))),
            )
            .child(
                button("new", lang.text("new")).on_click(cx.listener(|this, _, window, cx| {
                    let id = this.settings.borrow_mut().workspace.add_resource();
                    this.edit(id, window, cx);
                })),
            );
        let mut previous_group = None;
        for r in resources {
            let id = r.id;
            if previous_group.as_ref() != Some(&r.group) {
                content = content.child(
                    div()
                        .px_2()
                        .pt_2()
                        .text_size(px(10.))
                        .text_color(rgb(p.muted))
                        .child(r.group.clone()),
                );
                previous_group = Some(r.group.clone());
            }
            content = content.child(
                row()
                    .id(("resource", id))
                    .px_2()
                    .py_2()
                    .gap_2()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(selected == Some(id), |d| d.bg(rgb(p.selected)))
                    .hover(|s| s.bg(rgb(p.hover)))
                    .child(icon(IconName::Network).text_color(rgb(p.accent)))
                    .child(div().flex_1().child(r.name))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings.borrow_mut().workspace.activate_resource(id);
                        this.persist();
                        cx.refresh_windows();
                    })),
            );
            if selected == Some(id) {
                content = content.child(
                    row()
                        .flex_wrap()
                        .gap_1()
                        .child(button(("ssh", id), "SSH").on_click(cx.listener(
                            move |this, _, w, cx| {
                                this.open(ToolKind::Terminal, TargetMode::Fixed(id), w, cx)
                            },
                        )))
                        .child(button(("sftp", id), "SFTP").on_click(cx.listener(
                            move |this, _, w, cx| {
                                this.open(ToolKind::Sftp, TargetMode::Fixed(id), w, cx)
                            },
                        )))
                        .child(
                            button(("edit", id), lang.text("edit"))
                                .on_click(cx.listener(move |this, _, w, cx| this.edit(id, w, cx))),
                        ),
                );
            }
        }
        content
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(p.muted))
                    .child(lang.text("workspace_status")),
            )
            .into_any_element()
    }
    fn rail(&self, cx: &mut Context<Self>) -> AnyElement {
        let language = self.settings.borrow().language;
        let p = Palette::current(cx);
        let mut rail = div()
            .flex()
            .flex_col()
            .items_center()
            .w(px(42.))
            .flex_none()
            .pt_2()
            .gap_2()
            .child(
                Button::new("resources-toggle")
                    .ghost()
                    .icon(icon(IconName::PanelLeft))
                    .tooltip(language.text("resources"))
                    .w(px(32.))
                    .h(px(32.))
                    .when(self.show_resources, |b| b.bg(rgb(p.hover)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_resources = !this.show_resources;
                        cx.notify();
                    })),
            );
        for (index, tool, name) in [
            (0usize, ToolKind::Terminal, IconName::SquareTerminal),
            (1, ToolKind::Sftp, IconName::Folder),
            (2, ToolKind::Services, IconName::LayoutDashboard),
            (3, ToolKind::Transfers, IconName::ArrowDown),
            (4, ToolKind::Information, IconName::Info),
            (5, ToolKind::Tmux, IconName::SquareTerminal),
        ] {
            rail = rail.child(
                Button::new(("tool-rail", index))
                    .ghost()
                    .icon(icon(name))
                    .tooltip(language.text(tool.key()))
                    .w(px(32.))
                    .h(px(32.))
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.open(tool, TargetMode::Follow, w, cx)
                    })),
            );
        }
        rail.into_any_element()
    }
    fn editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = Palette::current(cx);
        let mut form = island(p).p_4().gap_3().child(self.text("edit"));
        for (index, key) in ["name", "host", "user", "port", "group", "tags", "notes"]
            .into_iter()
            .enumerate()
        {
            let label = self.text(key);
            form = form
                .child(div().text_color(rgb(p.muted)).child(label))
                .child(Input::new(&self.fields[index]).small());
        }
        if let Some(error) = &self.editor_error {
            form = form.child(div().text_color(rgb(p.warning)).child(error.clone()));
        }
        form.child(
            row()
                .gap_2()
                .child(
                    button("save", self.text("save"))
                        .on_click(cx.listener(|this, _, _, cx| this.save_resource(cx))),
                )
                .child(button("cancel", self.text("cancel")).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.editing = None;
                        cx.notify();
                    },
                ))),
        )
        .child(
            button(
                "delete",
                self.text(if self.deleting {
                    "confirm_delete"
                } else {
                    "delete"
                }),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                if !this.deleting {
                    this.deleting = true;
                    cx.notify();
                    return;
                }
                let id = this.editing.take();
                this.settings
                    .borrow_mut()
                    .workspace
                    .resources
                    .retain(|r| Some(r.id) != id);
                this.persist();
                cx.refresh_windows();
            })),
        )
        .into_any_element()
    }
}
fn placement(location: Location) -> DockPlacement {
    match location {
        Location::Left => DockPlacement::Left,
        Location::Right => DockPlacement::Right,
        Location::Bottom => DockPlacement::Bottom,
        _ => DockPlacement::Center,
    }
}
impl Render for HelixLane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.layout_save.after_render() {
            let owner = cx.entity().downgrade();
            // Schedule from rendering, so the edited frame completes layout and
            // Kit has actual pixel measurements before a layout snapshot is taken.
            window.on_next_frame(move |_, cx| {
                let _ = owner.update(cx, |this, cx| {
                    if this.layout_save.after_frame() {
                        this.snapshot_layout(cx);
                    }
                    // Reconciliation can change Services locations/counts or save errors.
                    cx.notify();
                });
            });
        }
        let p = Palette::current(cx);
        let language = self.settings.borrow().language;
        let current_theme = self.settings.borrow().theme;
        let owner = cx.entity().downgrade();
        let toolbar = row()
            .h(px(TOOLBAR_HEIGHT))
            .w_full()
            .px_3()
            .gap_3()
            .child(
                div()
                    .rounded(px(6.))
                    .p_1()
                    .bg(rgb(p.accent))
                    .text_color(rgb(p.editor))
                    .child("HL"),
            )
            .child("HelixLane")
            .child(icon(IconName::ChevronRight).text_color(rgb(p.muted)))
            .child(
                div()
                    .text_color(rgb(p.muted))
                    .child(language.text("workbench")),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(p.muted))
                    .child(language.text("workspace_status")),
            )
            .child(
                Button::new("settings")
                    .ghost()
                    .icon(icon(IconName::Settings))
                    .tooltip(language.text("settings"))
                    .dropdown_menu(move |mut menu, _, _| {
                        for (theme, key) in [
                            (ThemePreference::System, "system"),
                            (ThemePreference::Dark, "dark"),
                            (ThemePreference::Light, "light"),
                        ] {
                            let owner = owner.clone();
                            menu = menu.item(
                                PopupMenuItem::new(language.text(key))
                                    .checked(current_theme == theme)
                                    .on_click(move |_, window, cx| {
                                        let _ = owner.update(cx, |this, cx| {
                                            this.settings.borrow_mut().theme = theme;
                                            apply(theme, Some(window), cx);
                                            this.persist();
                                            cx.refresh_windows();
                                        });
                                    }),
                            );
                        }
                        menu = menu.separator();
                        for (lang, label) in
                            [(Language::English, "English"), (Language::Chinese, "中文")]
                        {
                            let owner = owner.clone();
                            menu = menu.item(
                                PopupMenuItem::new(label)
                                    .checked(language == lang)
                                    .on_click(move |_, _, cx| {
                                        let _ = owner.update(cx, |this, cx| {
                                            this.settings.borrow_mut().language = lang;
                                            gpui_kit::component::set_locale(
                                                if lang == Language::Chinese {
                                                    "zh-CN"
                                                } else {
                                                    "en"
                                                },
                                            );
                                            this.persist();
                                            cx.refresh_windows();
                                        });
                                    }),
                            );
                        }
                        let owner = owner.clone();
                        menu.separator().item(
                            PopupMenuItem::new(language.text("four_hosts")).on_click(
                                move |_, w, cx| {
                                    let _ = owner.update(cx, |this, cx| this.four_hosts(w, cx));
                                },
                            ),
                        )
                    }),
            );
        let has_top = self
            .settings
            .borrow()
            .workspace
            .views
            .iter()
            .any(|v| v.location == Location::Top);
        let center = if has_top {
            v_resizable("top-and-center")
                .child(resizable_panel().size(px(180.)).child(self.top.clone()))
                .child(resizable_panel().child(self.dock.clone()))
                .into_any_element()
        } else {
            div()
                .size_full()
                .child(self.dock.clone())
                .into_any_element()
        };
        let mut frame = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(p.frame))
            .text_color(rgb(p.text))
            .font_family("sans-serif")
            .text_size(px(12.))
            .child(
                TitleBar::new()
                    .h(px(TOOLBAR_HEIGHT))
                    .bg(rgb(p.frame))
                    .child(toolbar),
            );
        let message = self
            .error
            .as_ref()
            .map(|detail| format!("{}: {detail}", language.text("settings_error")))
            .or_else(|| {
                self.settings
                    .borrow()
                    .recovery
                    .as_ref()
                    .map(|key| language.text(key).to_string())
            });
        if let Some(message) = message {
            frame = frame.child(
                div()
                    .px_4()
                    .py_1()
                    .text_color(rgb(p.warning))
                    .child(message),
            );
        }
        frame
            .child(
                row()
                    .items_stretch()
                    .flex_1()
                    .min_h_0()
                    .gap_1()
                    .px_1()
                    .child(self.rail(cx))
                    .when(self.show_resources, |r| {
                        r.child(
                            div()
                                .w(px(230.))
                                .flex_none()
                                .child(if self.editing.is_some() {
                                    self.editor(cx)
                                } else {
                                    self.resources(cx)
                                }),
                        )
                    })
                    .child(div().flex_1().min_w_0().child(center)),
            )
            .child(
                row()
                    .h(px(28.))
                    .px_3()
                    .gap_3()
                    .text_size(px(11.))
                    .text_color(rgb(p.muted))
                    .child(language.text("workspace_status"))
                    .child(div().flex_1())
                    .child(format!(
                        "{} {} · {} {}",
                        self.settings.borrow().workspace.resources.len(),
                        language.text("resources"),
                        self.settings.borrow().workspace.views.len(),
                        language.text("views")
                    )),
            )
            .into_any_element()
    }
}
struct ToolPanel {
    id: ViewId,
    settings: Shared,
    owner: WeakEntity<HelixLane>,
    focus: FocusHandle,
    terminal: Option<Entity<crate::terminal_ui::TerminalPane>>,
    files: Option<Entity<crate::file_ui::FilePane>>,
    tmux: Option<Entity<crate::tmux_ui::TmuxPane>>,
    transfers: Option<Entity<crate::file_ui::TransfersPane>>,
    launch: Option<helixlane::launch::OpenRequest>,
}
impl EventEmitter<PanelEvent> for ToolPanel {}
impl Focusable for ToolPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.terminal
            .as_ref()
            .map(|terminal| terminal.read(cx).focus_handle(cx))
            .or_else(|| {
                self.files
                    .as_ref()
                    .map(|files| files.read(cx).focus_handle(cx))
            })
            .or_else(|| {
                self.tmux
                    .as_ref()
                    .map(|pane| pane.read(cx).focus_handle(cx))
            })
            .unwrap_or_else(|| self.focus.clone())
    }
}
impl BasePanel for ToolPanel {
    fn panel_name(&self) -> &'static str {
        "helixlane.tool"
    }
    fn dump(&self, _: &App) -> PanelState {
        PanelState {
            panel_name: "helixlane.tool".into(),
            children: vec![],
            info: PanelInfo::panel(serde_json::json!(self.id)),
        }
    }
    fn set_active(&mut self, active: bool, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(files) = &self.files {
            files.update(cx, |pane, _| pane.set_visible(active));
        }
        if let Some(tmux) = &self.tmux {
            tmux.update(cx, |pane, _| pane.set_visible(active));
        }
        if active {
            let target = {
                let s = self.settings.borrow();
                s.workspace
                    .views
                    .iter()
                    .find(|v| v.id == self.id)
                    .and_then(|v| match (v.tool, v.mode) {
                        (
                            ToolKind::Terminal | ToolKind::Sftp | ToolKind::Tmux,
                            TargetMode::Fixed(id),
                        ) => Some(id),
                        _ => None,
                    })
            };
            if let Some(id) = target {
                self.settings.borrow_mut().workspace.activate_resource(id);
                cx.refresh_windows();
            }
        }
    }
}
impl Panel for ToolPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let s = self.settings.borrow();
        if let Some(v) = s.workspace.views.iter().find(|v| v.id == self.id) {
            if !v.tool.resource_scoped() {
                return s.language.text(v.tool.key()).to_string();
            }
            let target = s
                .workspace
                .target(self.id)
                .and_then(|id| s.workspace.resources.iter().find(|r| r.id == id))
                .map(|r| r.name.as_str())
                .unwrap_or("—");
            format!(
                "{} · {} · {}",
                s.language.text(v.tool.key()),
                target,
                s.language.text(if v.mode == TargetMode::Follow {
                    "follow"
                } else {
                    "fixed"
                })
            )
        } else {
            "Closed".into()
        }
    }
    fn dropdown_menu(
        &mut self,
        mut menu: PopupMenu,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> PopupMenu {
        let language = self.settings.borrow().language;
        menu = menu.label(language.text("move_to"));
        for (key, location) in [
            ("center", Location::Center),
            ("left", Location::Left),
            ("right", Location::Right),
            ("top", Location::Top),
            ("bottom", Location::Bottom),
            ("services", Location::Services),
        ] {
            menu = menu.item(self.menu_action(key, language, move |this, id, w, cx| {
                this.move_tool(id, location, w, cx)
            }));
        }
        menu = menu
            .separator()
            .item(self.menu_action("split_h", language, |this, id, w, cx| {
                this.split_tool(id, false, w, cx)
            }))
            .item(self.menu_action("split_v", language, |this, id, w, cx| {
                this.split_tool(id, true, w, cx)
            }))
            .separator()
            .label(language.text("run_policy"));
        let current = self
            .settings
            .borrow()
            .workspace
            .views
            .iter()
            .find(|v| v.id == self.id)
            .map(|v| v.run_policy);
        for (key, policy) in [
            ("background", RunPolicy::Background),
            ("visible", RunPolicy::WhileVisible),
        ] {
            menu = menu.item(
                self.menu_action(key, language, move |this, id, _, cx| {
                    if let Some(v) = this
                        .settings
                        .borrow_mut()
                        .workspace
                        .views
                        .iter_mut()
                        .find(|v| v.id == id)
                    {
                        v.run_policy = policy;
                    }
                    this.persist();
                    cx.refresh_windows();
                })
                .checked(current == Some(policy)),
            );
        }
        menu
    }
}
impl ToolPanel {
    fn menu_action(
        &self,
        key: &str,
        language: Language,
        f: impl Fn(&mut HelixLane, ViewId, &mut Window, &mut Context<HelixLane>) + 'static,
    ) -> PopupMenuItem {
        let owner = self.owner.clone();
        let id = self.id;
        PopupMenuItem::new(language.text(key)).on_click(move |_, w, cx| {
            let _ = owner.update(cx, |this, cx| f(this, id, w, cx));
        })
    }

    fn action(
        &self,
        key: &'static str,
        lang: Language,
        cx: &mut Context<Self>,
        f: impl Fn(&mut HelixLane, ViewId, &mut Window, &mut Context<HelixLane>) + 'static,
    ) -> Button {
        let owner = self.owner.clone();
        let id = self.id;
        button((key, id), lang.text(key)).on_click(cx.listener(move |_, _, w, cx| {
            let _ = owner.update(cx, |this, cx| f(this, id, w, cx));
        }))
    }
    fn services(&self, p: Palette, language: Language, cx: &mut Context<Self>) -> AnyElement {
        let by_type = self
            .owner
            .upgrade()
            .map(|o| o.read(cx).by_type)
            .unwrap_or(false);
        let state = self.settings.borrow();
        let mut tree = div()
            .id(("services-scroll", self.id))
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .p_3()
            .gap_1()
            .child(
                row()
                    .gap_2()
                    .child(self.action("by_resource", language, cx, |this, _, _, cx| {
                        this.by_type = false;
                        cx.refresh_windows();
                    }))
                    .child(self.action("by_type", language, cx, |this, _, _, cx| {
                        this.by_type = true;
                        cx.refresh_windows();
                    })),
            );
        let mut groups: Vec<(String, Vec<ViewInstance>)> = if by_type {
            ToolKind::ALL
                .into_iter()
                .map(|tool| {
                    (
                        language.text(tool.key()).to_string(),
                        state
                            .workspace
                            .views
                            .iter()
                            .filter(|v| v.tool == tool)
                            .cloned()
                            .collect(),
                    )
                })
                .collect()
        } else {
            state
                .workspace
                .resources
                .iter()
                .map(|r| {
                    (
                        r.name.clone(),
                        state
                            .workspace
                            .views
                            .iter()
                            .filter(|v| state.workspace.target(v.id) == Some(r.id))
                            .cloned()
                            .collect(),
                    )
                })
                .collect()
        };
        if !by_type {
            groups.push((
                language.text("global").into(),
                state
                    .workspace
                    .views
                    .iter()
                    .filter(|v| !v.tool.resource_scoped())
                    .cloned()
                    .collect(),
            ));
        }
        for (group, views) in groups {
            tree = tree.child(div().pt_2().text_color(rgb(p.muted)).child(group));
            for v in views {
                let owner = self.owner.clone();
                let id = v.id;
                tree = tree.child(
                    row()
                        .id(("service-view", id))
                        .flex_none()
                        .h(px(28.))
                        .px_2()
                        .gap_2()
                        .rounded(px(5.))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(p.hover)))
                        .child(
                            icon(match v.tool {
                                ToolKind::Terminal => IconName::SquareTerminal,
                                ToolKind::Sftp => IconName::Folder,
                                ToolKind::Services => IconName::LayoutDashboard,
                                ToolKind::Transfers => IconName::ArrowDown,
                                ToolKind::Information => IconName::Info,
                                ToolKind::Tmux => IconName::SquareTerminal,
                            })
                            .text_color(rgb(p.muted)),
                        )
                        .child(format!("{} #{}", language.text(v.tool.key()), id))
                        .child(div().flex_1())
                        .child(div().text_size(px(10.)).text_color(rgb(p.muted)).child(
                            language.text(match v.location {
                                Location::Center => "center",
                                Location::Left => "left",
                                Location::Right => "right",
                                Location::Top => "top",
                                Location::Bottom => "bottom",
                                Location::Services => "services",
                            }),
                        ))
                        .on_click(cx.listener(move |_, _, w, cx| {
                            let _ = owner.update(cx, |this, cx| this.focus_tool(id, w, cx));
                        })),
                );
            }
        }
        tree.into_any_element()
    }
}
impl Render for ToolPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::current(cx);
        let state = self.settings.borrow();
        let language = state.language;
        let v = state
            .workspace
            .views
            .iter()
            .find(|v| v.id == self.id)
            .cloned();
        let Some(v) = v else {
            return div().child(language.text("empty")).into_any_element();
        };
        let resource = state
            .workspace
            .target(self.id)
            .and_then(|id| state.workspace.resources.iter().find(|r| r.id == id))
            .cloned();
        drop(state);
        let tools = v.tool.resource_scoped().then(|| {
            row()
                .h(px(34.))
                .flex_none()
                .px_3()
                .gap_2()
                .border_b_1()
                .border_color(rgb(p.border))
                .child(div().text_size(px(10.)).text_color(rgb(p.muted)).child("●"))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(11.))
                        .text_color(rgb(p.muted))
                        .child(
                            resource
                                .as_ref()
                                .map(|r| {
                                    format!(
                                        "{}@{}:{}",
                                        r.profiles[0].username,
                                        r.profiles[0].host,
                                        r.profiles[0].port
                                    )
                                })
                                .unwrap_or_else(|| language.text("empty").into()),
                        ),
                )
                .child(self.action(
                    if v.mode == TargetMode::Follow {
                        "follow"
                    } else {
                        "fixed"
                    },
                    language,
                    cx,
                    |this, id, _, cx| {
                        let mut s = this.settings.borrow_mut();
                        let mode = if s
                            .workspace
                            .views
                            .iter()
                            .any(|v| v.id == id && v.mode == TargetMode::Follow)
                        {
                            s.workspace
                                .target(id)
                                .map(TargetMode::Fixed)
                                .unwrap_or(TargetMode::Follow)
                        } else {
                            TargetMode::Follow
                        };
                        s.workspace.set_mode(id, mode);
                        drop(s);
                        this.persist();
                        cx.refresh_windows();
                    },
                ))
        });
        let content = match v.tool {
            ToolKind::Services => self.services(p, language, cx),
            ToolKind::Transfers => {
                let pane = self
                    .transfers
                    .get_or_insert_with(|| {
                        cx.new(|cx| crate::file_ui::TransfersPane::new(self.settings.clone(), cx))
                    })
                    .clone();
                div().size_full().min_h_0().child(pane).into_any_element()
            }
            ToolKind::Information => {
                let mut info = div().p_5().flex().flex_col().gap_3();
                if let Some(r) = resource {
                    let profile = &r.profiles[0];
                    info = info
                        .child(r.name)
                        .child(format!(
                            "{}@{}:{}",
                            profile.username, profile.host, profile.port
                        ))
                        .child(r.notes)
                        .child(language.text("connection_profile"));
                } else {
                    info = info.child(language.text("empty"));
                }
                info.into_any_element()
            }
            ToolKind::Terminal => {
                if let Some(profile) = resource.as_ref().and_then(|r| r.profiles.first()).cloned() {
                    let terminal = self
                        .terminal
                        .get_or_insert_with(|| {
                            cx.new(|cx| {
                                crate::terminal_ui::TerminalPane::new(
                                    self.id,
                                    profile.clone(),
                                    self.settings.clone(),
                                    window,
                                    cx,
                                )
                            })
                        })
                        .clone();
                    terminal.update(cx, |pane, cx| pane.bind(profile, window, cx));
                    if let Some(request)=self.launch.take(){terminal.update(cx,|pane,cx|pane.launch(request.secret,window,cx));}
                    div().size_full().child(terminal).into_any_element()
                } else {
                    div().p_5().child(language.text("empty")).into_any_element()
                }
            }
            ToolKind::Tmux => {
                if let Some(profile) = resource.as_ref().and_then(|r| r.profiles.first()).cloned() {
                    let pane = self
                        .tmux
                        .get_or_insert_with(|| {
                            cx.new(|cx| {
                                crate::tmux_ui::TmuxPane::new(
                                    self.id,
                                    profile.clone(),
                                    self.settings.clone(),
                                    window,
                                    cx,
                                )
                            })
                        })
                        .clone();
                    pane.update(cx, |pane, cx| pane.bind(profile, window, cx));
                    div().size_full().min_h_0().child(pane).into_any_element()
                } else {
                    div().p_5().child(language.text("empty")).into_any_element()
                }
            }
            ToolKind::Sftp => {
                if let Some(profile) = resource.as_ref().and_then(|r| r.profiles.first()).cloned() {
                    let pane = self
                        .files
                        .get_or_insert_with(|| {
                            cx.new(|cx| {
                                crate::file_ui::FilePane::new(
                                    self.id,
                                    profile.clone(),
                                    self.settings.clone(),
                                    window,
                                    cx,
                                )
                            })
                        })
                        .clone();
                    pane.update(cx, |pane, cx| pane.bind(profile, window, cx));
                    if let Some(request)=self.launch.take(){pane.update(cx,|pane,cx|pane.launch(request,window,cx));}
                    div().size_full().min_h_0().child(pane).into_any_element()
                } else {
                    div().p_5().child(language.text("empty")).into_any_element()
                }
            }
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .min_h_0()
            .bg(rgb(p.panel))
            .text_color(rgb(p.text))
            .text_size(px(12.))
            .track_focus(&self.focus)
            .when_some(tools, |d, tools| d.child(tools))
            .child(content)
            .into_any_element()
    }
}
