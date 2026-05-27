//! Interactive chat REPL with persistent history.
//!
//! The daemon owns the session and conversation history; this client is
//! fully stateless except for the optional `session_id` used to resume.
use mimir_api_types::ChatRequest;
use mimir_client::MimirClient;
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;

pub async fn handle_chat(base_url: &str) {
    let client = MimirClient::new(base_url);
    let mut session_id: Option<String> = None;

    let history_path = dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("mimir")
        .join("history.txt");

    if let Some(parent) = history_path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        eprintln!("Warning: failed to create history directory: {}", e);
    }

    let mut editor = match DefaultEditor::new() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Failed to initialize line editor: {}", e);
            std::process::exit(1);
        }
    };

    if history_path.exists()
        && let Err(e) = editor.load_history(&history_path)
    {
        eprintln!("Warning: failed to load history: {}", e);
    }

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
                    editor.add_history_entry(&line).ok();
                    continue;
                }
                if trimmed == "/clear" {
                    session_id = None;
                    println!("Session reset.");
                    editor.add_history_entry(&line).ok();
                    continue;
                }
                if trimmed == "/memory" {
                    match client.memory().await {
                        Ok(content) => println!("{}", content),
                        Err(e) => eprintln!("Failed to load memory: {}", e),
                    }
                    editor.add_history_entry(&line).ok();
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
                                "Memory: {} ({}), {} / {} chars ({:.1}%)",
                                status.memory_path,
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
                    editor.add_history_entry(&line).ok();
                    continue;
                }
                if trimmed == "/history" {
                    match handle_history(&client, &mut session_id).await {
                        Ok(()) => {}
                        Err(e) => eprintln!("History error: {}", e),
                    }
                    editor.add_history_entry(&line).ok();
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

                editor.add_history_entry(&line).ok();
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
            session_id: session_id.clone(),
            message: line,
            model: None,
            personality_preset: None,
            incognito: None,
        };

        match client.chat(req).await {
            Ok(resp) => {
                println!("{}", format_markdown_for_terminal(&resp.response));
                session_id = Some(resp.session_id);
            }
            Err(e) => {
                eprintln!("LLM error: {}", e);
            }
        }
    }

    if let Err(e) = editor.save_history(&history_path) {
        eprintln!("Warning: failed to save history: {}", e);
    }
}

fn print_help() {
    println!("Commands:");
    println!("  /exit    - Exit the REPL");
    println!("  /clear   - Reset the conversation session");
    println!("  /memory  - Show memory.md contents");
    println!("  /status  - Quick health check");
    println!("  /history - Resume a previous conversation");
    println!();
    println!("Multi-line input: end a line with \\ to continue.");
}

async fn handle_history(
    client: &MimirClient,
    session_id: &mut Option<String>,
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
                    if p.len() > 60 {
                        format!("{}...", &p[..60])
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
    let sid = selected.session_id.clone();
    let resp = client.session_messages(&sid).await?;

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
