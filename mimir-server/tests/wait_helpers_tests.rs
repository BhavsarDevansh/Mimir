mod common;

use common::*;

#[tokio::test]
async fn poll_until_returns_immediately_when_condition_is_met() {
    let result = poll_until(Duration::from_millis(50), || async { true }).await;
    assert!(result);
}

#[tokio::test]
async fn poll_until_expires_when_condition_is_never_met() {
    let result = poll_until(Duration::from_millis(10), || async { false }).await;
    assert!(!result);
}
