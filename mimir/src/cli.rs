use clap::{ArgGroup, Parser, Subcommand};

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
    /// Knowledge graph commands.
    Kb {
        #[command(subcommand)]
        command: KbCommands,
    },
    /// Connector management commands.
    Connector {
        #[command(subcommand)]
        command: ConnectorCommands,
    },
    /// Personality preset commands.
    Personality {
        #[command(subcommand)]
        command: PersonalityCommands,
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
    Chat {
        /// Override the configured LLM model for this session.
        #[arg(short = 'm', long)]
        model: Option<String>,

        /// Print token usage after each response.
        #[arg(short = 'v', long)]
        verbose: bool,

        /// Skip context persistence and memory learning.
        #[arg(long)]
        incognito: bool,

        /// Override the personality preset.
        #[arg(short = 'p', long)]
        personality: Option<String>,
    },
    /// Display system status and connectivity.
    Status,
    /// Print the live condensed memory block.
    #[command(arg_required_else_help = false)]
    Memory {
        /// Trigger condensation immediately.
        #[arg(long)]
        refresh: bool,
    },
}

#[derive(Subcommand)]
pub enum PersonalityCommands {
    /// List available personality presets (built-in and custom).
    List,
}

#[derive(Subcommand)]
pub enum KbCommands {
    /// Manage knowledge graph categories.
    Category {
        #[command(subcommand)]
        command: CategoryCommands,
    },
    /// Manage knowledge graph optimization.
    #[command(group = ArgGroup::new("action").required(true).args(["status", "run_now"]))]
    Optimization {
        /// Show optimization job status.
        #[arg(long)]
        status: bool,
        /// Trigger optimization immediately.
        #[arg(long)]
        run_now: bool,
        /// Output raw JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Query facts for an entity.
    Query {
        /// Entity name to query.
        entity: String,
        /// Filter by predicate name.
        #[arg(long)]
        predicate: Option<String>,
        /// Minimum confidence threshold.
        #[arg(long)]
        min_confidence: Option<f32>,
        /// Output raw JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Show a single fact by ID.
    Show {
        /// Fact ID.
        fact_id: i32,
        /// Output raw JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Edit a fact's mutable fields.
    #[command(group = ArgGroup::new("edits").required(true).args(["confidence", "valid_from", "valid_until", "object", "status"]))]
    Edit {
        /// Fact ID.
        fact_id: i32,
        /// Update confidence.
        #[arg(long)]
        confidence: Option<f32>,
        /// Update valid-from timestamp.
        #[arg(long)]
        valid_from: Option<String>,
        /// Update valid-until timestamp.
        #[arg(long)]
        valid_until: Option<String>,
        /// Update object literal.
        #[arg(long)]
        object: Option<String>,
        /// Update status (Active, Inferred, Disputed, Corrected, Superseded, Forgotten).
        #[arg(long)]
        status: Option<String>,
        /// Output raw JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Browse the knowledge graph from an entity.
    Browse {
        /// Entity name to start from.
        entity: String,
        /// Graph traversal depth (default 2, max 5).
        #[arg(long)]
        depth: Option<u32>,
        /// Maximum rows to return.
        #[arg(long, default_value = "50")]
        limit: u32,
        /// Rows to skip.
        #[arg(long, default_value = "0")]
        offset: u32,
        /// Output raw JSON instead of a tree.
        #[arg(long)]
        json: bool,
    },
    /// Generate a profile from top-confidence facts.
    Profile {
        /// Entity name (defaults to configured user).
        #[arg(long)]
        entity: Option<String>,
        /// Output raw JSON instead of prose.
        #[arg(long)]
        json: bool,
    },
    /// Query the fact audit log.
    Audit {
        /// Filter by entity name.
        #[arg(long)]
        entity: Option<String>,
        /// Filter by predicate name.
        #[arg(long)]
        predicate: Option<String>,
        /// Filter from datetime (ISO-8601).
        #[arg(long)]
        from: Option<String>,
        /// Filter to datetime (ISO-8601).
        #[arg(long)]
        to: Option<String>,
        /// Filter by change type.
        #[arg(long)]
        change_type: Option<String>,
        /// Output raw JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Forget facts (single, bulk, or full reset).
    Forget {
        /// Single fact ID.
        #[arg(long)]
        fact_id: Option<i32>,
        /// Filter by predicate name.
        #[arg(long)]
        predicate: Option<String>,
        /// Filter by subject entity name.
        #[arg(long)]
        subject: Option<String>,
        /// Filter by entity name (subject or object).
        #[arg(long)]
        entity: Option<String>,
        /// Filter by source connector name.
        #[arg(long)]
        source: Option<String>,
        /// Filter from datetime.
        #[arg(long)]
        from: Option<String>,
        /// Filter to datetime.
        #[arg(long)]
        to: Option<String>,
        /// Forget everything (full reset).
        #[arg(long)]
        all: bool,
        /// Skip confirmation for bulk.
        #[arg(long)]
        yes: bool,
        /// Confirm sensitive predicate deletion.
        #[arg(long)]
        confirm_sensitive: bool,
        /// Archive to trash instead of hard-delete on full reset.
        #[arg(long)]
        archive: bool,
        /// Confirmation phrase for full reset.
        #[arg(long)]
        confirmation_phrase: Option<String>,
    },
    /// Restore facts from trash.
    Restore {
        /// Trash row ID.
        #[arg(long)]
        trash_id: Option<i32>,
        /// Restore everything.
        #[arg(long)]
        all: bool,
    },
    /// List or empty trash.
    Trash {
        /// Empty trash immediately.
        #[arg(long)]
        empty: bool,
        /// Maximum rows to list.
        #[arg(long, default_value = "50")]
        limit: u32,
        /// Rows to skip.
        #[arg(long, default_value = "0")]
        offset: u32,
        /// Output raw JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// List sensitive facts awaiting confirmation.
    Pending {
        /// Output raw JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Confirm a pending sensitive fact.
    Confirm {
        /// Fact ID to confirm.
        fact_id: i32,
        /// Output raw JSON instead of a summary.
        #[arg(long)]
        json: bool,
    },
    /// Reject a pending sensitive fact.
    Reject {
        /// Fact ID to reject.
        fact_id: i32,
        /// Optional reason recorded in the audit log.
        #[arg(long)]
        reason: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum CategoryCommands {
    /// List categories, optionally filtered by parent.
    List {
        /// Filter by parent category ID.
        #[arg(long)]
        parent: Option<i32>,
    },
    /// Show a single category with its children and fact count.
    Show {
        /// Category ID.
        id: i32,
    },
    /// Add a new category.
    Add {
        /// Category ID (e.g., 200, 210).
        id: i32,
        /// Category name.
        name: String,
        /// Parent category ID (omit for top-level).
        #[arg(long)]
        parent: Option<i32>,
        /// One-line description.
        #[arg(long)]
        description: Option<String>,
        /// Memory weight (higher = more important).
        #[arg(long)]
        memory_weight: Option<f32>,
        /// Memory bucket id (1 Identity, 2 Upcoming, 3 Relationships,
        /// 4 Preferences, 5 General; omit for General).
        #[arg(long)]
        memory_bucket_id: Option<i16>,
    },
    /// Delete a category (only if empty).
    Delete {
        /// Category ID.
        id: i32,
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

#[derive(Subcommand)]
pub enum ConnectorCommands {
    /// Register a new connector instance.
    Add {
        /// Connector type (gmail, calendar, photos). Omit together with
        /// `--backend` to run the interactive wizard.
        connector_type: Option<String>,
        /// Backend (run `mimir connector catalog` for the daemon's supported set). Omit together with the type to run the interactive wizard.
        #[arg(long)]
        backend: Option<String>,
        /// Configuration as `key=value` pairs (dotted keys nest, e.g. `auth.kind=app_password`).
        config: Vec<String>,
        /// Full backend configuration as a JSON object (key=value pairs override it).
        #[arg(long)]
        config_json: Option<String>,
        /// Unique slug (defaults to the connector type).
        #[arg(long)]
        slug: Option<String>,
        /// Human-readable display name (defaults to the connector type).
        #[arg(long)]
        name: Option<String>,
        /// App-password credential (skips the interactive prompt).
        #[arg(long, conflicts_with = "token")]
        password: Option<String>,
        /// Read the app-password credential from stdin (piped secrets; skips the interactive prompt).
        #[arg(long, conflicts_with_all = ["password", "token", "token_stdin"])]
        password_stdin: bool,
        /// API-token credential (skips the interactive prompt).
        #[arg(long)]
        token: Option<String>,
        /// Read the API-token credential from stdin (piped secrets; skips the interactive prompt).
        #[arg(long, conflicts_with_all = ["token", "password", "password_stdin"])]
        token_stdin: bool,
        /// Output raw JSON instead of a summary.
        #[arg(long)]
        json: bool,
    },
    /// Ingest credentials for an existing connector (completes an unauthenticated instance, or re-auths after expiry).
    Auth {
        /// Connector slug.
        slug: String,
        /// Configuration as `key=value` pairs (dotted keys nest, e.g. `auth.kind=oauth`). Required to re-run the OAuth PKCE flow.
        config: Vec<String>,
        /// Full backend configuration as a JSON object (key=value pairs override it). Required to re-run the OAuth PKCE flow.
        #[arg(long)]
        config_json: Option<String>,
        /// App-password credential (skips the interactive prompt).
        #[arg(long, conflicts_with = "token")]
        password: Option<String>,
        /// Read the app-password credential from stdin (piped secrets; skips the interactive prompt).
        #[arg(long, conflicts_with_all = ["password", "token", "token_stdin"])]
        password_stdin: bool,
        /// API-token credential (skips the interactive prompt).
        #[arg(long)]
        token: Option<String>,
        /// Read the API-token credential from stdin (piped secrets; skips the interactive prompt).
        #[arg(long, conflicts_with_all = ["token", "password", "password_stdin"])]
        token_stdin: bool,
        /// Output raw JSON instead of a summary.
        #[arg(long)]
        json: bool,
    },
    /// List the connector types and backends the daemon supports.
    Catalog {
        /// Output raw JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// List every registered connector instance.
    List {
        /// Output raw JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Show connector status (all instances, or one by slug).
    Status {
        /// Connector slug (omitting shows every instance).
        slug: Option<String>,
        /// Output raw JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Trigger a manual sync of a connector.
    Sync {
        /// Connector slug.
        slug: String,
        /// Force a full (non-incremental) sync.
        #[arg(long, conflicts_with = "since")]
        full: bool,
        /// Only fetch items newer than this window (e.g. 7d, 12h, 30m, or bare seconds).
        #[arg(long)]
        since: Option<String>,
        /// Output raw JSON instead of a summary.
        #[arg(long)]
        json: bool,
    },
    /// Pause a connector (stop its runner).
    Pause {
        /// Connector slug.
        slug: String,
        /// Output raw JSON instead of a summary.
        #[arg(long)]
        json: bool,
    },
    /// Resume a connector (re-spawn its runner).
    Resume {
        /// Connector slug.
        slug: String,
        /// Output raw JSON instead of a summary.
        #[arg(long)]
        json: bool,
    },
    /// Remove a connector, detaching its provenance (ingested facts survive).
    Remove {
        /// Connector slug.
        slug: String,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Forget a connector: trash its sourced facts (recoverable 30 days), delete its credentials and row.
    Forget {
        /// Connector slug.
        slug: String,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
        /// Output raw JSON instead of a summary.
        #[arg(long)]
        json: bool,
    },
    /// Dispatch a write-back action (e.g. create_event, update_event, delete_event).
    Act {
        /// Connector slug.
        slug: String,
        /// Action kind.
        kind: String,
        /// Inline JSON payload.
        payload: Option<String>,
        /// Read the JSON payload from a file instead of the positional argument.
        #[arg(long, conflicts_with = "payload")]
        json_file: Option<std::path::PathBuf>,
        /// Output raw JSON instead of a summary.
        #[arg(long)]
        json: bool,
    },
}
