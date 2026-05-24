//! Interactive chat REPL with persistent history.
//!
//! The daemon owns the session and conversation history; this client is
//! fully stateless except for the optional `session_id` used to resume.
use mimir_api_types::ChatRequest;
use mimir_client::MimirClient;
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8080";

pub async fn handle_chat() {
    let client = MimirClient::new(DEFAULT_BASE_URL);
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
                    println!("Commands:");
                    println!("  /exit   - Exit the REPL");
                    println!("  /clear  - Reset the conversation session");
                    println!("  /memory - Show memory.md contents");
                    println!("  /status - Quick health check");
                    println!();
                    println!("Multi-line input: end a line with \\ to continue.");
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
                println!("{}", resp.response);
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
