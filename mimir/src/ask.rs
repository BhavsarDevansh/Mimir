//! Single-shot LLM query handler.
//!
//! Supports streaming/non-streaming, piped stdin, model/personality overrides,
//! token usage reporting, and incognito mode.

use colored::Colorize;
use is_terminal::IsTerminal;
use std::io::Read;

use futures::StreamExt;
use mimir_api_types::{ChatRequest, StreamItem};

pub struct AskOptions {
    pub query: String,
    pub no_stream: bool,
    pub model: Option<String>,
    pub verbose: bool,
    pub incognito: bool,
    pub personality: Option<String>,
    pub piped_input: Option<String>,
}

pub async fn handle_ask(base_url: &str, opts: AskOptions) {
    let client = crate::cli_util::make_client(base_url);

    let mut message = String::new();
    if let Some(ref piped) = opts.piped_input {
        message.push_str(&format!("[Context from stdin]\n{}\n\n", piped));
    }
    message.push_str(&opts.query);

    let req = ChatRequest {
        session_id: None,
        message,
        model: opts.model,
        personality_preset: opts.personality,
        incognito: Some(opts.incognito),
    };

    if opts.no_stream {
        match client.chat(req).await {
            Ok(resp) => {
                for tc in &resp.tool_calls {
                    eprintln!(
                        "{}",
                        format!("🔧 {} → {}", tc.display_name, tc.result)
                            .dimmed()
                            .italic()
                    );
                }
                println!("{}", resp.response);
                if opts.verbose {
                    eprintln!(
                        "Tokens: {} prompt + {} completion = {} total",
                        resp.usage.prompt_tokens,
                        resp.usage.completion_tokens,
                        resp.usage.total_tokens
                    );
                }
            }
            Err(e) => {
                eprintln!("LLM request failed: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        match client.chat_stream(req).await {
            Ok(mut stream) => {
                let mut full_response = String::new();
                let mut total_usage = mimir_api_types::Usage::default();
                while let Some(item) = stream.next().await {
                    match item {
                        Ok(StreamItem::Text(text)) => {
                            print!("{}", text);
                            use std::io::Write;
                            let _ = std::io::stdout().flush();
                            full_response.push_str(&text);
                        }
                        Ok(StreamItem::Usage(u)) => {
                            total_usage = u;
                        }
                        Ok(StreamItem::SessionId(_)) => {}
                        Ok(StreamItem::ToolCall(info)) => {
                            eprintln!(
                                "{}",
                                format!("🔧 {} → {}", info.display_name, info.result)
                                    .dimmed()
                                    .italic()
                            );
                        }
                        Ok(StreamItem::ToolCallStart(info)) => {
                            eprintln!("{}", format!("🔧 {}…", info.display_name).dimmed().italic());
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
            }
            Err(e) => {
                eprintln!("LLM stream request failed: {}", e);
                std::process::exit(1);
            }
        }
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
