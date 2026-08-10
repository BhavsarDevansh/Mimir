//! HTML `<script type="application/ld+json">` block extraction.

pub(super) fn extract_jsonld_blocks(html: &str) -> Vec<&str> {
    // ASCII lowercasing is a 1:1 byte mapping, so byte offsets in the
    // lowercased string correspond exactly to offsets in the original.
    let lower = html.to_ascii_lowercase();
    let mut blocks = Vec::new();
    let mut pos = 0;
    let script_tag_len = "<script".len();
    let close_tag_len = "</script>".len();

    while let Some(rel) = lower[pos..].find("<script") {
        let tag_start = pos + rel;
        let after_tag_name = tag_start + script_tag_len;

        // Find the end of the opening `<script ...>` tag.
        let Some(greater_rel) = lower[after_tag_name..].find('>') else {
            break;
        };
        let tag_end = after_tag_name + greater_rel;
        let tag_inner = &html[after_tag_name..tag_end];

        // Content starts after `>`.
        let content_start = tag_end + 1;

        // Find the closing `</script>` (case-insensitive). Every `<script>`
        // element has a closing `</script>` — we must skip past it *regardless*
        // of whether this is a JSON-LD script, so JavaScript content
        // containing `<script` string literals (common in templating/tracking
        // snippets) is not re-scanned for JSON-LD blocks. HTML5 §12.1.2: a
        // script element's text content terminates at the first `</script>`
        // end tag.
        let Some(close_rel) = lower[content_start..].find("</script>") else {
            break;
        };
        let content_end = content_start + close_rel;

        if has_jsonld_type(tag_inner) {
            blocks.push(html[content_start..content_end].trim());
        }

        pos = content_end + close_tag_len;
    }
    blocks
}

/// Check whether a `<script>` tag's inner attribute string contains
/// `type="application/ld+json"` (case-insensitive).
pub(super) fn has_jsonld_type(tag_inner: &str) -> bool {
    for (name, value) in parse_html_attributes(tag_inner) {
        if name.eq_ignore_ascii_case("type")
            && value.trim().eq_ignore_ascii_case("application/ld+json")
        {
            return true;
        }
    }
    false
}

/// Parse HTML attribute name=value pairs from the text between `<script` and
/// `>`.
///
/// Handles double-quoted, single-quoted, and unquoted values, and boolean
/// attributes (no `=`). Attribute names are matched case-insensitively by
/// callers. This is a minimal parser sufficient for `<script>` tag
/// attributes — it does not attempt to be a general HTML attribute parser.
pub(super) fn parse_html_attributes(s: &str) -> Vec<(String, String)> {
    let chars: Vec<char> = s.chars().collect();
    let mut attrs = Vec::new();
    let mut i = 0;

    while i < chars.len() {
        // Skip whitespace.
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }

        // Read attribute name (up to `=`, whitespace, or end).
        let name_start = i;
        while i < chars.len() && !chars[i].is_whitespace() && chars[i] != '=' {
            i += 1;
        }
        let name: String = chars[name_start..i].iter().collect();
        if name.is_empty() {
            i += 1;
            continue;
        }

        // Skip whitespace before potential `=`.
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }

        if i < chars.len() && chars[i] == '=' {
            i += 1; // consume `=`
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            if i >= chars.len() {
                attrs.push((name, String::new()));
                break;
            }
            let value = if chars[i] == '"' || chars[i] == '\'' {
                let quote = chars[i];
                i += 1; // skip opening quote
                let val_start = i;
                while i < chars.len() && chars[i] != quote {
                    i += 1;
                }
                let val: String = chars[val_start..i].iter().collect();
                if i < chars.len() {
                    i += 1; // skip closing quote
                }
                val
            } else {
                let val_start = i;
                while i < chars.len() && !chars[i].is_whitespace() {
                    i += 1;
                }
                chars[val_start..i].iter().collect()
            };
            attrs.push((name, value));
        } else {
            // Boolean attribute (no value).
            attrs.push((name, String::new()));
        }
    }
    attrs
}
