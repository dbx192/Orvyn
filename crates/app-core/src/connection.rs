use db_common::{ConnectionId, ConnectionProfile, Result};

/// In-memory connection registry used by the early skeleton.
#[derive(Debug, Default)]
pub struct ConnectionRegistry {
    profiles: Vec<ConnectionProfile>,
}

impl ConnectionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, profile: ConnectionProfile) {
        self.profiles.push(profile);
    }

    pub fn list(&self) -> &[ConnectionProfile] {
        &self.profiles
    }

    pub fn find(&self, id: ConnectionId) -> Option<&ConnectionProfile> {
        self.profiles.iter().find(|profile| profile.id == id)
    }

    pub fn validate(profile: &ConnectionProfile) -> Result<()> {
        profile.validate()
    }
}
