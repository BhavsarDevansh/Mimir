//! Interactive chat REPL with persistent history and context management.
use futures::StreamExt;
use mimir_core::config::Config;
use mimir_core::context::ContextManager;
use mimir_core::llm::LlmClient;
use mimir_core::llm::types::Message;
use mimir_core::memory::MemoryLoader;
use mimir_core::personality::Personality;
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;

pub async fn handle_chat() {
    let config = match Config::load(None) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load config: {}", e);
            std::process::exit(1);
        }
    };

    let personality = Personality::new(&config.personality);

    let mem_path = MemoryLoader::get_memory_path();
    let memory_content = match MemoryLoader::load(&mem_path).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Warning: failed to load memory: {}", e);
            String::new()
        }
    };

    let system_prompt = personality.system_prompt(&memory_content);

    let db_path = config
        .context
        .db_path
        .clone()
        .unwrap_or_else(|| std::path::PathBuf::from("~/.local/share/mimir/context.db"));
    let ctx = match ContextManager::new(&db_path).await {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("Failed to initialize context manager: {}", e);
            std::process::exit(1);
        }
    };
    let mut session_id = match ctx.create_session(&system_prompt).await {
        Ok(id) => id,
        Err(e) => {
            eprintln!("Failed to create session: {}", e);
            std::process::exit(1);
        }
    };

    let history_path = dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("mimir")
        .join("history.txt");

    let mut editor = match DefaultEditor::new() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Failed to initialize line editor: {}", e);
            std::process::exit(1);
        }
    };

    let _ = editor.load_history(&history_path);

    println!("Mimir chat. Type /help for commands, /exit to quit.");
    println!("Press Ctrl+C during input to exit, Ctrl+C during streaming to abort.");

    // Clone llm config before giving it away to the client so we can still
    // reference endpoint/model in the /status handler.
    let llm_endpoint = config.llm.endpoint.clone();
    let llm_model = config.llm.model.clone();
    let config_path_clone = Config::config_path();
    let agent_name = config.agent.name.clone();
    let client = LlmClient::new(config.llm).await;
    let mut conversation: Vec<Message> = Vec::new();

    loop {
        let prompt = format!("{}> ", agent_name);
        let line = match editor.readline(&prompt) {
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
                    println!("Multi-line input: end a line with \\\\ to continue.");
                    editor.add_history_entry(&line).ok();
                    continue;
                }
                if trimmed == "/clear" {
                    conversation.clear();
                    match ctx.create_session(&system_prompt).await {
                        Ok(new_id) => {
                            session_id = new_id;
                            println!("Session reset.");
                        }
                        Err(e) => {
                            eprintln!("Failed to create new session: {}", e);
                        }
                    }
                    editor.add_history_entry(&line).ok();
                    continue;
                }
                if trimmed == "/memory" {
                    match MemoryLoader::load(&mem_path).await {
                        Ok(content) => println!("{}", content),
                        Err(e) => eprintln!("Failed to load memory: {}", e),
                    }
                    editor.add_history_entry(&line).ok();
                    continue;
                }
                if trimmed == "/status" {
                    println!(
                        "Config: {} ({}), LLM: {} @ {}",
                        config_path_clone
                            .as_deref()
                            .map_or("unknown", |p| p.to_str().unwrap_or("?")),
                        if config_path_clone.as_ref().is_some_and(|p| p.exists()) {
                            "ok"
                        } else {
                            "missing"
                        },
                        llm_model,
                        llm_endpoint,
                    );
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

        conversation.push(Message::user(&line));
        let _ = ctx.add_user_message(&session_id, &line).await;

        let mut messages = vec![Message::system(&system_prompt)];
        messages.extend(conversation.clone());

        match client.chat_stream_with_usage(messages).await {
            Ok(mut stream) => {
                let mut full_response = String::new();
                let mut total_usage = mimir_core::llm::types::Usage::default();
                while let Some(item) = stream.next().await {
                    match item {
                        Ok(mimir_core::llm::StreamItem::Text(text)) => {
                            print!("{}", text);
                            use std::io::Write;
                            let _ = std::io::stdout().flush();
                            full_response.push_str(&text);
                        }
                        Ok(mimir_core::llm::StreamItem::Usage(u)) => {
                            total_usage = u;
                        }
                        Err(e) => {
                            eprintln!("\nStream error: {}", e);
                            break;
                        }
                    }
                }
                println!();

                if !full_response.is_empty() {
                    conversation.push(Message::assistant(&full_response));
                    let _ = ctx.add_assistant_message(&session_id, &full_response).await;
                }
                if total_usage.total_tokens > 0 {
                    let _ = ctx
                        .record_usage(
                            &session_id,
                            total_usage.prompt_tokens,
                            total_usage.completion_tokens,
                        )
                        .await;
                }
            }
            Err(e) => {
                eprintln!("LLM error: {}", e);
            }
        }
    }

    let _ = editor.save_history(&history_path);
}
