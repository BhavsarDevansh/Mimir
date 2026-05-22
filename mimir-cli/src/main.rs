mod cli;
mod commands;
mod skills_permissions_config;

use clap::Parser;
use cli::Cli;
use commands::{handle_skill_command, handle_tool_command};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        cli::Commands::Tool { command } => handle_tool_command(command).await,
        cli::Commands::Skill { command } => handle_skill_command(command).await,
    }
}
