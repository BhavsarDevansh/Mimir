//! Shared conversation-turn types for agent tasks.

use chrono::{DateTime, Utc};
use std::hash::{Hash, Hasher};

/// A single completed exchange between the user and the assistant.
///
/// Used by background agents (e.g. the Librarian) that need the full
/// conversational context rather than just the raw user string.
#[derive(Debug, Clone)]
pub struct ConversationTurn {
    pub user_message: String,
    pub assistant_response: String,
    pub session_id: i64,
    pub timestamp: DateTime<Utc>,
}

impl ConversationTurn {
    /// Create a turn from the user and assistant messages.
    pub fn new(
        session_id: i64,
        user_message: impl Into<String>,
        assistant_response: impl Into<String>,
    ) -> Self {
        Self {
            user_message: user_message.into(),
            assistant_response: assistant_response.into(),
            session_id,
            timestamp: Utc::now(),
        }
    }
}

impl PartialEq for ConversationTurn {
    fn eq(&self, other: &Self) -> bool {
        self.user_message == other.user_message
            && self.assistant_response == other.assistant_response
            && self.session_id == other.session_id
    }
}

impl Eq for ConversationTurn {}

impl Hash for ConversationTurn {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.user_message.hash(state);
        self.assistant_response.hash(state);
        self.session_id.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_turn_stores_messages() {
        let turn = ConversationTurn::new(42, "hello", "hi there");
        assert_eq!(turn.user_message, "hello");
        assert_eq!(turn.assistant_response, "hi there");
        assert_eq!(turn.session_id, 42);
    }

    #[test]
    fn conversation_turns_with_same_content_are_equal_and_hash_equal() {
        let a = ConversationTurn::new(42, "hello", "hi there");
        // Sleep briefly so the timestamps differ.
        std::thread::sleep(std::time::Duration::from_millis(5));
        let b = ConversationTurn::new(42, "hello", "hi there");

        assert_eq!(a, b);

        let mut hasher_a = std::collections::hash_map::DefaultHasher::new();
        a.hash(&mut hasher_a);
        let mut hasher_b = std::collections::hash_map::DefaultHasher::new();
        b.hash(&mut hasher_b);
        assert_eq!(hasher_a.finish(), hasher_b.finish());
    }
}
