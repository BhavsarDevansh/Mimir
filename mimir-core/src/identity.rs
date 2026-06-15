//! User identity types for agents that need name + entity resolution.

/// Resolved user identity from configuration.
///
/// `entity_id` is the row id in the knowledge graph for the configured user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UserIdentity {
    pub name: &'static str,
    pub entity_id: i32,
}

impl UserIdentity {
    /// Create a new identity.
    pub fn new(name: &'static str, entity_id: i32) -> Self {
        Self { name, entity_id }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_holds_name_and_entity_id() {
        let id = UserIdentity::new("Devansh", 7);
        assert_eq!(id.name, "Devansh");
        assert_eq!(id.entity_id, 7);
    }
}
