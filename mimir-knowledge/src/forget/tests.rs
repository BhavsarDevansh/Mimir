//! Forget pipeline tests.

use super::*;

#[test]
fn forget_filters_full_reset() {
    let mut f = ForgetFilters::default();
    assert!(!f.is_full_reset());
    f.all = true;
    assert!(f.is_full_reset());
}
