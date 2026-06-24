//! FTS5 query escaping utilities.

/// Escape a raw string for safe use in an FTS5 MATCH expression.
///
/// FTS5 treats spaces, `OR`, `AND`, `NOT`, `*`, `-`, `(` and `)` as query
/// operators. To avoid syntax errors and force literal matching, the input is
/// wrapped in a double-quoted phrase. Internal double quotes are doubled and
/// asterisks are replaced with spaces so that prefix-operator syntax cannot
/// appear inside the quoted phrase.
///
/// Whitespace-only inputs are returned as empty strings to avoid overly broad
/// matches.
pub fn escape_fts5(query: &str) -> String {
    if query.is_empty() {
        return String::new();
    }
    let escaped = query.replace('"', "\"\"").replace('*', " ");
    let trimmed = escaped.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    format!("\"{}\"", trimmed)
}

#[cfg(test)]
mod tests {
    use super::escape_fts5;

    #[test]
    fn escape_fts5_empty() {
        assert_eq!(escape_fts5(""), "");
    }

    #[test]
    fn escape_fts5_plain_word() {
        assert_eq!(escape_fts5("hello"), "\"hello\"");
    }

    #[test]
    fn escape_fts5_doubles_quotes() {
        assert_eq!(escape_fts5("foo\"bar"), "\"foo\"\"bar\"");
    }

    #[test]
    fn escape_fts5_replaces_asterisk_with_space() {
        assert_eq!(escape_fts5("foo*bar"), "\"foo bar\"");
    }

    #[test]
    fn escape_fts5_boolean_operators_become_literal_phrase() {
        // Without escaping, "foo OR bar" would be parsed as a boolean expression.
        assert_eq!(escape_fts5("foo OR bar"), "\"foo OR bar\"");
        assert_eq!(escape_fts5("foo AND bar"), "\"foo AND bar\"");
        assert_eq!(escape_fts5("foo NOT bar"), "\"foo NOT bar\"");
    }

    #[test]
    fn escape_fts5_parentheses_and_dash_literal() {
        assert_eq!(escape_fts5("(foo-bar)"), "\"(foo-bar)\"");
    }

    #[test]
    fn escape_fts5_whitespace_only_returns_empty() {
        assert_eq!(escape_fts5("   "), "");
        assert_eq!(escape_fts5("*"), "");
        assert_eq!(escape_fts5("  *  "), "");
    }

    #[test]
    fn escape_fts5_trims_surrounding_whitespace() {
        // Issue #163: surrounding whitespace must not leak into the quoted
        // phrase, otherwise FTS5 phrase matching includes the padding.
        assert_eq!(escape_fts5("  hello  "), "\"hello\"");
        assert_eq!(escape_fts5("\thello\n"), "\"hello\"");
        assert_eq!(escape_fts5("  foo OR bar  "), "\"foo OR bar\"");
    }
}
