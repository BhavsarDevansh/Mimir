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

/// Escape a raw string for safe token-level AND matching in an FTS5 MATCH expression.
///
/// Unlike [`escape_fts5`], which forces an exact phrase, this splits the input
/// into tokens on any run of non-alphanumeric characters (mirroring the FTS5
/// unicode61 tokenizer, so hyphenated words like "check-in" become "check" and
/// "in") and joins the quoted tokens with ` AND `. Every term must be present
/// but may appear in any order, avoiding the false negatives of whole-query
/// phrase quoting. Each token is double-quoted so FTS5 operators cannot inject
/// syntax. A query that is itself wrapped in double quotes keeps exact-phrase
/// semantics via [`escape_fts5`].
///
/// Whitespace-only inputs are returned as empty strings to avoid overly broad
/// matches.
pub fn escape_fts5_tokens(query: &str) -> String {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // Whole-query phrase fallback: explicitly quoted input keeps exact-phrase
    // semantics (e.g. `"check in time"`).
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        return escape_fts5(&trimmed[1..trimmed.len() - 1]);
    }
    // Tokens are alphanumeric-only by construction (the split predicate
    // excludes quotes and every other FTS5 operator character), so quoting
    // each token fully neutralises FTS5 query syntax.
    let tokens: Vec<String> = trimmed
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"", t))
        .collect();
    if tokens.is_empty() {
        return String::new();
    }
    tokens.join(" AND ")
}

#[cfg(test)]
mod tests {
    use super::{escape_fts5, escape_fts5_tokens};

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

    #[test]
    fn escape_fts5_tokens_empty() {
        assert_eq!(escape_fts5_tokens(""), "");
    }

    #[test]
    fn escape_fts5_tokens_whitespace_only_returns_empty() {
        assert_eq!(escape_fts5_tokens("   "), "");
        assert_eq!(escape_fts5_tokens("*"), "");
        assert_eq!(escape_fts5_tokens("  *  "), "");
    }

    #[test]
    fn escape_fts5_tokens_single_word() {
        assert_eq!(escape_fts5_tokens("hello"), "\"hello\"");
    }

    #[test]
    fn escape_fts5_tokens_and_joins_terms() {
        // Issue #493: multi-word queries must match terms in any order, not
        // as an exact phrase.
        assert_eq!(
            escape_fts5_tokens("check in time"),
            "\"check\" AND \"in\" AND \"time\""
        );
    }

    #[test]
    fn escape_fts5_tokens_splits_hyphenated_words() {
        // The FTS5 unicode61 tokenizer indexes "check-in" as "check" and "in",
        // so the query must split on hyphens to match both forms.
        assert_eq!(escape_fts5_tokens("check-in"), "\"check\" AND \"in\"");
    }

    #[test]
    fn escape_fts5_tokens_neutralises_operators() {
        // Each token is quoted, so FTS5 operators cannot inject syntax.
        assert_eq!(
            escape_fts5_tokens("foo OR bar"),
            "\"foo\" AND \"OR\" AND \"bar\""
        );
        assert_eq!(
            escape_fts5_tokens("foo AND bar"),
            "\"foo\" AND \"AND\" AND \"bar\""
        );
        assert_eq!(
            escape_fts5_tokens("foo NOT bar"),
            "\"foo\" AND \"NOT\" AND \"bar\""
        );
    }

    #[test]
    fn escape_fts5_tokens_asterisk_and_quotes_are_separators() {
        assert_eq!(escape_fts5_tokens("foo*bar"), "\"foo\" AND \"bar\"");
        assert_eq!(escape_fts5_tokens("foo\"bar"), "\"foo\" AND \"bar\"");
    }

    #[test]
    fn escape_fts5_tokens_parentheses_are_separators() {
        assert_eq!(escape_fts5_tokens("(foo-bar)"), "\"foo\" AND \"bar\"");
    }

    #[test]
    fn escape_fts5_tokens_quoted_input_falls_back_to_phrase() {
        // Explicitly quoted input keeps exact-phrase semantics.
        assert_eq!(escape_fts5_tokens("\"check in time\""), "\"check in time\"");
    }

    #[test]
    fn escape_fts5_tokens_unicode_words_are_preserved() {
        assert_eq!(
            escape_fts5_tokens("café au lait"),
            "\"café\" AND \"au\" AND \"lait\""
        );
    }
}
