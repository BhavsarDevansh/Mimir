//! iMIP invite extraction from a MIME message body.

use mail_parser::MimeHeaders;
use tracing::debug;

use crate::email::connector::EmailConnector;

/// Layer 1 of the extraction cascade (C6 / #200): iMIP calendar invites.
///
/// Walks every MIME part of the message for `text/calendar` parts whose
/// iMIP `METHOD` is `REQUEST` (a meeting request), `REPLY` (an attendee
/// response), or `CANCEL` (a deletion lifecycle signal), parses each
/// embedded VEVENT via the shared [`crate::ical::parse_ical_to_vevents`],
/// and turns it into the appointment fact cluster via
/// [`crate::ical::vevent_to_facts`]. The full `parts` walk (not only
/// `attachments()`) catches a `text/calendar` part nested inside
/// `multipart/alternative` that carries no `Content-Disposition: attachment`
/// header and is therefore classified as a body part by `mail-parser`.
/// `PUBLISH` (often marketing webinars) is skipped.
///
/// **Fact keying (issue #283).** REQUEST/REPLY facts are provenanced with
/// the VEVENT `UID` namespaced as `imip:{uid}` as `raw_reference` — the
/// stable iMIP identity RFC 5546 requires every method (REQUEST → REPLY →
/// CANCEL) to share — so a CANCEL maps 1:1 onto the facts the original
/// invite authored. The `imip:` prefix keeps the sender-controlled iMIP
/// identity space disjoint from the `{uid_validity}:{uid}` references the
/// JSON-LD and LLM cascade layers write, so a crafted `UID` can never
/// address another layer's facts. A VEVENT without a `UID` (invalid per
/// RFC 5545, tolerated by the lenient parser) falls back to the email's
/// `UIDVALIDITY`-qualified IMAP UID (`raw_ref`) in its own `imip-email:`
/// namespace. The JSON-LD and LLM cascade layers keep the email UID — they
/// have no CANCEL lifecycle.
///
/// **CANCEL handling.** A `CANCEL` part emits no facts; each cancelled
/// VEVENT's namespaced reference (`imip:{uid}`) is buffered as a tombstone
/// (reported via
/// [`Connector::extract_deletions`](crate::connector::Connector::extract_deletions)
/// and trashed through the shared #247 machinery), so a cancelled meeting
/// stops surfacing in "Upcoming". A CANCEL VEVENT without a `UID` cannot be
/// mapped and is skipped. `SEQUENCE` is not consulted in V1: the knowledge
/// graph does not store the original sequence, so a CANCEL trashes by `UID`
/// regardless of sequence. The tombstone buffer is part of the connector's
/// durable state, so a restart between `extract` and the supervisor's
/// deletion pass re-reports the removals instead of losing them.
///
/// The returned `bool` reports whether the message carried a handled iMIP
/// part (REQUEST/REPLY/CANCEL), so the cascade gate in
/// [`Connector::extract`](crate::connector::Connector::extract) treats a
/// CANCEL — which emits no facts — as read and skips the LLM layer instead
/// of letting the cancellation prose author junk facts. REQUEST/REPLY facts
/// are appended to `facts` (the batch being built by the caller); the
/// caller's post-loop tombstone filter drops any fact whose `raw_reference`
/// matches a buffered CANCEL, so a CANCEL wins over a same-batch REQUEST
/// regardless of message order.
impl EmailConnector {
    pub(super) fn extract_invites(
        &self,
        message: &mail_parser::Message<'_>,
        raw_ref: &str,
        facts: &mut Vec<mimir_knowledge::normalize::NormalizedFact>,
    ) -> bool {
        let mut handled = false;
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
                Some("REQUEST") | Some("REPLY") => {
                    handled = true;
                    for vevent in crate::ical::parse_ical_to_vevents(ical) {
                        // Namespace the iMIP identity space (`imip:`) so a
                        // sender-chosen VEVENT UID can never collide with the
                        // `{uid_validity}:{uid}` references the JSON-LD and
                        // LLM layers write (a crafted `UID:17:99` CANCEL must
                        // not address another layer's facts). A UID-less
                        // VEVENT falls back to the email reference in its own
                        // `imip-email:` namespace.
                        let reference = match vevent.uid.as_deref() {
                            Some(uid) => format!("imip:{uid}"),
                            None => format!("imip-email:{raw_ref}"),
                        };
                        facts.extend(crate::ical::vevent_to_facts(
                            self.user_identity.as_deref(),
                            &vevent,
                            &reference,
                        ));
                    }
                }
                Some("CANCEL") => {
                    handled = true;
                    for vevent in crate::ical::parse_ical_to_vevents(ical) {
                        match vevent.uid.as_deref() {
                            Some(uid) => {
                                // Buffer the removal; the supervisor trashes
                                // the facts this instance authored for the
                                // namespaced reference after `extract` (issue
                                // #283, #247). The caller (`extract`) drops
                                // any facts this batch staged for the same
                                // reference after the message loop, so a
                                // CANCEL wins over a same-batch REQUEST
                                // regardless of message order.
                                self.prose_retry
                                    .lock()
                                    .unwrap()
                                    .push_tombstone(format!("imip:{uid}"));
                            }
                            None => {
                                debug!(
                                    raw_ref,
                                    "skipping CANCEL VEVENT with no UID; cannot map to prior facts"
                                );
                            }
                        }
                    }
                }
                other => {
                    debug!(raw_ref, method = ?other, "skipping text/calendar part: unsupported/absent METHOD");
                    continue;
                }
            }
        }
        handled
    }
}
