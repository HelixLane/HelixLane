#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_top_restore_is_recovered_without_being_mistaken_for_close() {
        let mut w = Workspace::demo_fixture();
        let central = w.open(ToolKind::Sftp, TargetMode::Fixed(1));
        let top = w.open(ToolKind::Terminal, TargetMode::Fixed(2));
        w.move_view(top, Location::Top);
        let mounted = std::collections::HashMap::from([(central, Location::Center)]);
        let missing = w.unmounted_views(&mounted);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].id, top);
        w.reconcile_view_locations(&mounted, &std::collections::HashSet::from([central]));
        assert!(
            w.views
                .iter()
                .any(|v| v.id == top && v.location == Location::Top)
        );
        let all =
            std::collections::HashMap::from([(central, Location::Center), (top, Location::Top)]);
        w.reconcile_view_locations(&all, &mounted.keys().copied().collect());
        w.reconcile_view_locations(&mounted, &all.keys().copied().collect());
        assert!(
            !w.views.iter().any(|v| v.id == top),
            "a previously mounted view can still be deliberately closed"
        );
    }
    #[test]
    fn layout_snapshot_waits_for_render_and_coalesces_intervening_edits() {
        let mut gate = LayoutSaveGate::default();
        gate.request();
        assert!(gate.after_render());
        assert!(!gate.after_render());
        gate.request();
        assert!(!gate.after_frame());
        assert!(gate.after_render());
        assert!(gate.after_frame());
        assert!(!gate.after_render());
    }
    #[test]
    fn resource_form_requires_explicit_username() {
        assert!(validate_resource_form("Host", "host.invalid", " ", "22").is_err());
        assert_eq!(
            validate_resource_form("Host", "host.invalid", "user", "22"),
            Ok(22)
        );
    }
    #[test]
    fn aggregate_tools_do_not_follow_resources() {
        let mut w = Workspace::demo_fixture();
        let id = w.open(ToolKind::Services, TargetMode::Follow);
        w.activate_resource(2);
        assert_eq!(w.target(id), None);
        assert_eq!(w.views[0].generation, 0);
    }
    #[test]
    fn moving_keeps_identity_and_session() {
        let mut w = Workspace::demo_fixture();
        let id = w.open(ToolKind::Sftp, TargetMode::Fixed(1));
        w.views[0].session_id = Some(9);
        w.move_view(id, Location::Right);
        assert_eq!(w.views[0].id, id);
        assert_eq!(w.views[0].session_id, Some(9));
    }
    #[test]
    fn fixed_ignores_follow_and_late_results_are_rejected() {
        let mut w = Workspace::demo_fixture();
        let fixed = w.open(ToolKind::Sftp, TargetMode::Fixed(1));
        let follow = w.open(ToolKind::Terminal, TargetMode::Follow);
        w.activate_resource(1);
        let old = w.ticket(follow).unwrap();
        w.activate_resource(2);
        assert_eq!(w.target(fixed), Some(1));
        assert!(!w.accepts(old));
        let snapshot = w.ticket(follow).unwrap();
        w.activate_resource(3);
        assert_eq!(snapshot.resource_id, 2);
    }
    #[test]
    fn task_submission_is_immutable_and_closing_view_keeps_snapshot() {
        let mut w = Workspace::demo_fixture();
        let id = w.open(ToolKind::Sftp, TargetMode::Follow);
        let submitted = w.capture_task_target(id).unwrap();
        w.activate_resource(2);
        w.resources[0].profiles[0].host = "edited.example.invalid".into();
        w.close(id);
        assert_eq!(submitted.resource_id, 1);
        assert_eq!(submitted.profile.host, "host-1.example.invalid");
    }
    #[test]
    fn removed_resource_is_unavailable_and_visibility_only_pauses_refresh() {
        let mut w = Workspace::demo_fixture();
        let id = w.open(ToolKind::Terminal, TargetMode::Fixed(1));
        w.views[0].session_id = Some(44);
        assert!(!w.should_refresh(id, false, false));
        assert_eq!(w.views[0].session_id, Some(44));
        w.views[0].run_policy = RunPolicy::Background;
        assert!(w.should_refresh(id, false, true));
        w.resources.clear();
        assert_eq!(w.target(id), None);
    }
    #[test]
    fn layout_roundtrip() {
        let mut w = Workspace::demo_fixture();
        let a = w.open(ToolKind::Sftp, TargetMode::Fixed(1));
        w.move_view(a, Location::Top);
        let encoded = serde_json::to_string(&w).unwrap();
        assert_eq!(w, serde_json::from_str(&encoded).unwrap());
    }
}
/// Coalesces layout edits and permits a snapshot only after the edited frame was measured.
#[derive(Default)]
pub struct LayoutSaveGate {
    dirty: bool,
    scheduled: bool,
}
impl LayoutSaveGate {
    pub fn request(&mut self) {
        self.dirty = true;
    }
    pub fn after_render(&mut self) -> bool {
        if !self.dirty || self.scheduled {
            return false;
        }
        self.dirty = false;
        self.scheduled = true;
        true
    }
    pub fn after_frame(&mut self) -> bool {
        self.scheduled = false;
        !self.dirty
    }
}
pub fn validate_resource_form(
    name: &str,
    host: &str,
    username: &str,
    port: &str,
) -> Result<u16, &'static str> {
    if name.trim().is_empty() || host.trim().is_empty() || username.trim().is_empty() {
        return Err("invalid_resource");
    }
    port.parse::<u16>()
        .ok()
        .filter(|p| *p > 0)
        .ok_or("invalid_resource")
}
use serde::{Deserialize, Serialize};
pub type ResourceId = u64;
pub type ViewId = u64;
pub type WindowId = u64;
fn window_open_default() -> bool {
    true
}
fn default_top_height() -> f32 {
    180.
}
fn primary_window() -> WindowId {
    1
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceWindow {
    #[serde(default = "window_open_default")]
    pub open: bool,
    #[serde(default)]
    pub bounds: Option<[f32; 4]>,
    #[serde(default = "default_top_height")]
    pub top_height: f32,
    pub id: WindowId,
    pub layout: LayoutNode,
    pub active_resource: Option<ResourceId>,
    pub source: Option<WindowId>,
    #[serde(default)]
    pub follow_paused: bool,
    pub dock_layout: Option<serde_json::Value>,
    pub top_layout: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum AuthMethod {
    #[default]
    Password,
    PrivateKey {
        path: String,
    },
    Agent,
    KeyboardInteractive,
    None,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum TransportPolicy {
    #[default]
    Shared,
    Independent,
    SingleChannel,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectionProfile {
    #[serde(skip)]
    pub ephemeral: bool,
    pub id: u64,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub credential_ref: Option<String>,
    #[serde(default)]
    pub auth: AuthMethod,
    pub jump_resource: Option<ResourceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<crate::ssh::route::RouteSnapshot>,
    #[serde(default)]
    pub transport_policy: TransportPolicy,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Resource {
    pub id: ResourceId,
    pub name: String,
    pub group: String,
    pub tags: Vec<String>,
    pub notes: String,
    pub profiles: Vec<ConnectionProfile>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolKind {
    Terminal,
    Sftp,
    Transfers,
    Services,
    Information,
    Tmux,
    Plugin,
}
impl ToolKind {
    pub fn resource_scoped(self) -> bool {
        matches!(
            self,
            Self::Terminal | Self::Sftp | Self::Information | Self::Tmux | Self::Plugin
        )
    }
    pub const ALL: [Self; 7] = [
        Self::Terminal,
        Self::Sftp,
        Self::Transfers,
        Self::Services,
        Self::Information,
        Self::Tmux,
        Self::Plugin,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Sftp => "sftp",
            Self::Transfers => "transfers",
            Self::Services => "services",
            Self::Information => "information",
            Self::Tmux => "tmux",
            Self::Plugin => "plugin",
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum TargetMode {
    Follow,
    Fixed(ResourceId),
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum RunPolicy {
    Background,
    WhileVisible,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Location {
    Center,
    Left,
    Right,
    Top,
    Bottom,
    Services,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ViewPageState {
    #[serde(default)]
    pub browser_hidden: [bool; 2],
    #[serde(default)]
    pub browser_sort: [crate::sftp::browser::Sort; 2],
    pub resource: Option<ResourceId>,
    pub local_path: Option<String>,
    pub remote_path: Option<String>,
    pub local_scroll: [f32; 2],
    pub remote_scroll: [f32; 2],
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ViewInstance {
    #[serde(default)]
    pub page: ViewPageState,
    #[serde(default = "primary_window")]
    pub window_id: WindowId,
    #[serde(skip)]
    pub ephemeral: bool,
    pub id: ViewId,
    pub tool: ToolKind,
    #[serde(default)]
    pub plugin_enabled: bool,
    pub mode: TargetMode,
    pub run_policy: RunPolicy,
    pub location: Location,
    #[serde(skip)]
    pub session_id: Option<u64>,
    #[serde(skip)]
    pub generation: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum LayoutNode {
    Tabs {
        views: Vec<ViewId>,
        active: Option<ViewId>,
    },
    Split {
        vertical: bool,
        ratio: f32,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}
impl Default for LayoutNode {
    fn default() -> Self {
        Self::Tabs {
            views: vec![],
            active: None,
        }
    }
}
impl LayoutNode {
    fn remove(&mut self, id: ViewId) {
        match self {
            Self::Tabs { views, active } => {
                views.retain(|v| *v != id);
                if *active == Some(id) {
                    *active = views.first().copied();
                }
            }
            Self::Split { first, second, .. } => {
                first.remove(id);
                second.remove(id);
            }
        }
    }
    fn add(&mut self, id: ViewId) {
        match self {
            Self::Tabs { views, active } => {
                views.push(id);
                *active = Some(id);
            }
            Self::Split { first, .. } => first.add(id),
        }
    }
    pub fn focus(&mut self, id: ViewId) {
        match self {
            Self::Tabs { views, active } => {
                if views.contains(&id) {
                    *active = Some(id)
                }
            }
            Self::Split { first, second, .. } => {
                first.focus(id);
                second.focus(id);
            }
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetTicket {
    pub view_id: ViewId,
    pub resource_id: ResourceId,
    pub generation: u64,
}
/// A submitted task owns its destination independently of future focus or profile edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskTarget {
    pub resource_id: ResourceId,
    pub profile: ConnectionProfile,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Workspace {
    #[serde(default)]
    pub windows: Vec<WorkspaceWindow>,
    pub resources: Vec<Resource>,
    pub views: Vec<ViewInstance>,
    pub layout: LayoutNode,
    pub active_resource: Option<ResourceId>,
    pub next_id: u64,
}
impl Default for Workspace {
    fn default() -> Self {
        Self {
            windows: vec![],
            resources: vec![],
            views: vec![],
            layout: LayoutNode::default(),
            active_resource: None,
            next_id: 10,
        }
    }
}
impl Workspace {
    /// Explicit disconnected fixture for QA and tests; never used by startup.
    pub fn demo_fixture() -> Self {
        Self {
            windows: vec![],
            resources: (1..=4)
                .map(|id| Resource {
                    id,
                    name: format!("Demo host {id}"),
                    group: "Demo · disconnected".into(),
                    tags: vec![],
                    notes: "Demonstration resource. No connection has been opened.".into(),
                    profiles: vec![ConnectionProfile {
                        ephemeral: false,
                        id,
                        host: format!("host-{id}.example.invalid"),
                        port: 22,
                        username: "user".into(),
                        credential_ref: None,
                        auth: AuthMethod::default(),
                        jump_resource: None,
                        route: None,
                        transport_policy: Default::default(),
                    }],
                })
                .collect(),
            views: vec![],
            layout: LayoutNode::default(),
            active_resource: Some(1),
            next_id: 10,
        }
    }
}
impl Workspace {
    pub fn window(&self, id: WindowId) -> Option<&WorkspaceWindow> {
        self.windows.iter().find(|w| w.id == id)
    }
    pub fn ensure_windows(&mut self) {
        if self.window(1).is_none() {
            self.windows.push(WorkspaceWindow {
                open: true,
                bounds: None,
                top_height: default_top_height(),
                id: 1,
                layout: self.layout.clone(),
                active_resource: self.active_resource,
                source: None,
                follow_paused: false,
                dock_layout: None,
                top_layout: None,
            });
        }
    }
    pub fn create_window(&mut self, source: Option<WindowId>) -> WindowId {
        self.ensure_windows();
        let id = self.next_id;
        self.next_id += 1;
        let active_resource = source
            .and_then(|id| self.window_target(id))
            .or(self.active_resource);
        self.windows.push(WorkspaceWindow {
            open: true,
            bounds: None,
            top_height: default_top_height(),
            id,
            layout: LayoutNode::default(),
            active_resource,
            source,
            follow_paused: false,
            dock_layout: None,
            top_layout: None,
        });
        id
    }
    pub fn owner(&self, id: ViewId) -> Option<WindowId> {
        self.views.iter().find(|v| v.id == id).map(|v| v.window_id)
    }
    pub fn window_target(&self, mut id: WindowId) -> Option<ResourceId> {
        for _ in 0..=self.windows.len() {
            let Some(w) = self.window(id) else {
                return (id == 1 && self.windows.is_empty())
                    .then_some(self.active_resource)
                    .flatten();
            };
            if w.follow_paused {
                return None;
            }
            match w.source {
                Some(source) => id = source,
                None => {
                    return if id == 1 {
                        self.active_resource
                    } else {
                        w.active_resource
                    };
                }
            }
        }
        None
    }
    fn follow_targets(&self) -> Vec<(ViewId, Option<ResourceId>)> {
        self.views
            .iter()
            .filter(|v| v.tool.resource_scoped() && v.mode == TargetMode::Follow)
            .map(|v| (v.id, self.target(v.id)))
            .collect()
    }
    fn invalidate_changed_targets(&mut self, before: Vec<(ViewId, Option<ResourceId>)>) {
        for (id, target) in before {
            if self.target(id) != target
                && let Some(v) = self.views.iter_mut().find(|v| v.id == id)
            {
                v.generation += 1;
            }
        }
    }
    pub fn set_source(&mut self, window: WindowId, source: WindowId) -> bool {
        if self.window(source).is_none_or(|w| !w.open) {
            return false;
        }
        let mut cursor = Some(source);
        for _ in 0..=self.windows.len() {
            let Some(id) = cursor else {
                break;
            };
            if id == window {
                return false;
            }
            cursor = self.window(id).and_then(|w| w.source);
        }
        let before = self.follow_targets();
        let target = self.window_target(source);
        let Some(w) = self.windows.iter_mut().find(|w| w.id == window) else {
            return false;
        };
        w.source = Some(source);
        w.follow_paused = false;
        w.active_resource = target;
        self.invalidate_changed_targets(before);
        true
    }
    pub fn activate_resource_in(&mut self, window: WindowId, resource: ResourceId) {
        let before = self.follow_targets();
        if window == 1 {
            self.active_resource = Some(resource);
        }
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == window) {
            w.active_resource = Some(resource);
        }
        self.invalidate_changed_targets(before);
    }
    pub fn activate_content_view(&mut self, id: ViewId) -> bool {
        let Some(view) = self.views.iter().find(|v| v.id == id) else {
            return false;
        };
        if !matches!(view.tool, ToolKind::Terminal | ToolKind::Sftp) {
            return false;
        }
        let window = view.window_id;
        let Some(target) = self.target(id) else {
            return false;
        };
        self.activate_resource_in(window, target);
        true
    }
    pub fn open_in(&mut self, window: WindowId, tool: ToolKind, mode: TargetMode) -> ViewId {
        let id = self.open(tool, mode);
        if window != 1 {
            self.layout.remove(id);
            if let Some(w) = self.windows.iter_mut().find(|w| w.id == window) {
                w.layout.add(id);
            }
        }
        self.views.last_mut().unwrap().window_id = window;
        id
    }
    pub fn move_to_window(&mut self, id: ViewId, destination: WindowId, location: Location) {
        let before = self.follow_targets();
        let source = self.owner(id);
        let actual = self.target(id);
        let content = self
            .views
            .iter()
            .any(|v| v.id == id && matches!(v.tool, ToolKind::Terminal | ToolKind::Sftp));
        if content
            && source != Some(destination)
            && let Some(resource) = actual
        {
            if let Some(w) = self.windows.iter_mut().find(|w| w.id == destination) {
                w.source = None;
                w.follow_paused = false;
            }
            self.activate_resource_in(destination, resource);
        }
        self.layout.remove(id);
        for w in &mut self.windows {
            w.layout.remove(id);
        }
        if let Some(v) = self.views.iter_mut().find(|v| v.id == id) {
            v.window_id = destination;
            v.location = location;
        }
        if location == Location::Center {
            if destination == 1 {
                self.layout.add(id);
            } else if let Some(w) = self.windows.iter_mut().find(|w| w.id == destination) {
                w.layout.add(id);
            }
        }
        self.invalidate_changed_targets(before);
    }
    pub fn split_in(&mut self, window: WindowId, id: ViewId, vertical: bool) {
        if window == 1 {
            self.split(id, vertical);
            return;
        }
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == window) {
            w.layout.remove(id);
            let old = std::mem::take(&mut w.layout);
            w.layout = LayoutNode::Split {
                vertical,
                ratio: 0.5,
                first: Box::new(old),
                second: Box::new(LayoutNode::Tabs {
                    views: vec![id],
                    active: Some(id),
                }),
            };
        }
    }
    pub fn close_window(&mut self, id: WindowId) {
        let before = self.follow_targets();
        let last = self.window_target(id);
        let mut closed = std::collections::HashSet::from([id]);
        loop {
            let previous = closed.len();
            for w in &mut self.windows {
                if w.source.is_some_and(|source| closed.contains(&source)) {
                    w.follow_paused = true;
                    w.active_resource = last;
                    closed.insert(w.id);
                }
            }
            if previous == closed.len() {
                break;
            }
        }
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.open = false;
            w.active_resource = last;
        }
        self.invalidate_changed_targets(before);
    }
    pub fn unmounted_views(
        &self,
        mounted: &std::collections::HashMap<ViewId, Location>,
    ) -> Vec<ViewInstance> {
        self.views
            .iter()
            .filter(|v| v.location != Location::Services && !mounted.contains_key(&v.id))
            .cloned()
            .collect()
    }
    /// A missing saved view is recoverable. Only absence after a known live mount means closure.
    pub fn reconcile_view_locations(
        &mut self,
        mounted: &std::collections::HashMap<ViewId, Location>,
        previously_mounted: &std::collections::HashSet<ViewId>,
    ) {
        self.views.retain_mut(|v| {
            if let Some(location) = mounted.get(&v.id) {
                v.location = *location;
                true
            } else {
                v.location == Location::Services || !previously_mounted.contains(&v.id)
            }
        });
    }
    pub fn open(&mut self, tool: ToolKind, mode: TargetMode) -> ViewId {
        let id = self.next_id;
        self.next_id += 1;
        self.views.push(ViewInstance {
            page: ViewPageState::default(),
            window_id: 1,
            ephemeral: false,
            id,
            tool,
            plugin_enabled: false,
            mode,
            run_policy: RunPolicy::WhileVisible,
            location: Location::Center,
            session_id: None,
            generation: 0,
        });
        self.layout.add(id);
        id
    }
    pub fn target(&self, id: ViewId) -> Option<ResourceId> {
        let v = self.views.iter().find(|v| v.id == id)?;
        if !v.tool.resource_scoped() {
            return None;
        }
        let target = match v.mode {
            TargetMode::Follow => self.window_target(v.window_id),
            TargetMode::Fixed(id) => Some(id),
        };
        target.filter(|id| self.resources.iter().any(|r| r.id == *id))
    }
    pub fn capture_task_target(&self, id: ViewId) -> Option<TaskTarget> {
        let resource_id = self.target(id)?;
        let r = self.resources.iter().find(|r| r.id == resource_id)?;
        let mut profile = r.profiles.first()?.clone();
        profile.capture_route(&self.resources);
        Some(TaskTarget {
            resource_id,
            profile,
        })
    }
    /// Visibility changes affect collection only; session/task ownership lives elsewhere.
    pub fn should_refresh(&self, id: ViewId, visible: bool, minimized: bool) -> bool {
        self.target(id).is_some()
            && self
                .views
                .iter()
                .find(|v| v.id == id)
                .is_some_and(|v| v.run_policy == RunPolicy::Background || (visible && !minimized))
    }
    pub fn ticket(&self, id: ViewId) -> Option<TargetTicket> {
        Some(TargetTicket {
            view_id: id,
            resource_id: self.target(id)?,
            generation: self.views.iter().find(|v| v.id == id)?.generation,
        })
    }
    pub fn accepts(&self, t: TargetTicket) -> bool {
        self.ticket(t.view_id) == Some(t)
    }
    pub fn activate_resource(&mut self, id: ResourceId) {
        self.activate_resource_in(1, id);
    }
    pub fn move_view(&mut self, id: ViewId, location: Location) {
        if let Some(owner) = self.owner(id) {
            self.move_to_window(id, owner, location);
        }
    }
    pub fn close(&mut self, id: ViewId) {
        self.layout.remove(id);
        for window in &mut self.windows {
            window.layout.remove(id);
        }
        self.views.retain(|v| v.id != id);
    }
    pub fn split(&mut self, id: ViewId, vertical: bool) {
        self.move_view(id, Location::Center);
        self.layout.remove(id);
        let old = std::mem::take(&mut self.layout);
        self.layout = LayoutNode::Split {
            vertical,
            ratio: 0.5,
            first: Box::new(old),
            second: Box::new(LayoutNode::Tabs {
                views: vec![id],
                active: Some(id),
            }),
        };
    }
    pub fn set_mode(&mut self, id: ViewId, mode: TargetMode) {
        if let Some(v) = self.views.iter_mut().find(|v| v.id == id) {
            v.mode = mode;
            v.generation += 1;
        }
    }
    pub fn add_resource(&mut self) -> ResourceId {
        let id = self.next_id;
        self.next_id += 1;
        self.resources.push(Resource {
            id,
            name: format!("New host {id}"),
            group: "Resources".into(),
            tags: vec![],
            notes: String::new(),
            profiles: vec![ConnectionProfile {
                ephemeral: false,
                id,
                host: String::new(),
                port: 22,
                username: String::new(),
                credential_ref: None,
                auth: AuthMethod::default(),
                jump_resource: None,
                route: None,
                transport_policy: Default::default(),
            }],
        });
        self.active_resource = Some(id);
        id
    }
}

#[cfg(test)]
mod advanced_route_tests {
    use super::*;
    #[test]
    fn captured_target_contains_immutable_jump_profiles() {
        let mut w = Workspace::demo_fixture();
        let destination = w.resources[0].id;
        let jump = w.add_resource();
        w.resources.last_mut().unwrap().profiles[0].host = "jump-a".into();
        w.resources[0].profiles[0].jump_resource = Some(jump);
        w.active_resource = Some(destination);
        let view = w.open(ToolKind::Terminal, TargetMode::Follow);
        let captured = w.capture_task_target(view).unwrap();
        w.resources.last_mut().unwrap().profiles[0].host = "mutated".into();
        let snapshot = serde_json::to_string(&captured.profile).unwrap();
        assert!(
            snapshot.contains("jump-a"),
            "captured task lost its jump endpoint: {snapshot}"
        );
        assert!(!snapshot.contains("mutated"));
    }
}

#[cfg(test)]
mod window_tests {
    use super::*;
    #[test]
    fn independent_context_and_detached_source_pause() {
        let mut w = Workspace::demo_fixture();
        let other = w.create_window(None);
        let detached = w.create_window(Some(1));
        let a = w.open_in(1, ToolKind::Information, TargetMode::Follow);
        let b = w.open_in(other, ToolKind::Information, TargetMode::Follow);
        let c = w.open_in(detached, ToolKind::Information, TargetMode::Follow);
        w.activate_resource_in(1, 2);
        w.activate_resource_in(other, 3);
        assert_eq!(
            (w.target(a), w.target(b), w.target(c)),
            (Some(2), Some(3), Some(2))
        );
        w.close_window(1);
        assert_eq!(w.target(c), None);
        assert!(w.window(detached).unwrap().follow_paused);
        assert_eq!(w.target(b), Some(3));
    }
    #[test]
    fn moving_content_carries_actual_target_without_restarting_identity() {
        let mut w = Workspace::demo_fixture();
        let other = w.create_window(None);
        let a = w.open_in(1, ToolKind::Sftp, TargetMode::Follow);
        w.views.iter_mut().find(|v| v.id == a).unwrap().session_id = Some(72);
        let ticket = w.ticket(a);
        w.activate_resource_in(other, 3);
        w.move_to_window(a, other, Location::Center);
        assert_eq!(w.target(a), Some(1));
        assert_eq!(w.ticket(a), ticket);
        assert_eq!(
            w.views.iter().find(|v| v.id == a).unwrap().session_id,
            Some(72)
        );
        assert_eq!(w.owner(a), Some(other));
        let b = w.open_in(1, ToolKind::Information, TargetMode::Fixed(2));
        w.move_to_window(b, other, Location::Right);
        assert_eq!(
            w.window_target(other),
            Some(1),
            "auxiliary focus cannot become resource context"
        );
    }
    #[test]
    fn window_layout_and_global_ids_roundtrip() {
        let mut w = Workspace::demo_fixture();
        let other = w.create_window(None);
        let a = w.open_in(other, ToolKind::Sftp, TargetMode::Fixed(1));
        let b = w.open_in(other, ToolKind::Sftp, TargetMode::Fixed(2));
        w.split_in(other, b, true);
        assert!(matches!(
            w.window(other).unwrap().layout,
            LayoutNode::Split { vertical: true, .. }
        ));
        assert_ne!(a, b);
        let restored: Workspace =
            serde_json::from_str(&serde_json::to_string(&w).unwrap()).unwrap();
        assert_eq!(restored, w);
        assert_eq!(restored.owner(a), Some(other));
    }
}

#[cfg(test)]
mod follow_chain_tests {
    use super::*;
    #[test]
    fn source_chain_pauses_transitively_and_cannot_cycle() {
        let mut workspace = Workspace::demo_fixture();
        let child = workspace.create_window(Some(1));
        let grandchild = workspace.create_window(Some(child));
        let view = workspace.open_in(grandchild, ToolKind::Information, TargetMode::Follow);
        workspace.activate_resource(3);
        assert_eq!(workspace.target(view), Some(3));
        assert!(!workspace.set_source(1, grandchild));
        workspace.close_window(1);
        assert_eq!(workspace.target(view), None);
        assert!(workspace.window(grandchild).unwrap().follow_paused);
        assert_eq!(
            workspace.window(grandchild).unwrap().active_resource,
            Some(3)
        );
    }
}

#[cfg(test)]
mod content_focus_tests {
    use super::*;
    #[test]
    fn auxiliary_activation_cannot_publish_context_and_source_roundtrip_rejects_old_ticket() {
        let mut w = Workspace::demo_fixture();
        let other = w.create_window(None);
        let detached = w.create_window(Some(1));
        let nested = w.create_window(Some(detached));
        let follower = w.open_in(nested, ToolKind::Information, TargetMode::Follow);
        let fixed = w.open_in(other, ToolKind::Sftp, TargetMode::Fixed(4));
        let fixed_ticket = w.ticket(fixed);
        for tool in [ToolKind::Tmux, ToolKind::Plugin, ToolKind::Information] {
            let aux = w.open_in(1, tool, TargetMode::Fixed(2));
            assert!(!w.activate_content_view(aux));
            assert_eq!(w.target(follower), Some(1));
        }
        let old = w.ticket(follower).unwrap();
        let content = w.open_in(1, ToolKind::Sftp, TargetMode::Fixed(2));
        assert!(w.activate_content_view(content));
        assert_eq!(w.target(follower), Some(2));
        w.activate_resource_in(1, 1);
        assert_eq!(w.target(follower), Some(1));
        assert!(!w.accepts(old));
        assert_eq!(w.ticket(fixed), fixed_ticket);
    }
}

#[cfg(test)]
mod background_window_tests {
    use super::*;
    #[test]
    fn closing_keeps_background_target_and_hiding_does_not_publish_context() {
        let mut w = Workspace::demo_fixture();
        let other = w.create_window(None);
        let following = w.open_in(1, ToolKind::Sftp, TargetMode::Follow);
        let fixed = w.open_in(1, ToolKind::Sftp, TargetMode::Fixed(3));
        w.move_view(fixed, Location::Services);
        assert_eq!(
            w.target(following),
            Some(1),
            "closing a fixed presentation must not activate it"
        );
        w.close_window(1);
        assert_eq!(
            w.target(following),
            Some(1),
            "background work retains the last actual target"
        );
        w.activate_resource_in(other, 4);
        w.move_to_window(following, other, Location::Center);
        assert_eq!(w.target(following), Some(1));
    }
}
