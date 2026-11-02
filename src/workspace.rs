#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_top_restore_is_recovered_without_being_mistaken_for_close() {
        let mut w = Workspace::default();
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
        let mut w = Workspace::default();
        let id = w.open(ToolKind::Services, TargetMode::Follow);
        w.activate_resource(2);
        assert_eq!(w.target(id), None);
        assert_eq!(w.views[0].generation, 0);
    }
    #[test]
    fn moving_keeps_identity_and_session() {
        let mut w = Workspace::default();
        let id = w.open(ToolKind::Sftp, TargetMode::Fixed(1));
        w.views[0].session_id = Some(9);
        w.move_view(id, Location::Right);
        assert_eq!(w.views[0].id, id);
        assert_eq!(w.views[0].session_id, Some(9));
    }
    #[test]
    fn fixed_ignores_follow_and_late_results_are_rejected() {
        let mut w = Workspace::default();
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
        let mut w = Workspace::default();
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
        let mut w = Workspace::default();
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
        let mut w = Workspace::default();
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
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum AuthMethod {
    #[default]
    Password,
    PrivateKey {
        path: String,
    },
    None,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectionProfile {
    pub id: u64,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub credential_ref: Option<String>,
    #[serde(default)]
    pub auth: AuthMethod,
    pub jump_resource: Option<ResourceId>,
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
}
impl ToolKind {
    pub fn resource_scoped(self) -> bool {
        matches!(
            self,
            Self::Terminal | Self::Sftp | Self::Information | Self::Tmux
        )
    }
    pub const ALL: [Self; 6] = [
        Self::Terminal,
        Self::Sftp,
        Self::Transfers,
        Self::Services,
        Self::Information,
        Self::Tmux,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Sftp => "sftp",
            Self::Transfers => "transfers",
            Self::Services => "services",
            Self::Information => "information",
            Self::Tmux => "tmux",
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
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ViewInstance {
    pub id: ViewId,
    pub tool: ToolKind,
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
    pub resources: Vec<Resource>,
    pub views: Vec<ViewInstance>,
    pub layout: LayoutNode,
    pub active_resource: Option<ResourceId>,
    pub next_id: u64,
}
impl Default for Workspace {
    fn default() -> Self {
        Self {
            resources: (1..=4)
                .map(|id| Resource {
                    id,
                    name: format!("Demo host {id}"),
                    group: "Demo · disconnected".into(),
                    tags: vec![],
                    notes: "Demonstration resource. No connection has been opened.".into(),
                    profiles: vec![ConnectionProfile {
                        id,
                        host: format!("host-{id}.example.invalid"),
                        port: 22,
                        username: "user".into(),
                        credential_ref: None,
                        auth: AuthMethod::default(),
                        jump_resource: None,
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
            id,
            tool,
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
            TargetMode::Follow => self.active_resource,
            TargetMode::Fixed(id) => Some(id),
        };
        target.filter(|id| self.resources.iter().any(|r| r.id == *id))
    }
    pub fn capture_task_target(&self, id: ViewId) -> Option<TaskTarget> {
        let resource_id = self.target(id)?;
        let r = self.resources.iter().find(|r| r.id == resource_id)?;
        Some(TaskTarget {
            resource_id,
            profile: r.profiles.first()?.clone(),
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
        if self.active_resource != Some(id) {
            self.active_resource = Some(id);
            for v in &mut self.views {
                if v.tool.resource_scoped() && v.mode == TargetMode::Follow {
                    v.generation += 1;
                }
            }
        }
    }
    pub fn move_view(&mut self, id: ViewId, location: Location) {
        self.layout.remove(id);
        if let Some(v) = self.views.iter_mut().find(|v| v.id == id) {
            v.location = location;
            if location == Location::Center {
                self.layout.add(id);
            }
        }
    }
    pub fn close(&mut self, id: ViewId) {
        self.layout.remove(id);
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
                id,
                host: String::new(),
                port: 22,
                username: String::new(),
                credential_ref: None,
                auth: AuthMethod::default(),
                jump_resource: None,
            }],
        });
        self.active_resource = Some(id);
        id
    }
}
