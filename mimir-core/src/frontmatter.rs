//! Shared YAML-frontmatter splitting for Markdown-based user files.
//!
//! Both the skills loader (`skills/markdown.rs`) and the personality preset
//! loader (`personality.rs`) read the same convention: an optional block at
//! the top of a Markdown file, delimited by standalone `---` lines, whose
//! contents are YAML. Keeping the splitter here avoids two divergent copies
//! of the same fence logic (DRY, issue #387).

/// Split a Markdown file into optional YAML frontmatter and body.
///
/// Returns:
/// - `None` when the file does not start with a `---` delimiter — the file
///   has no frontmatter and the whole content is the body.
/// - `Some(Err(_))` when the file starts with `---` but the block is never
///   closed by a standalone `---` line (malformed frontmatter).
/// - `Some(Ok((yaml, body)))` otherwise, where `yaml` is the raw frontmatter
///   content and `body` is the remaining text with leading whitespace
///   trimmed. The closing delimiter line is not part of either.
///
/// A `---` line inside the body does not close the block unless it is alone
/// on its line; `\r\n` line endings are handled because each chunk carries
/// its own terminator, so the body offset is exact for both conventions.
pub fn split_yaml_frontmatter(contents: &str) -> Option<Result<(&str, &str), &'static str>> {
    let trimmed = contents.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }

    let after_first = &trimmed[3..];
    let mut offset = 0usize;
    for chunk in after_first.split_inclusive('\n') {
        let line = chunk.trim_end_matches(['\r', '\n']);
        if line.trim() == "---" {
            // `offset` is the byte position where this chunk starts, so the
            // YAML is everything before it and the body starts after the
            // whole delimiter line (including its terminator).
            let yaml = &after_first[..offset];
            let body = &after_first[offset + chunk.len()..];
            return Some(Ok((yaml.trim(), body.trim_start())));
        }
        offset += chunk.len();
    }

    Some(Err(
        "frontmatter is not closed with a standalone '---' line",
    ))
}

#[cfg(test)]
mod tests {
    use super::split_yaml_frontmatter;

    #[test]
    fn no_frontmatter_returns_none() {
        assert_eq!(split_yaml_frontmatter("plain text"), None);
    }

    #[test]
    fn frontmatter_and_body_are_split() {
        let (yaml, body) = split_yaml_frontmatter("---\ndescription: x\n---\nbody")
            .expect("frontmatter present")
            .expect("closed");
        assert_eq!(yaml, "description: x");
        assert_eq!(body, "body");
    }

    #[test]
    fn leading_whitespace_before_delimiter_is_tolerated() {
        let (yaml, body) = split_yaml_frontmatter("\n\n---\ndescription: x\n---\nbody")
            .expect("frontmatter present")
            .expect("closed");
        assert_eq!(yaml, "description: x");
        assert_eq!(body, "body");
    }

    #[test]
    fn unterminated_frontmatter_is_an_error() {
        let result = split_yaml_frontmatter("---\ndescription: x\nbody");
        assert!(matches!(result, Some(Err(_))));
    }

    #[test]
    fn closing_delimiter_must_be_a_standalone_line() {
        let result = split_yaml_frontmatter("---\ndescription: x\n--- trailing\nbody");
        assert!(matches!(result, Some(Err(_))));
    }

    #[test]
    fn empty_frontmatter_is_valid() {
        let (yaml, body) = split_yaml_frontmatter("---\n---\nbody")
            .expect("frontmatter present")
            .expect("closed");
        assert_eq!(yaml, "");
        assert_eq!(body, "body");
    }

    #[test]
    fn body_leading_whitespace_is_trimmed() {
        let (_, body) = split_yaml_frontmatter("---\n---\n\n  \nbody")
            .expect("frontmatter present")
            .expect("closed");
        assert_eq!(body, "body");
    }

    #[test]
    fn body_without_trailing_newline_is_handled() {
        let (_, body) = split_yaml_frontmatter("---\n---\nbody")
            .expect("frontmatter present")
            .expect("closed");
        assert_eq!(body, "body");
    }

    #[test]
    fn crlf_line_endings_are_handled() {
        let (yaml, body) = split_yaml_frontmatter("---\r\ndescription: x\r\n---\r\nbody")
            .expect("frontmatter present")
            .expect("closed");
        assert_eq!(yaml, "description: x");
        assert_eq!(body, "body");
    }
}
