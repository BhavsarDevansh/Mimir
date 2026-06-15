//! Shared conversation-turn types for agent tasks.

use chrono::{DateTime, Utc};

/// A single completed exchange between the user and the assistant.
///
/// Used by background agents (e.g. the Librarian) that need the full
/// conversational context rather than just the raw user string.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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
}
