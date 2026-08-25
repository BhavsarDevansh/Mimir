//! Interactive chat REPL with persistent history.
//!
//! The daemon owns the session and conversation history; this client is
//! fully stateless except for the optional `session_id` used to resume.
use colored::Colorize;
use mimir_api_types::ChatRequest;
use mimir_client::MimirClient;
use mimir_core::paths;
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;

/// Initial chat session options sourced from CLI flags (issue #81).
pub struct ChatOptions {
    pub model: Option<String>,
    pub verbose: bool,
    pub incognito: bool,
    pub personality: Option<String>,
}

/// Mutable per-session state for the REPL (issue #81).
struct SessionState {
    model: Option<String>,
    personality: Option<String>,
    incognito: bool,
    verbose: bool,
}

/// Strip a prefix from a string only if followed by a space or end-of-string.
/// This prevents unintended matches like "/modelx" when looking for "/model".
fn strip_command_prefix<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = s.strip_prefix(prefix)?;
    if rest.is_empty() || rest.starts_with(' ') {
        Some(rest)
    } else {
        None
    }
}

pub async fn handle_chat(transport: &crate::transport::DaemonTransport, opts: ChatOptions) {
    let client = crate::cli_util::make_client(transport);
    let mut session_id: Option<i64> = None;
    let mut session = SessionState {
        model: opts.model,
        personality: opts.personality,
        incognito: opts.incognito,
        verbose: opts.verbose,
    };

    let mut editor = match DefaultEditor::new() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Failed to initialize line editor: {}", e);
            std::process::exit(1);
        }
    };

    let history_path = match paths::history_path() {
        Ok(path) => {
            if let Some(parent) = path.parent()
                && let Err(e) = std::fs::create_dir_all(parent)
            {
                eprintln!("Warning: failed to create history directory: {}", e);
            }

            if path.exists()
                && let Err(e) = editor.load_history(&path)
            {
                eprintln!("Warning: failed to load history: {}", e);
            }
            Some(path)
        }
        Err(e) => {
            eprintln!(
                "Warning: failed to resolve history path: {e}; history will not be persisted"
            );
            None
        }
    };

    println!("Mimir chat. Type /help for commands, /exit to quit.");
    println!("Press Ctrl+C during input to exit.");

    loop {
        let prompt = "Mimir> ";
        let line = match editor.readline(prompt) {
            Ok(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if trimmed == "/exit" {
                    println!("Goodbye.");
                    break;
                }
                if trimmed == "/help" {
                    print_help();
                    if !session.incognito {
                        editor.add_history_entry(&line).ok();
                    }
                    continue;
                }
                if trimmed == "/clear" {
                    session_id = None;
                    println!("Session reset.");
                    if !session.incognito {
                        editor.add_history_entry(&line).ok();
                    }
                    continue;
                }
                if trimmed == "/memory" {
                    match client.memory().await {
                        Ok(content) => println!("{}", content),
                        Err(e) => eprintln!("Failed to load memory: {}", e),
                    }
                    if !session.incognito {
                        editor.add_history_entry(&line).ok();
                    }
                    continue;
                }
                if trimmed == "/status" {
                    match client.status().await {
                        Ok(status) => {
                            println!(
                                "Version: {}, Uptime: {}s, Endpoint: {}, Model: {}",
                                status.version,
                                status.uptime_seconds,
                                status.endpoint,
                                status.model
                            );
                            println!(
                                "Queue: user={} system={}, Workers: {}",
                                status.queue_depth_user,
                                status.queue_depth_system,
                                status.worker_threads
                            );
                            println!(
                                "Config: {} ({})",
                                status.config_path.as_deref().unwrap_or("unknown"),
                                if status.config_exists {
                                    "exists"
                                } else {
                                    "missing"
                                }
                            );
                            println!(
                                "LLM: reachable={}, context_window={:?}",
                                status.llm_reachable, status.context_window
                            );
                            println!(
                                "Memory: {}, {} / {} chars ({:.1}%)",
                                if status.memory_exists {
                                    "exists"
                                } else {
                                    "NOT FOUND"
                                },
                                status.memory_chars,
                                status.memory_limit,
                                status.memory_usage_pct
                            );
                        }
                        Err(e) => eprintln!("Status request failed: {}", e),
                    }
                    if !session.incognito {
                        editor.add_history_entry(&line).ok();
                    }
                    continue;
                }
                if trimmed == "/history" {
                    match handle_history(&client, &mut session_id).await {
                        Ok(()) => {}
                        Err(e) => eprintln!("History error: {}", e),
                    }
                    if !session.incognito {
                        editor.add_history_entry(&line).ok();
                    }
                    continue;
                }
                if let Some(rest) = strip_command_prefix(trimmed, "/model") {
                    let value = rest.trim();
                    if value.is_empty() {
                        let current = session.model.as_deref().unwrap_or("(server default)");
                        println!("Model: {current}");
                    } else {
                        session.model = Some(value.to_string());
                        println!("Model set to {value}.");
                    }
                    if !session.incognito {
                        editor.add_history_entry(&line).ok();
                    }
                    continue;
                }
                if let Some(rest) = strip_command_prefix(trimmed, "/personality") {
                    let value = rest.trim();
                    if value.is_empty() {
                        let current = session.personality.as_deref().unwrap_or("(server default)");
                        println!("Personality: {current}");
                    } else {
                        session.personality = Some(value.to_string());
                        println!("Personality set to {value}.");
                    }
                    if !session.incognito {
                        editor.add_history_entry(&line).ok();
                    }
                    continue;
                }
                if let Some(rest) = strip_command_prefix(trimmed, "/incognito") {
                    let value = rest.trim();
                    session.incognito = match value {
                        "" => !session.incognito,
                        "on" | "true" | "1" => true,
                        "off" | "false" | "0" => false,
                        other => {
                            eprintln!("Unknown value '{other}'. Use on/off.");
                            if !session.incognito {
                                editor.add_history_entry(&line).ok();
                            }
                            continue;
                        }
                    };
                    println!(
                        "Incognito {}.",
                        if session.incognito { "on" } else { "off" }
                    );
                    if !session.incognito {
                        editor.add_history_entry(&line).ok();
                    }
                    continue;
                }
                if let Some(rest) = strip_command_prefix(trimmed, "/verbose") {
                    let value = rest.trim();
                    session.verbose = match value {
                        "" => !session.verbose,
                        "on" | "true" | "1" => true,
                        "off" | "false" | "0" => false,
                        other => {
                            eprintln!("Unknown value '{other}'. Use on/off.");
                            if !session.incognito {
                                editor.add_history_entry(&line).ok();
                            }
                            continue;
                        }
                    };
                    println!("Verbose {}.", if session.verbose { "on" } else { "off" });
                    if !session.incognito {
                        editor.add_history_entry(&line).ok();
                    }
                    continue;
                }

                let mut input = trimmed.to_string();
                while input.ends_with('\\') {
                    input.pop();
                    match editor.readline("... ") {
                        Ok(cont) => {
                            input.push('\n');
                            input.push_str(cont.trim());
                        }
                        Err(_) => break,
                    }
                }

                if !session.incognito {
                    editor.add_history_entry(&line).ok();
                }
                input
            }
            Err(ReadlineError::Interrupted) => {
                println!("^C");
                break;
            }
            Err(ReadlineError::Eof) => {
                println!("Goodbye.");
                break;
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                break;
            }
        };

        let req = ChatRequest {
            session_id,
            message: line,
            model: session.model.clone(),
            personality_preset: session.personality.clone(),
            incognito: Some(session.incognito),
        };
        let verbose = session.verbose;

        match client.chat_stream(req).await {
            Ok(mut stream) => {
                use futures::StreamExt;
                let mut last_usage: Option<mimir_api_types::Usage> = None;
                while let Some(item) = stream.next().await {
                    match item {
                        Ok(mimir_api_types::StreamItem::Text(text)) => {
                            print!("{}", text);
                            use std::io::Write;
                            let _ = std::io::stdout().flush();
                        }
                        Ok(mimir_api_types::StreamItem::Usage(u)) => {
                            last_usage = Some(u);
                        }
                        Ok(mimir_api_types::StreamItem::SessionId(id)) => {
                            session_id = id.parse().ok();
                        }
                        Ok(mimir_api_types::StreamItem::ToolCall(info)) => {
                            eprintln!(
                                "{}",
                                format!("🔧 {} → {}", info.display_name, info.result)
                                    .dimmed()
                                    .italic()
                            );
                        }
                        Ok(mimir_api_types::StreamItem::ToolCallStart(info)) => {
                            eprintln!("{}", format!("🔧 {}…", info.display_name).dimmed().italic());
                        }
                        Err(e) => {
                            eprintln!(
                                "
Stream error: {}",
                                e
                            );
                            break;
                        }
                    }
                }
                println!();
                if verbose && let Some(u) = last_usage {
                    eprintln!(
                        "Tokens: {} prompt + {} completion = {} total",
                        u.prompt_tokens, u.completion_tokens, u.total_tokens
                    );
                }
            }
            Err(e) => {
                eprintln!("LLM stream error: {}", e);
            }
        }
    }

    if let Some(history_path) = history_path {
        if let Err(e) = editor.save_history(&history_path) {
            eprintln!("Warning: failed to save history: {}", e);
        }
    }
}

fn print_help() {
    println!("Commands:");
    println!("  /exit       - Exit the REPL");
    println!("  /clear      - Reset the conversation session");
    println!("  /memory     - Show the live condensed memory block");
    println!("  /status     - Quick health check");
    println!("  /history    - Resume a previous conversation");
    println!("  /model [m]  - Show or set the LLM model override");
    println!("  /personality [p] - Show or set the personality preset");
    println!("  /incognito [on|off] - Toggle incognito (skip persistence)");
    println!("  /verbose [on|off]   - Toggle token usage reporting");
    println!();
    println!("Multi-line input: end a line with \\ to continue.");
}

async fn handle_history(
    client: &MimirClient,
    session_id: &mut Option<i64>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let sessions = client.sessions().await?;
    if sessions.is_empty() {
        println!("No conversation history yet.");
        return Ok(());
    }

    let options: Vec<String> = sessions
        .iter()
        .map(|s| {
            let dt = s
                .updated_at
                .split('T')
                .next()
                .unwrap_or(&s.updated_at)
                .to_string();
            let preview = s
                .preview
                .as_ref()
                .map(|p| {
                    let char_count = p.chars().count();
                    if char_count > 60 {
                        format!("{}...", p.chars().take(60).collect::<String>())
                    } else {
                        p.clone()
                    }
                })
                .unwrap_or_else(|| "(no preview)".to_string());
            format!("{} — \"{}\"", dt, preview)
        })
        .collect();

    let selection = inquire::Select::new("Resume conversation:", options)
        .with_starting_cursor(0)
        .raw_prompt()
        .ok();

    let idx = match selection {
        Some(s) => s.index,
        None => return Ok(()),
    };

    let selected = &sessions[idx];
    let sid = selected.session_id;
    let resp = client.session_messages(sid).await?;

    *session_id = Some(sid);

    for msg in resp.messages {
        if msg.role == "system" {
            continue;
        }
        if msg.role == "user" {
            println!("\nYou: {}", msg.content);
        } else if msg.role == "assistant" {
            println!("\nMimir: {}", format_markdown_for_terminal(&msg.content));
        }
    }
    println!();

    Ok(())
}

/// Ensure markdown code fences have blank lines around them for terminal
/// readability.
pub fn format_markdown_for_terminal(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return text.to_string();
    }

    let mut result = Vec::new();
    let mut in_code_block = false;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let prev = i
            .checked_sub(1)
            .and_then(|p| lines.get(p))
            .map(|l| l.trim());
        let next = lines.get(i + 1).map(|l| l.trim());

        if trimmed.starts_with("```") {
            if !in_code_block {
                // Opening fence
                if let Some(p) = prev
                    && !p.is_empty()
                    && !p.starts_with("```")
                    && result.last().map(|l: &String| l.trim()) != Some("")
                {
                    result.push(String::new());
                }
                result.push(line.to_string());
                in_code_block = true;
            } else {
                // Closing fence
                result.push(line.to_string());
                if let Some(n) = next
                    && !n.is_empty()
                    && !n.starts_with("```")
                {
                    result.push(String::new());
                }
                in_code_block = false;
            }
        } else {
            result.push(line.to_string());
        }
    }
    result.join(
        "
",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_markdown_fence_at_start_gets_space_after() {
        let input = "```rust\nlet x = 1;\n```\nHello";
        let out = format_markdown_for_terminal(input);
        assert!(out.contains("```\n\nHello"), "output was: {}", out);
    }

    #[test]
    fn test_format_markdown_fence_in_middle() {
        let input = "Some text\n```\ncode\n```\nMore text";
        let out = format_markdown_for_terminal(input);
        assert!(out.contains("Some text\n\n```"), "output was: {}", out);
        assert!(out.contains("```\n\nMore text"), "output was: {}", out);
    }

    #[test]
    fn test_format_markdown_consecutive_fences_no_extra_space() {
        let input = "```\na\n```\n```\nb\n```";
        let out = format_markdown_for_terminal(input);
        // No blank line between closing and opening fence
        assert!(out.contains("```\n```"), "output was: {}", out);
    }

    #[test]
    fn test_format_markdown_fence_at_end_no_trailing_space() {
        let input = "text\n```\ncode\n```";
        let out = format_markdown_for_terminal(input);
        assert!(out.contains("text\n\n```"), "output was: {}", out);
        assert!(!out.ends_with("\n\n"), "output was: {}", out);
    }

    #[test]
    fn test_format_markdown_empty_input() {
        assert_eq!(format_markdown_for_terminal(""), "");
    }

    #[test]
    fn test_format_markdown_no_fences_unchanged() {
        let input = "Hello world\nHow are you?";
        assert_eq!(format_markdown_for_terminal(input), input);
    }
}
