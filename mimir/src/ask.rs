//! Single-shot LLM query handler.
//!
//! Supports streaming/non-streaming, piped stdin, model/personality overrides,
//! token usage reporting, and incognito mode.

use std::io::Read;

use futures::StreamExt;
use is_terminal::IsTerminal;
use mimir_core::config::Config;
use mimir_core::context::ContextManager;
use mimir_core::llm::LlmClient;
use mimir_core::llm::types::{Message, Usage};
use mimir_core::memory::MemoryLoader;
use mimir_core::personality::Personality;

pub struct AskOptions {
    pub query: String,
    pub no_stream: bool,
    pub model: Option<String>,
    pub verbose: bool,
    pub incognito: bool,
    pub personality: Option<String>,
    pub piped_input: Option<String>,
}

pub async fn handle_ask(opts: AskOptions) {
    let mut config = match Config::load(None) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load config: {}", e);
            std::process::exit(1);
        }
    };

    if let Some(ref model) = opts.model {
        config.llm.model = model.clone();
    }

    let personality_preset = opts
        .personality
        .unwrap_or_else(|| config.personality.preset.clone());
    let personality = Personality::new(&mimir_core::config::PersonalityConfig {
        preset: personality_preset,
    });

    let mem_path = MemoryLoader::get_memory_path();
    let memory_content = match MemoryLoader::load(&mem_path).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Warning: failed to load memory: {}", e);
            String::new()
        }
    };

    let mut user_content = String::new();
    if let Some(ref piped) = opts.piped_input {
        user_content.push_str(&format!("[Context from stdin]\n{}\n\n", piped));
    }
    user_content.push_str(&opts.query);

    let system_prompt = personality.system_prompt(&memory_content);
    let messages = vec![
        Message::system(&system_prompt),
        Message::user(&user_content),
    ];

    let client = LlmClient::new(config.llm.clone()).await;

    // Collect the full response and usage so we can persist them.
    let (response_text, usage) = if opts.no_stream {
        match client.chat(messages).await {
            Ok((response, usage)) => {
                println!("{}", response);
                if opts.verbose {
                    eprintln!(
                        "Tokens: {} prompt + {} completion = {} total",
                        usage.prompt_tokens, usage.completion_tokens, usage.total_tokens
                    );
                }
                (Some(response), Some(usage))
            }
            Err(e) => {
                eprintln!("LLM request failed: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        match client.chat_stream_with_usage(messages).await {
            Ok(mut stream) => {
                let mut full_response = String::new();
                let mut total_usage = Usage::default();
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
                            std::process::exit(1);
                        }
                    }
                }
                println!();
                if opts.verbose {
                    eprintln!(
                        "Tokens: {} prompt + {} completion = {} total",
                        total_usage.prompt_tokens,
                        total_usage.completion_tokens,
                        total_usage.total_tokens
                    );
                }
                let text = if full_response.is_empty() {
                    None
                } else {
                    Some(full_response)
                };
                let usage = if total_usage.total_tokens > 0 {
                    Some(total_usage)
                } else {
                    None
                };
                (text, usage)
            }
            Err(e) => {
                eprintln!("LLM stream request failed: {}", e);
                std::process::exit(1);
            }
        }
    };

    // Persist the interaction unless incognito was requested.
    if !opts.incognito {
        let db_path = config.context.db_path.clone().unwrap_or_else(|| {
            dirs::data_dir()
                .map(|d| d.join("mimir").join("context.db"))
                .unwrap_or_else(|| std::path::PathBuf::from("/tmp/mimir/context.db"))
        });
        let ctx = match ContextManager::new(&db_path).await {
            Ok(ctx) => ctx,
            Err(e) => {
                eprintln!("Warning: failed to open context database: {}", e);
                return;
            }
        };
        let sid = match ctx.create_session(&system_prompt).await {
            Ok(sid) => sid,
            Err(e) => {
                eprintln!("Warning: failed to create context session: {}", e);
                return;
            }
        };
        if let Err(e) = ctx.add_user_message(&sid, &user_content).await {
            eprintln!("Warning: failed to persist user message: {}", e);
        }
        if let Some(ref text) = response_text
            && let Err(e) = ctx.add_assistant_message(&sid, text).await
        {
            eprintln!("Warning: failed to persist assistant message: {}", e);
        }
        if let Some(ref usage) = usage
            && let Err(e) = ctx
                .record_usage(&sid, usage.prompt_tokens, usage.completion_tokens)
                .await
        {
            eprintln!("Warning: failed to record usage: {}", e);
        }
        // Don't delete the session — let it persist for future context.
    }
}

pub fn read_piped_input() -> Option<String> {
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return None;
    }

    let mut buffer = String::new();
    match stdin.lock().read_to_string(&mut buffer) {
        Ok(_) if buffer.trim().is_empty() => None,
        Ok(_) => Some(buffer.trim_end().to_string()),
        Err(_) => None,
    }
}
