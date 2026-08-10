//! iMIP invite extraction from a MIME message body.

use mail_parser::MimeHeaders;
use tracing::debug;

use crate::email::connector::EmailConnector;

/// Layer 1 of the extraction cascade (C6 / #200): iMIP calendar invites.
///
/// Walks every MIME part of the message for `text/calendar` parts whose
/// iMIP `METHOD` is `REQUEST` (a meeting request) or `REPLY` (an attendee
/// response), parses each embedded VEVENT via the shared
/// [`crate::ical::parse_ical_to_vevents`], and turns it into the appointment
/// fact cluster via [`crate::ical::vevent_to_facts`]. The full `parts` walk
/// (not only `attachments()`) catches a `text/calendar` part nested inside
/// `multipart/alternative` that carries no `Content-Disposition: attachment`
/// header and is therefore classified as a body part by `mail-parser`.
/// `PUBLISH` (often marketing webinars) and `CANCEL` (deletion lifecycle)
/// are skipped for now — `CANCEL` → KB fact lifecycle is tracked
/// separately. Every fact is provenanced with `raw_ref` (the email's
/// `UIDVALIDITY`-qualified IMAP UID) and authored against the injected
/// [`user_identity`](Self::user_identity) when set.
impl EmailConnector {
    pub(super) fn extract_invites(
        &self,
        message: &mail_parser::Message<'_>,
        raw_ref: &str,
    ) -> Vec<mimir_knowledge::normalize::NormalizedFact> {
        let mut facts = Vec::new();
        for part in &message.parts {
            if !part.is_content_type("text", "calendar") {
                continue;
            }
            let Some(ct) = part.content_type() else {
                continue;
            };
            let Some(ical) = part.text_contents() else {
                debug!(
                    raw_ref,
                    "text/calendar part had no decodable text contents; skipping"
                );
                continue;
            };
            // RFC 6047 §2.4: the MIME `method` parameter is optional. Prefer it
            // when present (it must agree with the body), otherwise fall back to
            // the iCalendar `METHOD` property in the body. Property names are
            // case-insensitive (RFC 5545).
            // If both are present and disagree, the part is rejected — a
            // conflicting `METHOD` is not a valid iMIP object (RFC 6047 §2.4
            // requires the parameter and body to match when both are supplied).
            let mime_method = ct
                .attribute("method")
                .map(|m| m.trim().to_ascii_uppercase());
            let calendar_method = ical
                .lines()
                .find_map(|l| {
                    let l = l.trim();
                    l.get(..7)
                        .filter(|p| p.eq_ignore_ascii_case("METHOD:"))
                        .map(|_| l[7..].trim())
                })
                .map(str::to_ascii_uppercase);
            let method = match (mime_method, calendar_method) {
                (Some(mime), Some(calendar)) if mime != calendar => {
                    debug!(
                        raw_ref,
                        mime_method = %mime,
                        calendar_method = %calendar,
                        "skipping text/calendar part: conflicting METHOD values"
                    );
                    continue;
                }
                (Some(mime), _) => Some(mime),
                (None, Some(calendar)) => Some(calendar),
                (None, None) => None,
            };
            match method.as_deref() {
                Some("REQUEST") | Some("REPLY") => {}
                other => {
                    debug!(raw_ref, method = ?other, "skipping text/calendar part: unsupported/absent METHOD");
                    continue;
                }
            }
            for vevent in crate::ical::parse_ical_to_vevents(ical) {
                facts.extend(crate::ical::vevent_to_facts(
                    self.user_identity.as_deref(),
                    &vevent,
                    raw_ref,
                ));
            }
        }
        facts
    }
}
