//! Shared tool ownership boundary. Native adapters keep their existing session services;
//! scripts only receive a deliberately small, captured resource description.
use crate::workspace::{ResourceId, TaskTarget, ToolKind, ViewId, Workspace};
use serde::Serialize;
use std::{cell::Cell, rc::Rc};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResourceCapabilities {
    pub metadata: bool,
}
impl ResourceCapabilities {
    pub const fn metadata() -> Self {
        Self { metadata: true }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ResourceContext {
    pub id: ResourceId,
    pub name: String,
    pub host: String,
    pub port: u16,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ToolContext {
    pub view_id: ViewId,
    pub generation: u64,
    pub resource: Option<ResourceContext>,
}
#[derive(Clone, Debug)]
pub struct ToolProvider {
    pub id: String,
    pub kind: Option<ToolKind>,
    capabilities: ResourceCapabilities,
}
impl ToolProvider {
    pub fn builtin(kind: ToolKind) -> Self {
        Self {
            id: format!("helixlane.{}", kind.key()),
            kind: Some(kind),
            capabilities: ResourceCapabilities::metadata(),
        }
    }
    pub fn plugin(id: impl Into<String>, capabilities: ResourceCapabilities) -> Self {
        Self {
            id: id.into(),
            kind: None,
            capabilities,
        }
    }
    pub fn instantiate(
        &self,
        id: ViewId,
        workspace: &Workspace,
    ) -> Result<ToolInstance, &'static str> {
        let view = workspace
            .views
            .iter()
            .find(|view| view.id == id)
            .ok_or("View is closed")?;
        if self.kind.is_some_and(|kind| kind != view.tool) {
            return Err("Provider does not own this tool");
        }
        if self.kind.is_none() && view.ephemeral {
            return Err("Transient launch views are unavailable to plugins");
        }
        let resource = workspace
            .target(id)
            .and_then(|id| {
                workspace
                    .resources
                    .iter()
                    .find(|resource| resource.id == id)
            })
            // CLI-only profiles are never exposed to script, even as metadata.
            .filter(|resource| {
                !view.ephemeral && !resource.profiles.iter().any(|profile| profile.ephemeral)
            })
            .and_then(|resource| {
                resource.profiles.first().map(|profile| ResourceContext {
                    id: resource.id,
                    name: resource.name.clone(),
                    host: profile.host.clone(),
                    port: profile.port,
                })
            });
        Ok(ToolInstance {
            native_target: self.kind.and_then(|_| workspace.capture_task_target(id)),
            context: ToolContext {
                view_id: id,
                generation: view.generation,
                resource,
            },
            capabilities: self.capabilities,
            lease: ToolLease(Rc::new(Cell::new(true))),
            collection: Rc::new(Cell::new(true)),
            collection_context: None,
            cleanup: Vec::new(),
        })
    }
}
#[derive(Clone)]
pub struct ToolLease(Rc<Cell<bool>>);
impl ToolLease {
    pub fn is_active(&self) -> bool {
        self.0.get()
    }
}
pub type CollectionContext = Rc<dyn Fn() -> Result<ToolContext, &'static str>>;

pub struct ToolInstance {
    // Only trusted builtin providers receive connection profiles. Never serialize
    // ToolInstance or expose this field from a script HostModule.
    native_target: Option<TaskTarget>,
    context: ToolContext,
    capabilities: ResourceCapabilities,
    lease: ToolLease,
    collection: Rc<Cell<bool>>,
    collection_context: Option<CollectionContext>,
    cleanup: Vec<Box<dyn FnOnce()>>,
}
impl ToolInstance {
    pub fn native_target(&self) -> Option<&TaskTarget> {
        self.native_target.as_ref()
    }
    pub fn context(&self) -> &ToolContext {
        &self.context
    }
    pub fn set_collection_active(&self, active: bool) {
        self.collection.set(active);
    }
    pub fn collection_activity(&self) -> Rc<Cell<bool>> {
        self.collection.clone()
    }
    /// Resolve only future collection calls. Returned contexts remain immutable snapshots,
    /// and rebinding does not revoke the separate lease held by submitted user jobs.
    pub fn bind_collection_context(
        &mut self,
        source: impl Fn() -> Result<ToolContext, &'static str> + 'static,
    ) {
        self.collection_context = Some(Rc::new(source));
    }
    pub fn collection_context(&self) -> CollectionContext {
        let permitted = self.capabilities.metadata;
        let context = self.context.clone();
        let source = self.collection_context.clone();
        Rc::new(move || {
            if !permitted {
                return Err("Resource metadata permission denied");
            }
            match &source {
                Some(source) => source(),
                None => Ok(context.clone()),
            }
        })
    }
    pub fn resource_context(&self) -> Result<ToolContext, &'static str> {
        if !self.lease.is_active() {
            return Err("Plugin is unloaded");
        }
        if !self.collection.get() {
            return Err("Collection paused");
        }
        if !self.capabilities.metadata {
            return Err("Resource metadata permission denied");
        }
        (self.collection_context())()
    }
    pub fn lease(&self) -> ToolLease {
        self.lease.clone()
    }
    /// Own subscriptions, cancellation handles, or host task aborts here.
    pub fn on_cleanup(&mut self, cleanup: impl FnOnce() + 'static) {
        self.cleanup.push(Box::new(cleanup));
    }
    pub fn unload(&mut self) {
        self.lease.0.set(false);
        for cleanup in self.cleanup.drain(..) {
            cleanup();
        }
    }
}
impl Drop for ToolInstance {
    fn drop(&mut self) {
        self.unload();
    }
}
#[cfg(test)]
mod tests;

#[cfg(feature = "desktop")]
pub mod shell;
