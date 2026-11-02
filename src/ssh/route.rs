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
