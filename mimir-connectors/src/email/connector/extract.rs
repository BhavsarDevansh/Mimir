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
/// the VEVENT `UID` as `raw_reference` — the stable iMIP identity RFC 5546
/// requires every method (REQUEST → REPLY → CANCEL) to share — so a CANCEL
/// maps 1:1 onto the facts the original invite authored. A VEVENT without a
/// `UID` (invalid per RFC 5545, tolerated by the lenient parser) falls back
/// to the email's `UIDVALIDITY`-qualified IMAP UID (`raw_ref`), preserving
/// the pre-#283 keying for malformed invites. The JSON-LD and LLM cascade
/// layers keep the email UID — they have no CANCEL lifecycle.
///
/// **CANCEL handling.** A `CANCEL` part emits no facts; each VEVENT `UID` is
/// buffered as a tombstone (reported via
/// [`Connector::extract_deletions`](crate::connector::Connector::extract_deletions)
/// and trashed through the shared #247 machinery), so a cancelled meeting
/// stops surfacing in "Upcoming". A CANCEL VEVENT without a `UID` cannot be
/// mapped and is skipped. `SEQUENCE` is not consulted in V1: the knowledge
/// graph does not store the original sequence, so a CANCEL trashes by `UID`
/// regardless of sequence. A CANCEL also drops any facts already staged in
/// `facts` for the same `UID` (a REQUEST and its CANCEL arriving in one sync
/// window): the CANCEL is the later signal, so the cancelled event is not
/// (re-)inserted by the same cycle that trashed its prior facts.
///
/// The returned `bool` reports whether the message carried a handled iMIP
/// part (REQUEST/REPLY/CANCEL), so the cascade gate in
/// [`Connector::extract`](crate::connector::Connector::extract) treats a
/// CANCEL — which emits no facts — as read and skips the LLM layer instead
/// of letting the cancellation prose author junk facts. REQUEST/REPLY facts
/// are appended to `facts` (the batch being built by the caller), so a CANCEL
/// later in the same batch can drop them.
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
                        // Key the cluster by the VEVENT UID (the stable iMIP
                        // identity a CANCEL maps onto); fall back to the
                        // email reference for UID-less VEVENTs.
                        let reference = vevent.uid.as_deref().unwrap_or(raw_ref);
                        facts.extend(crate::ical::vevent_to_facts(
                            self.user_identity.as_deref(),
                            &vevent,
                            reference,
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
                                // UID after `extract` (issue #283, #247).
                                self.tombstones.lock().unwrap().push(uid.to_string());
                                // Drop any facts this batch already staged
                                // for the same UID (a REQUEST and its CANCEL
                                // in one sync window): the supervisor trashes
                                // *before* inserting this cycle's facts, so
                                // without this the cancelled event's fresh
                                // facts would be inserted after the trash and
                                // survive. The CANCEL is the later signal.
                                facts.retain(|f| f.raw_reference.as_deref() != Some(uid));
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
