//! Immutable jump endpoints captured before submitting any network operation.
use crate::workspace::{ConnectionProfile, Resource, ResourceId};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouteSnapshot {
    pub hops: Vec<ConnectionProfile>,
    pub error: Option<String>,
}
impl ConnectionProfile {
    pub fn capture_route(&mut self, resources: &[Resource]) {
        let mut hops = Vec::new();
        let mut visited = Vec::<ResourceId>::new();
        let mut next = self.jump_resource;
        let mut error = None;
        while let Some(id) = next {
            if visited.contains(&id) {
                error = Some("Jump route contains a cycle".into());
                break;
            }
            if visited.len() >= 8 {
                error = Some("Jump route exceeds eight hops".into());
                break;
            }
            visited.push(id);
            let Some(mut profile) = resources
                .iter()
                .find(|r| r.id == id)
                .and_then(|r| r.profiles.first())
                .cloned()
            else {
                error = Some(format!("Jump resource #{id} is missing or has no profile"));
                break;
            };
            profile.route = None;
            next = profile.jump_resource;
            hops.push(profile);
        }
        hops.reverse();
        self.route = if self.jump_resource.is_some() {
            Some(RouteSnapshot { hops, error })
        } else {
            None
        };
    }
    pub fn contains_temporary_endpoint(&self) -> bool {
        self.ephemeral
            || self
                .route
                .as_ref()
                .is_some_and(|route| route.hops.iter().any(Self::contains_temporary_endpoint))
    }
    pub fn route_profiles(&self) -> Result<Vec<ConnectionProfile>, String> {
        let mut hops = match &self.route {
            Some(route) => {
                if let Some(error) = &route.error {
                    return Err(error.clone());
                }
                if route.hops.len() > 8 || route.hops.iter().any(|hop| hop.route.is_some()) {
                    return Err("Invalid captured jump route".into());
                }
                route.hops.clone()
            }
            None if self.jump_resource.is_some() => {
                return Err("Jump route has not been captured".into());
            }
            None => Vec::new(),
        };
        let mut destination = self.clone();
        destination.route = None;
        hops.push(destination);
        Ok(hops)
    }
}

#[cfg(test)]
mod tests {
    use crate::workspace::Workspace;
    #[test]
    fn invalid_routes_are_explicit_and_saved_settings_strip_runtime_snapshots() {
        let mut workspace = Workspace::default();
        let mut p = workspace.resources[0].profiles[0].clone();
        p.jump_resource = Some(999);
        p.capture_route(&workspace.resources);
        assert!(p.route_profiles().unwrap_err().contains("missing"));
        workspace.resources[0].profiles[0].jump_resource = Some(workspace.resources[0].id);
        p = workspace.resources[0].profiles[0].clone();
        p.capture_route(&workspace.resources);
        assert!(p.route_profiles().unwrap_err().contains("cycle"));
        workspace.resources[0].profiles[0].jump_resource = None;
        let mut previous = None;
        for _ in 0..9 {
            let id = workspace.add_resource();
            workspace.resources.last_mut().unwrap().profiles[0].jump_resource = previous;
            previous = Some(id);
        }
        p.jump_resource = previous;
        p.capture_route(&workspace.resources);
        assert!(p.route_profiles().unwrap_err().contains("eight hops"));
        p.jump_resource = Some(workspace.resources[0].id);
        workspace.resources[0].profiles[0].ephemeral = true;
        p.capture_route(&workspace.resources);
        assert!(p.contains_temporary_endpoint());
        let mut settings = crate::settings::Settings::default();
        settings.workspace.resources[0].profiles[0].route = p.route.clone();
        assert!(
            settings.persistent_snapshot().workspace.resources[0].profiles[0]
                .route
                .is_none()
        );
        assert!(
            settings.workspace.resources[0].profiles[0].route.is_some(),
            "saving mutated live task snapshot"
        );
    }
}
