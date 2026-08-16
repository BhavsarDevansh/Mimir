//! Unit tests for shared secrets-module helpers.

use super::*;

#[test]
fn mismatch_error_pins_the_exact_message_text() {
    // Both connectors surface this message (issue #273); the text is pinned
    // here so a future reword cannot drift one backend from the other.
    assert_eq!(
        mismatch_error("app_password").to_string(),
        "authentication failed: auth method app_password does not match stored secret kind",
    );
    assert_eq!(
        mismatch_error("oauth").to_string(),
        "authentication failed: auth method oauth does not match stored secret kind",
    );
}
