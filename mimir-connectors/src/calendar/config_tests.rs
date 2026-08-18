//! Unit tests for Calendar config DTOs.

use super::CalendarAuthMethod;
use crate::secrets::AuthMethodDiscriminant;

#[test]
fn auth_method_discriminants_match_serde_kind_tag() {
    // The shared trait contract (issue #341): every variant's
    // `discriminant()` must equal the serde `kind` tag so the mismatch error
    // can never drift from the stored-config kind.
    let app_password = CalendarAuthMethod::AppPassword {
        username: "devansh@example.com".into(),
    };
    let oauth = CalendarAuthMethod::OAuth {
        auth_uri: None,
        token_endpoint: "https://oauth.example.com/token".into(),
        client_id: "cid".into(),
        client_secret: None,
        scopes: None,
    };
    for (auth, kind) in [(&app_password, "app_password"), (&oauth, "oauth")] {
        assert_eq!(AuthMethodDiscriminant::discriminant(auth), kind);
        assert_eq!(serde_json::to_value(auth).unwrap()["kind"], kind);
    }
}
