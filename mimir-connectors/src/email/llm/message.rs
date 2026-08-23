//! Email message pre-processing: spam classification, body text, subject
//! canonicalisation.

use mail_parser::Message;

/// Domains of bulk *marketing* platforms — providers whose product is
/// newsletter/campaign delivery (Mailchimp, HubSpot, …). Mail sent from these
/// is marketing, so it is skipped before any extraction layer.
/// General-purpose email-service providers that also deliver transactional
/// receipts, bookings, and account notices (SendGrid, Mailgun, Postmark,
/// Amazon SES, Mandrill, SparkPost, Brevo) are deliberately *not* listed
/// here: a booking or bank statement routed through them must still reach
/// the extraction cascade. Those messages are skipped only when they carry
/// an explicit bulk signal (the `List-Unsubscribe` header — see
/// [`is_likely_spam`]).
const MARKETING_SENDER_DOMAINS: &[&str] = &[
    "mailchimp.com",
    "hubspot.com",
    "mailerlite.com",
    "constantcontact.com",
    "campaignmonitor.com",
    "elasticemail.com",
    "mail.marketing",
    "email-od.com",
];

pub(super) fn sender_domain(from: Option<&str>) -> Option<String> {
    let addr = from?;
    let domain = addr.rsplit_once('@').map(|(_, d)| d)?;
    let domain = domain.trim().trim_end_matches('>').to_ascii_lowercase();
    if domain.is_empty() {
        None
    } else {
        Some(domain)
    }
}

/// Conservative deterministic spam gate: skip a message for obvious
/// bulk-marketing mail. The gate runs before every extraction layer — the
/// iMIP and JSON-LD deterministic layers and the LLM prose layer — so a
/// marketing broadcast can never author facts. A message is skipped when
/// either (a) it carries a `List-Unsubscribe` header — the universal
/// bulk-mail signal (RFC 8058) that transactional receipts, bookings, and
/// account notices never carry — or (b) its sender domain is a pure
/// marketing platform (see [`MARKETING_SENDER_DOMAINS`]). Provider origin
/// alone never skips a message, so a transactional email routed through a
/// general-purpose ESP (SendGrid, Mailgun, Postmark, Amazon SES) still
/// reaches the cascade.
pub(crate) fn is_likely_spam(from_addr: Option<&str>, has_unsubscribe: bool) -> bool {
    // Explicit bulk signal: a `List-Unsubscribe` header is present only on
    // bulk mail (newsletters, campaigns, promotional broadcasts). This gate
    // never drops transactional mail, which does not carry one.
    if has_unsubscribe {
        return true;
    }
    let Some(domain) = sender_domain(from_addr) else {
        return false;
    };
    // Exact ESP domain, or a subdomain of one (`mc.us1.sendgrid.net`). The
    // `strip_suffix` + `ends_with('.')` check avoids a per-domain allocation
    // that `format!(".{esp}")` would incur on every email.
    MARKETING_SENDER_DOMAINS.iter().any(|esp| {
        domain == *esp
            || domain
                .strip_suffix(esp)
                .is_some_and(|rest| rest.ends_with('.'))
    })
}

/// Best-effort plain-text body: the first text/plain body, or the first HTML
/// body stripped of markup when no text/plain part exists.
pub(crate) fn body_text(message: &Message<'_>) -> Option<String> {
    if let Some(text) = message.body_text(0) {
        let text = text.into_owned();
        if !text.trim().is_empty() {
            return Some(text);
        }
    }
    message
        .body_html(0)
        .map(|html| strip_html(&html))
        .filter(|t| !t.trim().is_empty())
}

/// Naive HTML-to-text: drop tags, decode a few common entities, collapse
/// whitespace. Good enough to hand the LLM prose from an HTML-only email; the
pub(super) fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                if in_tag {
                    out.push(' ');
                    in_tag = false;
                }
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    collapse_whitespace(&decoded)
}

fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = true;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

/// Generic self-references the LLM may use for the mailbox owner. These are
/// normalised to the exact `user_identity` so the fact resolves to the
/// canonical user entity instead of a "the user" / "I" entity.
const GENERIC_USER_REFERENCES: &[&str] = &["i", "me", "myself", "user", "the user"];

pub(super) fn canonicalise_subject(subject: &str, user_identity: Option<&str>) -> String {
    let Some(identity) = user_identity else {
        return subject.to_string();
    };
    let trimmed = subject.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower == identity.to_ascii_lowercase() || GENERIC_USER_REFERENCES.contains(&lower.as_str()) {
        identity.to_string()
    } else {
        subject.to_string()
    }
}
