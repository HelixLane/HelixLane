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
use helixlane::workspace::WindowId;
use helixlane::{
    i18n::Language,
    settings::{Settings, ThemePreference},
    tools::{
        ResourceCapabilities, ToolInstance, ToolProvider,
        shell::{EXAMPLE_ID, NativePlugin},
    },
    workspace::*,
};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};
pub(crate) type Shared = Rc<RefCell<Settings>>;
pub(crate) type Panels = Rc<RefCell<HashMap<ViewId, Entity<ToolPanel>>>>;
pub struct HelixLane {
    pending_close: Option<Option<ViewId>>,
    window_id: WindowId,
    application: crate::application::ApplicationOwner,
    settings: Shared,
    panels: Panels,
    dock: Entity<DockArea>,
    top: Entity<DockArea>,
    editing: Option<ResourceId>,
    fields: Vec<Entity<InputState>>,
    editor_error: Option<String>,
    error: Option<String>,
    launch_error: Option<String>,
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
    pub fn new(
        window_id: WindowId,
        application: crate::application::ApplicationOwner,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let shared = application.settings();
        let panels = application.panels();
        application.register(window_id, window.window_handle(), cx.entity().downgrade());
        apply(shared.borrow().theme, Some(window), cx);
        gpui_kit::component::set_locale(if shared.borrow().language == Language::Chinese {
            "zh-CN"
        } else {
            "en"
        });
        if window_id == 1 && shared.borrow().workspace.views.is_empty() {
            let mut state = shared.borrow_mut();
            state
                .workspace
                .open_in(window_id, ToolKind::Sftp, TargetMode::Fixed(1));
            let services =
                state
                    .workspace
                    .open_in(window_id, ToolKind::Services, TargetMode::Follow);
            state.workspace.move_view(services, Location::Bottom);
        }
        let weak = cx.entity().downgrade();
        window.on_window_should_close(cx, move |w, cx| {
            let _ = weak.update(cx, |this, cx| this.request_close(None, w, cx));
            false
        });
        let dock = cx.new(|cx| {
            DockArea::new("workbench", Some(1), window, cx).with_renderer(DockSkin::new(cx))
        });
        let top = cx.new(|cx| {
            DockArea::new("top-tools", Some(1), window, cx).with_renderer(DockSkin::new(cx))
        });
        let mut this = Self {
            pending_close: None,
            window_id,
            application,
            settings: shared,
            panels,
            dock,
            top,
            editing: None,
            fields: vec![],
            editor_error: None,
            error: None,
            launch_error: None,
            by_type: false,
            deleting: false,
            show_resources: true,
            subscriptions: vec![],
            mounted: HashSet::new(),
            layout_save: LayoutSaveGate::default(),
        };
        let (saved, saved_top) = this
            .settings
            .borrow()
            .workspace
            .window(window_id)
            .map(|w| (w.dock_layout.clone(), w.top_layout.clone()))
            .unwrap_or_default();
        let mut main_restored = false;
        for (index, (area, saved)) in [(&this.dock, saved), (&this.top, saved_top)]
            .into_iter()
            .enumerate()
        {
            if let Some(saved) = saved {
                let restored = serde_json::from_value::<DockAreaState>(saved)
                    .ok()
                    .is_some_and(|state| {
                        fn valid(
                            panel: &PanelState,
                            owner: WindowId,
                            workspace: &Workspace,
                            seen: &mut HashSet<ViewId>,
                        ) -> bool {
                            if panel.panel_name == "helixlane.tool" {
                                let PanelInfo::Panel(value) = &panel.info else {
                                    return false;
                                };
                                let Some(id) = value.as_u64() else {
                                    return false;
                                };
                                if workspace.owner(id) != Some(owner) || !seen.insert(id) {
                                    return false;
                                }
                            }
                            panel
                                .children
                                .iter()
                                .all(|p| valid(p, owner, workspace, seen))
                        }
                        let mut seen = HashSet::new();
                        let valid = {
                            let settings = this.settings.borrow();
                            valid(&state.center, window_id, &settings.workspace, &mut seen)
                                && [&state.left_dock, &state.right_dock, &state.bottom_dock]
                                    .into_iter()
                                    .flatten()
                                    .all(|dock| {
                                        valid(
                                            dock.panel(),
                                            window_id,
                                            &settings.workspace,
                                            &mut seen,
                                        )
                                    })
                        };
                        valid && area.update(cx, |d, cx| d.load(state, window, cx).is_ok())
                    });
                if index == 0 {
                    main_restored = restored;
                }
                if !restored {
                    this.settings.borrow_mut().recovery = Some("recovered_layout".into());
                }
            }
        }
        // Restoration can succeed in one region and fail in another. Reconcile every
        // model instance against the panels that actually made it into either tree.
        let live = this.live_locations(cx);
        let missing = this.settings.borrow().workspace.unmounted_views(&live);
        for v in missing.into_iter().filter(|v| v.window_id == window_id) {
            this.mount(v.id, window, cx);
        }
        if !main_restored {
            let model = {
                let state = this.settings.borrow();
                if window_id == 1 {
                    state.workspace.layout.clone()
                } else {
                    state
                        .workspace
                        .window(window_id)
                        .map(|w| w.layout.clone())
                        .unwrap_or_default()
                }
            };
            let layout = this.restore_tree(&model, window.bounds().size, cx);
            this.dock
                .update(cx, |d, cx| d.set_center(layout, window, cx));
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
    fn restore_tree(&self, node: &LayoutNode, bounds: Size<Pixels>, cx: &App) -> DockLayout {
        match node {
            LayoutNode::Tabs { views, active } => {
                let panels = self.panels.borrow();
                let mut layout = DockLayout::tabs();
                let mut kept = vec![];
                for id in views {
                    if self.settings.borrow().workspace.owner(*id) != Some(self.window_id) {
                        continue;
                    }
                    if let Some(panel) = panels.get(id) {
                        kept.push(*id);
                        layout = layout.panel_view(panel_handle(panel.clone()), cx);
                    }
                }
                layout.active_index(
                    active
                        .and_then(|id| kept.iter().position(|v| *v == id))
                        .unwrap_or(0),
                )
            }
            LayoutNode::Split {
                vertical,
                ratio,
                first,
                second,
            } => {
                let ratio = if ratio.is_finite() {
                    ratio.clamp(0.05, 0.95)
                } else {
                    0.5
                };
                let total = if *vertical {
                    bounds.height
                } else {
                    bounds.width
                };
                (if *vertical {
                    DockLayout::v_split()
                } else {
                    DockLayout::h_split()
                })
                .child(self.restore_tree(first, bounds, cx), Some(total * ratio))
                .child(
                    self.restore_tree(second, bounds, cx),
                    Some(total * (1. - ratio)),
                )
            }
        }
    }
    pub fn dispatch_launch(
        &mut self,
        request: helixlane::launch::OpenRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if request.target == helixlane::launch::Target::Focus {
            window.activate_window();
            cx.activate(true);
            return;
        }
        self.launch_error = None;
        let opened =
            match request.apply_in(&mut self.settings.borrow_mut().workspace, self.window_id) {
                Ok(opened) => opened,
                Err(error) => {
                    self.launch_error = Some(error);
                    cx.notify();
                    return;
                }
            };
        if !self.panels.borrow().contains_key(&opened.view_id) {
            self.mount(opened.view_id, window, cx);
        }
        if let Some(panel) = self.panels.borrow().get(&opened.view_id).cloned() {
            panel.update(cx, |panel, cx| {
                panel.launch = Some(request);
                cx.notify();
            });
        }
        self.focus_tool(opened.view_id, window, cx);
        self.persist_layout(cx);
        window.activate_window();
        cx.activate(true);
        cx.notify();
    }
    pub(crate) fn receive_view(
        &mut self,
        id: ViewId,
        location: Location,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = self.panels.borrow().get(&id).cloned();
        if let Some(panel) = panel {
            let owner = cx.entity().downgrade();
            panel.update(cx, |panel, cx| {
                panel.owner = owner;
                if let Some(p) = &panel.files {
                    p.update(cx, |p, cx| p.rebind_window(window, cx));
                }
                if let Some(p) = &panel.tmux {
                    p.update(cx, |p, cx| p.rebind_window(window, cx));
                }
                if let Some(p) = &panel.terminal {
                    p.update(cx, |p, cx| p.rebind_window(window, cx));
                }
            });
        }
        self.move_tool(id, location, window, cx);
        self.focus_tool(id, window, cx);
    }
    pub(crate) fn move_window(
        &mut self,
        id: ViewId,
        destination: Option<WindowId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let source = if self
            .settings
            .borrow()
            .workspace
            .views
            .iter()
            .any(|v| v.id == id && matches!(v.tool, ToolKind::Terminal | ToolKind::Sftp))
        {
            None
        } else {
            Some(self.window_id)
        };
        let destination = match destination {
            Some(id) => id,
            None => match self.application.create(source, cx) {
                Ok(id) => id,
                Err(error) => {
                    self.error = Some(error);
                    return;
                }
            },
        };
        if destination == self.window_id {
            return;
        }
        let Some(panel) = self.panels.borrow().get(&id).cloned() else {
            return;
        };
        self.mounted.remove(&id);
        self.dock
            .update(cx, |d, cx| d.remove_panel(panel.clone(), window, cx));
        self.top
            .update(cx, |d, cx| d.remove_panel(panel.clone(), window, cx));
        self.settings
            .borrow_mut()
            .workspace
            .move_to_window(id, destination, Location::Center);
        if let Some((handle, owner)) = self.application.window(destination) {
            let _ = handle.update(cx, |_, w, cx| {
                let _ = owner.update(cx, |dest, cx| {
                    panel.update(cx, |panel, cx| {
                        panel.owner = owner.clone();
                        if let Some(pane) = &panel.files {
                            pane.update(cx, |pane, cx| pane.rebind_window(w, cx));
                        }
                        if let Some(pane) = &panel.tmux {
                            pane.update(cx, |pane, cx| pane.rebind_window(w, cx));
                        }
                        if let Some(pane) = &panel.terminal {
                            pane.update(cx, |pane, cx| pane.rebind_window(w, cx));
                        }
                    });
                    dest.mount_new(id, Location::Center, w, cx);
                    dest.focus_tool(id, w, cx);
                    dest.persist_layout(cx);
                    w.activate_window();
                });
            });
        }
        self.persist_layout(cx);
        cx.refresh_windows();
    }
    fn finish_close(&mut self, cancel: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(scope) = self.pending_close.take() else {
            return;
        };
        let ids = self
            .settings
            .borrow()
            .workspace
            .views
            .iter()
            .filter(|v| scope.map_or(v.window_id == self.window_id, |id| v.id == id))
            .map(|v| v.id)
            .collect::<Vec<_>>();
        if cancel {
            helixlane::tasks::TransferManager::global().cancel_views(&ids);
        }
        for id in ids {
            let panel = self.panels.borrow().get(&id).cloned();
            if let Some(panel) = panel {
                if cancel {
                    panel.update(cx, |p, cx| p.cancel_owned_jobs(cx));
                }
                self.mounted.remove(&id);
                self.dock
                    .update(cx, |d, cx| d.remove_panel(panel.clone(), window, cx));
                self.top
                    .update(cx, |d, cx| d.remove_panel(panel, window, cx));
                self.settings
                    .borrow_mut()
                    .workspace
                    .move_view(id, Location::Services);
            }
        }
        if cancel {
            let ids = self
                .settings
                .borrow()
                .workspace
                .views
                .iter()
                .filter(|v| scope.map_or(v.window_id == self.window_id, |id| id == v.id))
                .map(|v| v.id)
                .collect::<Vec<_>>();
            for id in ids {
                self.settings.borrow_mut().workspace.close(id);
                self.panels.borrow_mut().remove(&id);
            }
        }
        self.snapshot_layout(cx);
        if scope.is_none() {
            self.application.closed(self.window_id, cx);
            window.remove_window();
        }
        cx.refresh_windows();
    }
    pub(crate) fn request_close(
        &mut self,
        view: Option<ViewId>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_close = Some(view);
        cx.notify();
    }
    fn text(&self, key: &str) -> &'static str {
        self.settings.borrow().language.text(key)
    }
    fn persist(&mut self) {
        self.error = self.application.save().err();
    }
    fn persist_layout(&mut self, cx: &mut Context<Self>) {
        self.layout_save.request();
        cx.notify();
    }
    pub(crate) fn has_view(&self, id: ViewId, cx: &App) -> bool {
        self.live_locations(cx).contains_key(&id)
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
        let pages = self
            .panels
            .borrow()
            .iter()
            .filter_map(|(id, panel)| {
                panel
                    .read(cx)
                    .files
                    .as_ref()
                    .map(|file| (*id, file.read(cx).page_state()))
            })
            .collect::<Vec<_>>();
        {
            let mut state = self.settings.borrow_mut();
            for (id, page) in pages {
                if let Some(view) = state.workspace.views.iter_mut().find(|v| v.id == id) {
                    view.page = page;
                }
            }
        }
        let main = self.dock.read(cx).dump(cx);
        let top = self.top.read(cx).dump(cx);
        self.sync_layout(&main, &top);
        {
            let mut s = self.settings.borrow_mut();
            if let Some(w) = s
                .workspace
                .windows
                .iter_mut()
                .find(|w| w.id == self.window_id)
            {
                w.dock_layout = serde_json::to_value(main).ok();
                w.top_layout = serde_json::to_value(top).ok();
            }
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
        if self.window_id == 1 {
            s.workspace.layout = model(&main.center);
        }
        if let Some(w) = s
            .workspace
            .windows
            .iter_mut()
            .find(|w| w.id == self.window_id)
        {
            w.layout = model(&main.center);
        }
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
            let id = self.settings.borrow_mut().workspace.open_in(
                self.window_id,
                ToolKind::Sftp,
                TargetMode::Fixed(resource),
            );
            let owner = cx.entity().downgrade();
            let p = cx.new(|cx| ToolPanel::new(id, self.settings.clone(), owner, cx));
            self.panels.borrow_mut().insert(id, p.clone());
            groups.push((id, DockLayout::tabs().panel_view(panel_handle(p), cx)));
        }
        for v in &mut self.settings.borrow_mut().workspace.views {
            if v.window_id == self.window_id
                && v.location == Location::Center
                && !groups.iter().any(|(id, _)| *id == v.id)
            {
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
        let panel = existing
            .unwrap_or_else(|| cx.new(|pcx| ToolPanel::new(id, self.settings.clone(), owner, pcx)));
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
        let id = self
            .settings
            .borrow_mut()
            .workspace
            .open_in(self.window_id, tool, mode);
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
    pub(crate) fn move_tool(
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
        let location = self
            .settings
            .borrow()
            .workspace
            .views
            .iter()
            .find(|v| v.id == id)
            .map(|v| v.location)
            .unwrap_or(Location::Center);
        let area = if location == Location::Top {
            self.top.clone()
        } else {
            self.dock.clone()
        };
        let target = area.read(cx).layout(placement(location)).and_then(|tree| {
            tree.panels()
                .find(|p| *p != pid)
                .and_then(|other| tree.find_panel_node(other))
        });
        if let Some(target) = target {
            area.update(cx, |d, cx| {
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
            if location == Location::Center {
                self.settings
                    .borrow_mut()
                    .workspace
                    .split_in(self.window_id, id, vertical);
            }
            self.persist_layout(cx);
            cx.refresh_windows();
        }
    }
    pub(crate) fn focus_tool(&mut self, id: ViewId, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.borrow().workspace.owner(id) != Some(self.window_id) {
            self.application.focus(id, cx);
            return;
        }

        if !self.has_view(id, cx) {
            self.receive_view(id, Location::Center, window, cx);
            return;
        }
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
        self.settings
            .borrow_mut()
            .workspace
            .activate_content_view(id);
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
        let selected = state.workspace.window_target(self.window_id);
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
                        this.settings
                            .borrow_mut()
                            .workspace
                            .activate_resource_in(this.window_id, id);
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
            (6, ToolKind::Plugin, IconName::Code),
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
        for panel in self.panels.borrow().values() {
            panel.update(cx, |panel, cx| panel.update_plugin_activity(cx));
        }

        if self.layout_save.after_render() {
            let owner = cx.entity().downgrade();
            // Schedule from rendering, so the edited frame completes layout and
            // Kit has actual pixel measurements before a layout snapshot is taken.
            window.on_next_frame(move |window, cx| {
                let _ = owner.update(cx, |this, cx| {
                    if this.layout_save.after_frame() {
                        let bounds = window.bounds();
                        if let Some(w) = this
                            .settings
                            .borrow_mut()
                            .workspace
                            .windows
                            .iter_mut()
                            .find(|w| w.id == this.window_id)
                        {
                            w.bounds = Some([
                                bounds.origin.x.into(),
                                bounds.origin.y.into(),
                                bounds.size.width.into(),
                                bounds.size.height.into(),
                            ]);
                        }
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
                button("new-window", language.text("new_window")).on_click(cx.listener(
                    |this, _, _, cx| {
                        if let Err(error) = this.application.create(None, cx) {
                            this.error = Some(error);
                        }
                    },
                )),
            )
            .child(
                button("close-window", language.text("close_window"))
                    .on_click(cx.listener(|this, _, w, cx| this.request_close(None, w, cx))),
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
            .any(|v| v.window_id == self.window_id && v.location == Location::Top);
        let center = if has_top {
            v_resizable("top-and-center")
                .on_resize({
                    let owner = cx.entity().downgrade();
                    move |state, _, cx| {
                        let height = state.read(cx).sizes().first().map(|v| f32::from(*v));
                        if let Some(height) = height {
                            let _ = owner.update(cx, |this, cx| {
                                if let Some(w) = this
                                    .settings
                                    .borrow_mut()
                                    .workspace
                                    .windows
                                    .iter_mut()
                                    .find(|w| w.id == this.window_id)
                                {
                                    w.top_height = height;
                                }
                                this.persist_layout(cx);
                            });
                        }
                    }
                })
                .child(
                    resizable_panel()
                        .size(px(self
                            .settings
                            .borrow()
                            .workspace
                            .window(self.window_id)
                            .map(|w| w.top_height)
                            .filter(|v| v.is_finite())
                            .unwrap_or(180.)
                            .max(100.)))
                        .child(self.top.clone()),
                )
                .child(resizable_panel().child(self.dock.clone()))
                .into_any_element()
        } else {
            div()
                .size_full()
                .child(self.dock.clone())
                .into_any_element()
        };
        let mut frame = div()
            .on_drag_move(cx.listener(
                |this, event: &DragMoveEvent<gpui_kit::base::dock::DragPanel>, _, cx| {
                    let panel = event.drag(cx).panel();
                    if let Some(id) = this
                        .panels
                        .borrow()
                        .iter()
                        .find(|(_, p)| PanelId::from(p.entity_id()) == panel)
                        .map(|(id, _)| *id)
                    {
                        this.application.begin_drag(id);
                    }
                },
            ))
            .capture_any_mouse_up(cx.listener(|this, event, w, cx| {
                this.application.finish_cross_window_drag(event, w, cx);
            }))
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, event, w, cx| {
                    this.application.finish_cross_window_drag(event, w, cx);
                }),
            )
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
        let mut drop_targets = row().h(px(26.)).px_3().gap_2();
        for (key, location) in [
            ("left", Location::Left),
            ("center", Location::Center),
            ("right", Location::Right),
            ("top", Location::Top),
            ("bottom", Location::Bottom),
        ] {
            let app = self.application.clone();
            let destination = self.window_id;
            drop_targets = drop_targets.child(
                div()
                    .id(("dock-drop", location as usize))
                    .relative()
                    .child(
                        canvas(
                            move |bounds, window, _| {
                                let bounds = Bounds::new(
                                    window.bounds().origin + bounds.origin,
                                    bounds.size,
                                );
                                app.measure_drop_target(destination, location, bounds);
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .px_2()
                    .rounded(px(4.))
                    .text_size(px(11.))
                    .text_color(rgb(p.muted))
                    .child(language.text(key))
                    .drag_over::<gpui_kit::base::dock::DragPanel>(move |style, _, _, _| {
                        style.bg(rgb(p.selected))
                    })
                    .on_drop(cx.listener(
                        move |this, drag: &gpui_kit::base::dock::DragPanel, _, cx| {
                            let id = this
                                .panels
                                .borrow()
                                .iter()
                                .find(|(_, panel)| PanelId::from(panel.entity_id()) == drag.panel())
                                .map(|(id, _)| *id);
                            if let Some(id) = id {
                                let app = this.application.clone();
                                let destination = this.window_id;
                                cx.defer(move |cx| app.relocate(id, destination, location, cx));
                            }
                        },
                    )),
            );
        }
        frame = frame.child(drop_targets);
        let message = self
            .launch_error
            .clone()
            .or_else(|| {
                self.error
                    .as_ref()
                    .map(|detail| format!("{}: {detail}", language.text("settings_error")))
            })
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
        if let Some(scope) = self.pending_close {
            let scope_label = scope
                .map(|id| format!("{} #{id}", language.text("close_view")))
                .unwrap_or_else(|| format!("{} {}", language.text("close_window"), self.window_id));
            frame = frame.child(
                div()
                    .flex()
                    .flex_col()
                    .px_4()
                    .py_2()
                    .gap_2()
                    .bg(rgb(p.selected))
                    .child(
                        div()
                            .child(scope_label)
                            .child(" · ")
                            .child(language.text("close_scope")),
                    )
                    .child(
                        row()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                button("continue-background", language.text("continue_background"))
                                    .on_click(cx.listener(|this, _, w, cx| {
                                        this.finish_close(false, w, cx)
                                    })),
                            )
                            .child(
                                button("cancel-owned", language.text("cancel_owned")).on_click(
                                    cx.listener(|this, _, w, cx| this.finish_close(true, w, cx)),
                                ),
                            )
                            .child(button("keep-open", language.text("cancel")).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.pending_close = None;
                                    cx.notify();
                                }),
                            )),
                    ),
            );
        }
        if self
            .settings
            .borrow()
            .workspace
            .window(self.window_id)
            .is_some_and(|w| w.follow_paused)
        {
            let owner = cx.entity().downgrade();
            let application = self.application.clone();
            frame = frame.child(
                row()
                    .px_4()
                    .py_2()
                    .gap_2()
                    .flex_wrap()
                    .child(div().w_full().child(language.text("follow_paused")))
                    .child(
                        Button::new("choose-source")
                            .ghost()
                            .label(language.text("choose_source"))
                            .dropdown_menu(move |mut menu, _, _| {
                                for source in application.windows() {
                                    let owner = owner.clone();
                                    menu =
                                        menu.item(
                                            PopupMenuItem::new(format!(
                                                "{} {source}",
                                                language.text("window")
                                            ))
                                            .on_click(move |_, _, cx| {
                                                let _ = owner.update(cx, |this, cx| {
                                                    this.settings
                                                        .borrow_mut()
                                                        .workspace
                                                        .set_source(this.window_id, source);
                                                    this.persist();
                                                    cx.refresh_windows();
                                                });
                                            }),
                                        );
                                }
                                menu
                            }),
                    ),
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
struct ToolDragPreview(String);
impl Render for ToolDragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::current(cx);
        div()
            .px_3()
            .py_2()
            .bg(rgb(p.selected))
            .text_color(rgb(p.text))
            .rounded(px(5.))
            .child(self.0.clone())
    }
}
pub(crate) struct ToolPanel {
    active: bool,
    window_visible: bool,
    visibility: Option<Subscription>,
    focus_subscription: Option<Subscription>,
    drag_node: Option<NodeId>,
    id: ViewId,
    settings: Shared,
    owner: WeakEntity<HelixLane>,
    focus: FocusHandle,
    terminal: Option<Entity<crate::terminal_ui::TerminalPane>>,
    files: Option<Entity<crate::file_ui::FilePane>>,
    tmux: Option<Entity<crate::tmux_ui::TmuxPane>>,
    transfers: Option<Entity<crate::file_ui::TransfersPane>>,
    launch: Option<helixlane::launch::OpenRequest>,
    instance: Option<ToolInstance>,
    plugin: Option<NativePlugin>,
    plugin_error: Option<String>,
    plugin_language: Option<Language>,
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
    fn on_added_to(
        &mut self,
        group: WeakEntity<gpui_kit::base::dock::TabGroup>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = cx.entity().downgrade();
        cx.defer(move |cx| {
            if let Some(group) = group.upgrade() {
                let node = group.read(cx).node();
                let _ = panel.update(cx, |panel, cx| {
                    panel.drag_node = Some(node);
                    cx.notify();
                });
            }
        });
    }
    fn closable(&self, _: &App) -> bool {
        false
    }
    fn on_removed(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self.set_active(false, w, cx);
    }
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
    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.active = active;
        self.window_visible = window.is_visible();
        self.visibility = Some(cx.observe_window_visibility(window, |this, v, _, cx| {
            this.window_visible = v.is_visible();
            this.update_plugin_activity(cx);
            cx.notify();
        }));
        self.update_plugin_activity(cx);
        if let Some(files) = &self.files {
            files.update(cx, |pane, _| pane.set_visible(active));
        }
        if let Some(tmux) = &self.tmux {
            tmux.update(cx, |pane, _| pane.set_visible(active));
        }
        self.focus_subscription = Some(cx.on_focus_in(&self.focus, window, |this, _, cx| {
            if this
                .settings
                .borrow_mut()
                .workspace
                .activate_content_view(this.id)
            {
                cx.refresh_windows();
            }
        }));
        if active
            && self
                .settings
                .borrow_mut()
                .workspace
                .activate_content_view(self.id)
        {
            cx.refresh_windows();
        }
    }
}
impl Panel for ToolPanel {
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = self.label();
        let id = self.id;
        let application = cx.global::<crate::application::ApplicationOwner>().clone();
        let drag = self
            .drag_node
            .map(|node| gpui_kit::base::dock::DragPanel::new(PanelId::from(cx.entity_id()), node));
        div()
            .id(("view-title-drag", self.id))
            .cursor_grab()
            .child(title.clone())
            .when_some(drag, |element, drag| {
                element.on_drag(drag, move |drag, offset, _, cx| {
                    cx.stop_propagation();
                    application.begin_drag(id);
                    drag.set_drag_offset(offset);
                    drag.set_preview_size(size(px(300.), px(32.)));
                    cx.new(|_| ToolDragPreview(title.clone()))
                })
            })
    }
    fn dropdown_menu(
        &mut self,
        mut menu: PopupMenu,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let language = self.settings.borrow().language;
        menu = menu.item(
            self.menu_action("new_fixed_view", language, |this, id, w, cx| {
                let target = {
                    let state = this.settings.borrow();
                    state.workspace.target(id).and_then(|target| {
                        state
                            .workspace
                            .views
                            .iter()
                            .find(|v| v.id == id)
                            .map(|v| (v.tool, target))
                    })
                };
                if let Some((tool, target)) = target {
                    this.open(tool, TargetMode::Fixed(target), w, cx);
                }
            }),
        );
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
        menu =
            menu.separator()
                .item(self.menu_action("new_window", language, |this, id, w, cx| {
                    this.move_window(id, None, w, cx)
                }));
        if let Some(owner) = self.owner.upgrade() {
            for destination in owner.read(cx).application.windows() {
                let current = self.settings.borrow().workspace.owner(self.id);
                if current == Some(destination) {
                    continue;
                }
                let owner = self.owner.clone();
                let id = self.id;
                menu = menu.item(
                    PopupMenuItem::new(format!("{} {destination}", language.text("window")))
                        .on_click(move |_, w, cx| {
                            let _ = owner.update(cx, |this, cx| {
                                this.move_window(id, Some(destination), w, cx)
                            });
                        }),
                );
            }
        }
        menu = menu.item(self.menu_action("close_view", language, |this, id, w, cx| {
            this.request_close(Some(id), w, cx)
        }));
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
    fn label(&self) -> String {
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

    fn update_plugin_activity(&self, cx: &mut App) {
        let active = self.settings.borrow().workspace.should_refresh(
            self.id,
            self.active,
            !self.window_visible,
        );
        if let Some(plugin) = &self.plugin {
            plugin.set_collection_active(active, cx);
        }
    }
    fn cancel_owned_jobs(&mut self, cx: &mut Context<Self>) {
        if let Some(pane) = &self.terminal {
            pane.update(cx, |pane, _| pane.cancel_owned_jobs());
        }
        if let Some(pane) = &self.tmux {
            pane.update(cx, |pane, cx| pane.cancel_owned_jobs(cx));
        }
        if let Some(pane) = &self.files {
            pane.update(cx, |pane, cx| pane.cancel_owned_jobs(cx));
        }
    }
    pub(crate) fn new(
        id: ViewId,
        settings: Shared,
        owner: WeakEntity<HelixLane>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            active: true,
            window_visible: true,
            visibility: None,
            focus_subscription: None,
            drag_node: None,
            id,
            settings,
            owner,
            focus: cx.focus_handle(),
            terminal: None,
            files: None,
            tmux: None,
            transfers: None,
            launch: None,
            instance: None,
            plugin: None,
            plugin_error: None,
            plugin_language: None,
        }
    }

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
    fn plugin_content(
        &mut self,
        view: &ViewInstance,
        language: Language,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.plugin_language != Some(language) {
            self.plugin.take();
            self.plugin_error = None;
            self.plugin_language = Some(language);
        }
        if !view.plugin_enabled {
            self.plugin.take();
        }
        if view.plugin_enabled && self.plugin.is_none() && self.plugin_error.is_none() {
            let instance = ToolProvider::plugin(EXAMPLE_ID, ResourceCapabilities::metadata())
                .instantiate(self.id, &self.settings.borrow().workspace);
            match instance.map_err(str::to_owned).and_then(|mut instance| {
                let settings = self.settings.clone();
                let id = self.id;
                instance.bind_collection_context(move || {
                    ToolProvider::plugin(EXAMPLE_ID, ResourceCapabilities::metadata())
                        .instantiate(id, &settings.borrow().workspace)
                        .and_then(|next| next.resource_context())
                });
                NativePlugin::mount(instance, language, window, cx)
            }) {
                Ok(plugin) => self.plugin = Some(plugin),
                Err(error) => self.plugin_error = Some(error),
            }
        }
        self.update_plugin_activity(cx);
        let p = Palette::current(cx);
        let enabled = view.plugin_enabled;
        let mut content = div().flex().flex_col().size_full().min_h_0().gap_2();
        let mut controls = row().px_3().py_2().gap_2().child(
            button(
                ("plugin-toggle", self.id),
                language.text(if enabled {
                    "plugin_unload"
                } else {
                    "plugin_enable"
                }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.plugin.take();
                this.plugin_error = None;
                if let Some(view) = this
                    .settings
                    .borrow_mut()
                    .workspace
                    .views
                    .iter_mut()
                    .find(|view| view.id == this.id)
                {
                    view.plugin_enabled = !enabled;
                }
                let _ = this.owner.update(cx, |owner, _| owner.persist());
                cx.notify();
            })),
        );
        if enabled {
            controls = controls.child(
                button(("plugin-reload", self.id), language.text("plugin_retry")).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.plugin.take();
                        this.plugin_error = None;
                        cx.notify();
                    }),
                ),
            );
        }
        content = content.child(controls);
        if let Some(error) = &self.plugin_error {
            content = content
                .child(
                    div()
                        .px_3()
                        .text_color(rgb(p.muted))
                        .child(language.text("plugin_failed")),
                )
                .child(div().px_3().child(error.clone()));
        } else if let Some(plugin) = &self.plugin {
            content = content.child(
                div()
                    .px_3()
                    .text_size(px(11.))
                    .text_color(rgb(p.muted))
                    .child(language.text("plugin_running")),
            );
            if let Some(script) = plugin.view() {
                content = content.child(div().flex_1().min_h_0().child(script));
            }
        } else {
            content = content.child(div().p_3().child(language.text("plugin_disabled")));
        }
        content.into_any_element()
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
                                ToolKind::Plugin => IconName::Code,
                            })
                            .text_color(rgb(p.muted)),
                        )
                        .child(format!(
                            "{} #{} · {} {}",
                            language.text(v.tool.key()),
                            id,
                            language.text("window"),
                            v.window_id
                        ))
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
        let provider = if v.tool == ToolKind::Plugin {
            ToolProvider::plugin(EXAMPLE_ID, ResourceCapabilities::metadata())
        } else {
            ToolProvider::builtin(v.tool)
        };
        if let Ok(next) = provider.instantiate(self.id, &state.workspace)
            && self.instance.as_ref().is_none_or(|instance| {
                instance.context() != next.context()
                    || instance.native_target() != next.native_target()
            })
        {
            self.instance = Some(next);
            // Resource collection resolves its current window context at each call;
            // retain the VM and its independently captured user jobs on retarget.
            self.plugin_error = None;
        }
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
                                .or_else(|| {
                                    s.workspace
                                        .window(this.window_id)
                                        .and_then(|w| w.active_resource)
                                })
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
            ToolKind::Plugin => self.plugin_content(&v, language, window, cx),
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
                if let Some(profile) = self
                    .instance
                    .as_ref()
                    .and_then(|instance| instance.native_target())
                    .map(|target| target.profile.clone())
                {
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
                    if let Some(request) = self.launch.take() {
                        terminal.update(cx, |pane, cx| pane.launch(request.secret, window, cx));
                    }
                    div().size_full().child(terminal).into_any_element()
                } else {
                    div().p_5().child(language.text("empty")).into_any_element()
                }
            }
            ToolKind::Tmux => {
                if let Some(profile) = self
                    .instance
                    .as_ref()
                    .and_then(|instance| instance.native_target())
                    .map(|target| target.profile.clone())
                {
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
                if let Some(profile) = self
                    .instance
                    .as_ref()
                    .and_then(|instance| instance.native_target())
                    .map(|target| target.profile.clone())
                {
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
                    if let Some(request) = self.launch.take() {
                        pane.update(cx, |pane, cx| pane.launch(request, window, cx));
                    }
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

#[cfg(all(test, feature = "shell-tests"))]
mod window_regression_tests {
    use crate::application::ApplicationOwner;
    use gpui_kit as gpui;
    use helixlane::{
        settings::Settings,
        workspace::{TargetMode, ToolKind},
    };

    #[gpui::test]
    fn services_focus_remounts_background_entity_after_its_native_window_closes(
        cx: &mut gpui::TestAppContext,
    ) {
        let (owner, id, original) = cx.update(|cx| {
            gpui_shell::init(cx);
            gpui_kit::init(cx);
            let mut settings = Settings::default();
            settings.workspace.ensure_windows();
            let id = settings
                .workspace
                .open(ToolKind::Sftp, TargetMode::Fixed(1));
            let other = settings.workspace.create_window(None);
            let owner = ApplicationOwner::install_settings(settings, cx);
            owner.open(1, cx).unwrap();
            owner.open(other, cx).unwrap();
            let original = owner.panels().borrow().get(&id).unwrap().entity_id();
            let (handle, view) = owner.window(1).unwrap();
            handle
                .update(cx, |_, w, cx| {
                    view.update(cx, |v, cx| v.move_window(id, Some(other), w, cx))
                })
                .unwrap()
                .unwrap();
            let (handle, view) = owner.window(other).unwrap();
            handle
                .update(cx, |_, w, cx| {
                    view.update(cx, |v, cx| {
                        v.request_close(None, w, cx);
                        v.finish_close(false, w, cx);
                    })
                })
                .unwrap()
                .unwrap();
            (owner, id, original)
        });
        cx.update(|cx| {
            let (handle, view) = owner.window(1).unwrap();
            handle
                .update(cx, |_, w, cx| {
                    view.update(cx, |v, cx| v.focus_tool(id, w, cx))
                })
                .unwrap()
                .unwrap();
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let (_, view) = owner.window(1).unwrap();
            assert!(
                view.upgrade().unwrap().read(cx).has_view(id, cx),
                "logical ownership changed but no native dock contains the panel"
            );
            assert_eq!(
                owner.panels().borrow().get(&id).unwrap().entity_id(),
                original
            );
            assert_eq!(owner.settings().borrow().workspace.owner(id), Some(1));
        });
    }
}
