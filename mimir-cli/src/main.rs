mod ask;
mod chat;
mod cli;
mod commands;
mod init;
mod memory_cmd;
mod skills_permissions_config;
mod start;
mod status;

use clap::Parser;
use cli::Cli;
use commands::{handle_skill_command, handle_tool_command};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        cli::Commands::Tool { command } => handle_tool_command(command).await,
        cli::Commands::Skill { command } => handle_skill_command(command).await,
        cli::Commands::Init => init::handle_init().await,
        cli::Commands::Start => start::handle_start(),
        cli::Commands::Ask {
            query,
            no_stream,
            model,
            verbose,
            incognito,
            personality,
        } => {
            let piped = ask::read_piped_input();
            let query_str = query.join(" ");
            if query_str.trim().is_empty() && piped.is_none() {
                eprintln!("Error: no query provided.");
                std::process::exit(1);
            }
            ask::handle_ask(ask::AskOptions {
                query: query_str,
                no_stream,
                model,
                verbose,
                incognito,
                personality,
                piped_input: piped,
            })
            .await;
        }
        cli::Commands::Chat => chat::handle_chat().await,
        cli::Commands::Status => status::handle_status().await,
        cli::Commands::Memory => memory_cmd::handle_memory().await,
    }
}
