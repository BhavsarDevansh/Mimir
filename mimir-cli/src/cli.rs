use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "mimir")]
#[command(about = "Mimir — persistent personal intelligence")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Tool management commands.
    Tool {
        #[command(subcommand)]
        command: ToolCommands,
    },
}

#[derive(Subcommand)]
pub enum ToolCommands {
    /// List all registered tools.
    List,
    /// Enable a tool (set permission to Auto).
    Enable { name: String },
    /// Disable a tool.
    Disable { name: String },
    /// Set a tool's permission explicitly.
    Permission { name: String, level: String },
}
