//! Reflow Markdown prose to the Mimir single-line standard.
//!
//! The AGENTS.md "Finishing Work" rules require every prose paragraph and
//! list-item continuation to be a single flowing line. This tool walks the
//! CommonMark block tree with `pulldown-cmark` and joins the source lines of
//! each paragraph (and each tight list item) onto one line, leaving tables,
//! fenced code blocks, nested lists, and blockquote prose untouched.
//!
//! Blockquote paragraphs are only touched when they are unambiguous
//! "field-lists" (every line starts with a `**Field:**` marker): each entry is
//! then split into its own blockquote paragraph with a blank `>` line between
//! entries. Wrapped blockquote prose and mixed field-list/wrapped regions are
//! left alone for manual restructuring.

#![deny(unsafe_code)]

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use std::path::{Path, PathBuf};

const OPTIONS: Options = Options::ENABLE_TABLES
    .union(Options::ENABLE_TASKLISTS)
    .union(Options::ENABLE_STRIKETHROUGH);

#[derive(Clone, Copy, PartialEq)]
enum Frame {
    Paragraph,
    Item,
    BlockQuote,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args
        .iter()
        .find(|a| a.starts_with("--"))
        .map(|s| s.as_str())
        .unwrap_or("reflow");
    let paths: Vec<PathBuf> = args
        .iter()
        .skip(1)
        .filter(|a| !a.starts_with("--"))
        .map(PathBuf::from)
        .collect();
    let files = if paths.is_empty() {
        collect_md_files(Path::new("."))
    } else {
        paths
    };
    match mode {
        "--survey" => survey(files),
        "--check" => {
            let mut changed = 0usize;
            for file in files {
                let Ok(src) = std::fs::read_to_string(&file) else {
                    eprintln!("skip {}: unreadable", file.display());
                    continue;
                };
                if reflow_named(&src, &file) != src {
                    changed += 1;
                    println!("would reflow: {}", file.display());
                }
            }
            println!("{changed} files would change");
            if changed > 0 {
                std::process::exit(1);
            }
        }
        _ => {
            let mut changed = 0usize;
            for file in files {
                let Ok(src) = std::fs::read_to_string(&file) else {
                    eprintln!("skip {}: unreadable", file.display());
                    continue;
                };
                let reflowed = reflow_named(&src, &file);
                if reflowed != src {
                    changed += 1;
                    if let Err(e) = std::fs::write(&file, &reflowed) {
                        eprintln!("skip {}: {e}", file.display());
                        continue;
                    }
                    println!("reflowed: {}", file.display());
                }
            }
            println!("{changed} files changed");
        }
    }
}

fn collect_md_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n != "target" && n != ".git") {
                    stack.push(p);
                }
            } else if p.extension().is_some_and(|x| x == "md") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn survey(files: Vec<PathBuf>) {
    let mut para_files = 0usize;
    let mut bq_files = 0usize;
    let mut tight_files = 0usize;
    for file in files {
        let Ok(src) = std::fs::read_to_string(&file) else {
            eprintln!("skip {}: unreadable", file.display());
            continue;
        };
        let (paras, bqs, tights) = analyze(&src);
        if !paras.is_empty() {
            para_files += 1;
            println!("PARAS {} ({} regions)", file.display(), paras.len());
        }
        if !bqs.is_empty() {
            bq_files += 1;
            println!("BQ {} ({} regions)", file.display(), bqs.len());
            for (i, b) in bqs.iter().enumerate() {
                println!("  BQ#{i}: {b:?}");
            }
        }
        if !tights.is_empty() {
            tight_files += 1;
            println!("TIGHT {} ({} regions)", file.display(), tights.len());
            for (i, t) in tights.iter().enumerate() {
                println!("  TIGHT#{i}: {t:?}");
            }
        }
    }
    println!(
        "files with wrapped paras: {para_files}, wrapped bq paras: {bq_files}, wrapped tight items: {tight_files}"
    );
}

fn analyze(src: &str) -> (Vec<String>, Vec<String>, Vec<String>) {
    let parser = Parser::new_ext(src, OPTIONS);
    let mut paras = Vec::new();
    let mut bqs = Vec::new();
    let mut tights = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut bq_depth = 0usize;
    let mut item_spans: Vec<(usize, usize)> = Vec::new();
    let mut para_spans: Vec<(usize, usize)> = Vec::new();
    let mut block_spans: Vec<(usize, usize)> = Vec::new();
    for (event, range) in parser.into_offset_iter() {
        match event {
            Event::Start(Tag::Paragraph) => stack.push(Frame::Paragraph),
            Event::Start(Tag::Item) => {
                stack.push(Frame::Item);
                item_spans.push((range.start, range.end));
            }
            Event::Start(Tag::BlockQuote(_)) => {
                bq_depth += 1;
                stack.push(Frame::BlockQuote);
            }
            Event::Start(Tag::List(_))
            | Event::Start(Tag::CodeBlock(_))
            | Event::Start(Tag::Table(_))
            | Event::Start(Tag::HtmlBlock)
            | Event::Start(Tag::FootnoteDefinition(_))
            | Event::Start(Tag::DefinitionList) => {
                block_spans.push((range.start, range.end));
            }
            Event::End(tag_end) => {
                let frame = match tag_end {
                    TagEnd::Paragraph => Some(Frame::Paragraph),
                    TagEnd::Item => Some(Frame::Item),
                    TagEnd::BlockQuote(_) => Some(Frame::BlockQuote),
                    _ => None,
                };
                if let Some(frame) = frame {
                    if stack.last() == Some(&frame) {
                        stack.pop();
                        match frame {
                            Frame::Paragraph => {
                                let text = &src[range.start..range.end];
                                if text.lines().filter(|l| !is_blank_line(l)).count() > 1 {
                                    if bq_depth > 0 {
                                        bqs.push(text.to_string());
                                    } else {
                                        paras.push(text.to_string());
                                    }
                                }
                                para_spans.push((range.start, range.end));
                            }
                            Frame::BlockQuote => bq_depth -= 1,
                            Frame::Item => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }
    for (is, ie) in item_spans {
        let has_para = para_spans.iter().any(|(ps, pe)| ps >= &is && pe <= &ie);
        if !has_para {
            let mut gaps = vec![(is, ie)];
            for (bs, be) in &block_spans {
                let bs_line = src[..*bs].rfind('\n').map(|i| i + 1).unwrap_or(0);
                if bs_line > is && *be <= ie {
                    gaps = split_gaps(gaps, bs_line, *be);
                }
            }
            for (gs, ge) in gaps {
                let text = &src[gs..ge];
                if text.lines().filter(|l| !is_blank_line(l)).count() > 1 {
                    tights.push(text.to_string());
                }
            }
        }
    }
    (paras, bqs, tights)
}

fn split_gaps(gaps: Vec<(usize, usize)>, bs: usize, be: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (gs, ge) in gaps {
        if bs <= gs && be >= ge {
            continue;
        }
        if bs >= ge || be <= gs {
            out.push((gs, ge));
            continue;
        }
        if gs < bs {
            out.push((gs, bs));
        }
        if ge > be {
            out.push((be, ge));
        }
    }
    out
}

fn reflow_named(src: &str, file: &Path) -> String {
    let parser = Parser::new_ext(src, OPTIONS);
    let mut regions: Vec<(usize, usize, String)> = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut bq_depth = 0usize;
    let mut item_spans: Vec<(usize, usize)> = Vec::new();
    let mut para_spans: Vec<(usize, usize)> = Vec::new();
    let mut block_spans: Vec<(usize, usize)> = Vec::new();
    for (event, range) in parser.into_offset_iter() {
        match event {
            Event::Start(Tag::Paragraph) => stack.push(Frame::Paragraph),
            Event::Start(Tag::Item) => {
                stack.push(Frame::Item);
                item_spans.push((range.start, range.end));
            }
            Event::Start(Tag::BlockQuote(_)) => {
                bq_depth += 1;
                stack.push(Frame::BlockQuote);
            }
            Event::Start(Tag::List(_))
            | Event::Start(Tag::CodeBlock(_))
            | Event::Start(Tag::Table(_))
            | Event::Start(Tag::HtmlBlock)
            | Event::Start(Tag::FootnoteDefinition(_))
            | Event::Start(Tag::DefinitionList) => {
                block_spans.push((range.start, range.end));
            }
            Event::End(tag_end) => {
                let frame = match tag_end {
                    TagEnd::Paragraph => Some(Frame::Paragraph),
                    TagEnd::Item => Some(Frame::Item),
                    TagEnd::BlockQuote(_) => Some(Frame::BlockQuote),
                    _ => None,
                };
                if let Some(frame) = frame {
                    if stack.last() == Some(&frame) {
                        stack.pop();
                        match frame {
                            Frame::Paragraph => {
                                para_spans.push((range.start, range.end));
                                let text = &src[range.start..range.end];
                                let reflowed = if bq_depth == 0 {
                                    reflow_region(text)
                                } else {
                                    reflow_blockquote_field_list(text, bq_depth)
                                };
                                if reflowed != text {
                                    regions.push((range.start, range.end, reflowed));
                                }
                            }
                            Frame::BlockQuote => bq_depth -= 1,
                            Frame::Item => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }
    for (is, ie) in item_spans {
        let has_para = para_spans.iter().any(|(ps, pe)| ps >= &is && pe <= &ie);
        if !has_para {
            let mut gaps = vec![(is, ie)];
            for (bs, be) in &block_spans {
                let bs_line = src[..*bs].rfind('\n').map(|i| i + 1).unwrap_or(0);
                if bs_line > is && *be <= ie {
                    gaps = split_gaps(gaps, bs_line, *be);
                }
            }
            for (gs, ge) in gaps {
                if gs > ge {
                    eprintln!(
                        "inverted gap {gs}..{ge} in item {is}..{ie} (file {})",
                        file.display()
                    );
                    continue;
                }
                let text = &src[gs..ge];
                let reflowed = reflow_region(text);
                if reflowed != text {
                    regions.push((gs, ge, reflowed));
                }
            }
        }
    }
    apply_regions(src, regions)
}

fn reflow_blockquote_field_list(text: &str, depth: usize) -> String {
    let ends_with_newline = text.ends_with('\n');
    let trimmed = text.trim_end_matches('\n');
    let lines: Vec<&str> = trimmed.split('\n').collect();
    let stripped: Vec<&str> = lines
        .iter()
        .map(|l| strip_blockquote_markers(l).trim_end())
        .collect();
    if stripped.iter().filter(|l| !l.is_empty()).count() <= 1 {
        return text.to_string();
    }
    if !stripped.iter().all(|l| l.is_empty() || l.starts_with("**")) {
        return text.to_string();
    }
    let marker = "> ".repeat(depth);
    let blank = marker.trim_end();
    let mut out = String::new();
    for (i, line) in stripped.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        if i > 0 {
            out.push_str(blank);
            out.push('\n');
            out.push_str(&marker);
        }
        out.push_str(line);
        if i + 1 < stripped.len() || ends_with_newline {
            out.push('\n');
        }
    }
    out
}

fn strip_blockquote_markers(line: &str) -> &str {
    let mut l = line.trim_start();
    while let Some(rest) = l.strip_prefix('>') {
        l = rest.strip_prefix(' ').unwrap_or(rest).trim_start();
    }
    l
}

fn is_blank_line(line: &str) -> bool {
    let mut l = line.trim();
    while let Some(rest) = l.strip_prefix('>') {
        l = rest.trim_start();
    }
    l.is_empty()
}

fn reflow_region(text: &str) -> String {
    let trailing_newlines = text.len() - text.trim_end_matches('\n').len();
    let trimmed = text.trim_end_matches('\n');
    let lines: Vec<&str> = trimmed.split('\n').collect();
    if lines.iter().filter(|l| !is_blank_line(l)).count() <= 1 {
        return text.to_string();
    }
    let mut parts: Vec<&str> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let l = if i == 0 { line } else { line.trim_start() };
        parts.push(strip_blockquote_markers(l).trim_end());
    }
    let joined = parts.join(" ").trim_end().to_string();
    format!("{joined}{}", "\n".repeat(trailing_newlines))
}

fn apply_regions(src: &str, mut regions: Vec<(usize, usize, String)>) -> String {
    regions.sort_by_key(|(s, _, _)| *s);
    let mut out = String::with_capacity(src.len());
    let mut pos = 0usize;
    for (start, end, replacement) in regions {
        out.push_str(&src[pos..start]);
        out.push_str(&replacement);
        pos = end;
    }
    out.push_str(&src[pos..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reflow_str(src: &str) -> String {
        reflow_named(src, Path::new("<test>"))
    }

    #[test]
    fn plain_paragraph_joins() {
        assert_eq!(
            reflow_str("This is a long\nparagraph that wraps.\n"),
            "This is a long paragraph that wraps.\n"
        );
    }

    #[test]
    fn single_line_paragraph_untouched() {
        assert_eq!(reflow_str("One line.\n"), "One line.\n");
    }

    #[test]
    fn loose_list_item_joins() {
        assert_eq!(
            reflow_str("1. First para\n   of item.\n\n   Second para\n   of item.\n"),
            "1. First para of item.\n\n   Second para of item.\n"
        );
    }

    #[test]
    fn tight_list_item_joins() {
        assert_eq!(
            reflow_str("- This is a long\n  tight item\n- Second\n"),
            "- This is a long tight item\n- Second\n"
        );
    }

    #[test]
    fn nested_list_preserved() {
        assert_eq!(
            reflow_str("- Parent\n  - Child one\n\n    Child two\n"),
            "- Parent\n  - Child one\n\n    Child two\n"
        );
    }

    #[test]
    fn tight_item_with_nested_list_joins_text_only() {
        assert_eq!(
            reflow_str(
                "- This is a long\n  list item that wraps\n  - nested item\n- Second item\n"
            ),
            "- This is a long list item that wraps\n  - nested item\n- Second item\n"
        );
    }

    #[test]
    fn blockquote_field_list_split_into_entries() {
        assert_eq!(
            reflow_str("> **Phase:** 3\n> **Status:** Done\n"),
            "> **Phase:** 3\n>\n> **Status:** Done\n"
        );
    }

    #[test]
    fn wrapped_blockquote_prose_left_alone() {
        assert_eq!(
            reflow_str("> This is a long\n> blockquote that wraps.\n"),
            "> This is a long\n> blockquote that wraps.\n"
        );
    }

    #[test]
    fn blockquote_field_list_with_hard_breaks() {
        assert_eq!(
            reflow_str("> **Crate:** `x`  \n> **Backend:** SQLite  \n"),
            "> **Crate:** `x`\n>\n> **Backend:** SQLite\n"
        );
    }

    #[test]
    fn fenced_code_preserved() {
        assert_eq!(
            reflow_str("```rust\nlet x = 1;\nlet y = 2;\n```\n"),
            "```rust\nlet x = 1;\nlet y = 2;\n```\n"
        );
    }

    #[test]
    fn table_preserved() {
        assert_eq!(
            reflow_str("| A | B |\n|---|---|\n| 1 | 2 |\n"),
            "| A | B |\n|---|---|\n| 1 | 2 |\n"
        );
    }

    #[test]
    fn hard_break_inside_item_joins() {
        assert_eq!(
            reflow_str("- Features: `x`  \n    Example: `y`\n"),
            "- Features: `x` Example: `y`\n"
        );
    }

    #[test]
    fn heading_and_rule_untouched() {
        assert_eq!(
            reflow_str("# Heading\n\n---\n\nText.\n"),
            "# Heading\n\n---\n\nText.\n"
        );
    }

    #[test]
    fn paragraph_after_blockquote_joins() {
        assert_eq!(
            reflow_str("> quote\n\nThis is a long\nparagraph.\n"),
            "> quote\n\nThis is a long paragraph.\n"
        );
    }

    #[test]
    fn blank_line_after_list_item_preserved() {
        assert_eq!(
            reflow_str("- `rust-version` — `1.85`\n\nEach member uses `field.workspace = true`.\n"),
            "- `rust-version` — `1.85`\n\nEach member uses `field.workspace = true`.\n"
        );
    }

    #[test]
    fn blank_line_before_heading_preserved() {
        assert_eq!(
            reflow_str("- Version bumped 0.1.0 → 0.1.1.\n\n## [0.1.1]\n"),
            "- Version bumped 0.1.0 → 0.1.1.\n\n## [0.1.1]\n"
        );
    }

    #[test]
    fn second_item_with_nested_block_keeps_indent() {
        assert_eq!(
            reflow_str("- First item\n- Second item\n  with wrap\n  - nested\n"),
            "- First item\n- Second item with wrap\n  - nested\n"
        );
    }
}
