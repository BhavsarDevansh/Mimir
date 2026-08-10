use super::warn_err;

#[test]
fn warn_err_returns_some_on_ok() {
    assert_eq!(
        warn_err::<i32, std::io::Error>(Ok(42), "expected success"),
        Some(42)
    );
}

#[test]
fn warn_err_returns_none_on_err() {
    let err = std::io::Error::other("boom");
    assert!(warn_err::<i32, std::io::Error>(Err(err), "expected failure").is_none());
}
