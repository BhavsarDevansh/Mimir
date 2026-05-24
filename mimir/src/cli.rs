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
    /// Skill management commands.
    Skill {
        #[command(subcommand)]
        command: SkillCommands,
    },
    /// Start the Mimir HTTP server (foreground daemon).
    Start,
    /// Stop the Mimir HTTP server.
    Stop,
    /// Initialise Mimir directories and default configuration.
    Init,
    /// Send a single query to the LLM.
    Ask {
        /// The query to send.
        query: Vec<String>,

        /// Disable streaming; wait for the full response.
        #[arg(short = 'n', long)]
        no_stream: bool,

        /// Override the configured LLM model.
        #[arg(short = 'm', long)]
        model: Option<String>,

        /// Print token usage after the response.
        #[arg(short = 'v', long)]
        verbose: bool,

        /// Skip context persistence and memory learning.
        #[arg(long)]
        incognito: bool,

        /// Override the personality preset.
        #[arg(short = 'p', long)]
        personality: Option<String>,
    },
    /// Start an interactive chat REPL.
    Chat,
    /// Display system status and connectivity.
    Status,
    /// Print the contents of memory.md.
    Memory,
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

#[derive(Subcommand)]
pub enum SkillCommands {
    /// List all registered skills.
    List {
        /// Filter by origin (builtin, user, generated).
        #[arg(long)]
        origin: Option<String>,
        /// Filter by tag.
        #[arg(long)]
        tag: Option<String>,
    },
    /// Show the full details of a skill.
    Show { name: String },
    /// Add a user skill from a Markdown file.
    Add { path: std::path::PathBuf },
    /// Delete a user skill.
    Delete { name: String },
    /// Enable a skill (set permission to Auto).
    Enable { name: String },
    /// Disable a skill.
    Disable { name: String },
}
