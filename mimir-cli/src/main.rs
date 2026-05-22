mod cli;
mod commands;

use clap::Parser;
use cli::Cli;
use commands::handle_tool_command;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        cli::Commands::Tool { command } => handle_tool_command(command).await,
    }
}
