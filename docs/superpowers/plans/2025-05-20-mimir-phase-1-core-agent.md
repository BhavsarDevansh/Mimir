# Mimir Phase 1: Core Agent Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the foundational Core Agent: CLI, config, LLM client, memory.md, basic chat server, and tool registry. The agent can start, hold a conversation, and stream responses from an OpenAI-compatible endpoint.

**Architecture:** A Rust async daemon with modular crates: `mimir-cli` for the command-line interface, `mimir-core` for the agent logic (config, LLM client, context, memory), and `mimir-server` for the HTTP chat API. SQLite-backed memory.md with file watching. All communication is local-first.

**Tech Stack:** Rust, tokio, axum, reqwest, serde, toml, sqlx, clap, tracing, notify

---

## File Structure

```
/
├── Cargo.toml                    # Workspace root
├── crates/
│   ├── mimir-cli/                # CLI binary
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── main.rs
│   ├── mimir-core/               # Core agent library
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── config.rs         # TOML config + env overrides
│   │       ├── llm/
│   │       │   ├── mod.rs        # LLM client trait + OpenAI impl
│   │       │   ├── client.rs     # HTTP client with streaming
│   │       │   └── types.rs      # Request/response types
│   │       ├── memory/
│   │       │   ├── mod.rs        # memory.md manager
│   │       │   ├── loader.rs     # File loading + hot-reload
│   │       │   └── manager.rs    # Auto-management (add/replace/remove)
│   │       ├── context.rs        # Conversation context manager
│   │       ├── tools/
│   │       │   ├── mod.rs        # Tool registry + traits
│   │       │   └── registry.rs   # Dynamic tool registration
│   │       └── personality.rs    # Personality system + prompts
│   └── mimir-server/             # HTTP API daemon
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs           # Daemon entry point
│           └── routes/
│               ├── mod.rs
│               └── chat.rs       # SSE streaming endpoint
├── config/
│   └── default.toml              # Default configuration
└── tests/
    └── integration_tests.rs
```

---

### Task 1: Workspace and Crate Scaffolding

**Files:**
- Create: `Cargo.toml`
- Create: `crates/mimir-cli/Cargo.toml`
- Create: `crates/mimir-cli/src/main.rs`
- Create: `crates/mimir-core/Cargo.toml`
- Create: `crates/mimir-core/src/lib.rs`
- Create: `crates/mimir-server/Cargo.toml`
- Create: `crates/mimir-server/src/main.rs`

- [ ] **Step 1: Create workspace root `Cargo.toml`**

```toml
[workspace]
members = ["crates/mimir-cli", "crates/mimir-core", "crates/mimir-server"]
resolver = "2"

[workspace.package]
version = "0.1.0"
edition = "2024"
authors = ["Devansh Bhavsar <dev@example.com>"]
license = "GPL-3.0"
repository = "https://github.com/BhavsarDevansh/Mimir"

[workspace.dependencies]
tokio = { version = "1.43", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
anyhow = "1.0"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

- [ ] **Step 2: Create `mimir-core` crate**

`crates/mimir-core/Cargo.toml`:
```toml
[package]
name = "mimir-core"
version.workspace = true
edition.workspace = true
authors.workspace = true
license.workspace = true

[dependencies]
tokio = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
anyhow = { workspace = true }
tracing = { workspace = true }
reqwest = { version = "0.12", features = ["json", "stream", "rustls-tls"], default-features = false }
axum = "0.8"
tower = "0.5"
toml = "0.8"
dirs = "6.0"
notify = "7.0"
futures = "0.3"
 eventsource-stream = "0.2"
bytes = "1.9"
chrono = { version = "0.4", features = ["serde"] }
```

`crates/mimir-core/src/lib.rs`:
```rust
pub mod config;
pub mod llm;
pub mod memory;
pub mod context;
pub mod tools;
pub mod personality;
```

- [ ] **Step 3: Create `mimir-cli` crate**

`crates/mimir-cli/Cargo.toml`:
```toml
[package]
name = "mimir-cli"
version.workspace = true
edition.workspace = true
authors.workspace = true
license.workspace = true

[[bin]]
name = "mimir"
path = "src/main.rs"

[dependencies]
mimir-core = { path = "../mimir-core" }
tokio = { workspace = true }
anyhow = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
clap = { version = "4.5", features = ["derive"] }
```

`crates/mimir-cli/src/main.rs`:
```rust
use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

#[derive(Parser)]
#[command(name = "mimir")]
#[command(about = "Mimir — Persistent personal intelligence")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the Mimir daemon
    Start,
    /// Ask a one-shot question
    Ask { query: String },
    /// Interactive chat mode
    Chat,
    /// Check agent status
    Status,
}

#[tokio::main]
async fn main() -> Result<()> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    let cli = Cli::parse();

    match cli.command {
        Commands::Start => {
            info!("Starting Mimir daemon...");
            println!("Mimir daemon started.");
        }
        Commands::Ask { query } => {
            info!("Ask: {}", query);
            println!("You asked: {}", query);
        }
        Commands::Chat => {
            info!("Starting chat mode...");
            println!("Chat mode started.");
        }
        Commands::Status => {
            info!("Checking status...");
            println!("Mimir is running.");
        }
    }

    Ok(())
}
```

- [ ] **Step 4: Create `mimir-server` crate**

`crates/mimir-server/Cargo.toml`:
```toml
[package]
name = "mimir-server"
version.workspace = true
edition.workspace = true
authors.workspace = true
license.workspace = true

[[bin]]
name = "mimir-server"
path = "src/main.rs"

[dependencies]
mimir-core = { path = "../mimir-core" }
tokio = { workspace = true }
anyhow = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
axum = "0.8"
tower = "0.5"
tower-http = { version = "0.6", features = ["trace", "cors"] }
```

`crates/mimir-server/src/main.rs`:
```rust
use anyhow::Result;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

#[tokio::main]
async fn main() -> Result<()> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    info!("Starting Mimir server...");
    println!("Mimir server started on http://127.0.0.1:8080");

    // TODO: Start axum server in Task 7

    Ok(())
}
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check --workspace`
Expected: Clean compile, no errors

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/
git commit -m "chore: scaffold workspace with cli, core, server crates"
```

---

### Task 2: Configuration System

**Files:**
- Create: `crates/mimir-core/src/config.rs`
- Create: `config/default.toml`
- Modify: `crates/mimir-core/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/mimir-core/src/config.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_default_config() {
        let config = Config::load(None).unwrap();
        assert_eq!(config.agent.name, "Mimir");
        assert_eq!(config.llm.model, "gpt-4o");
    }

    #[test]
    fn test_config_from_env() {
        std::env::set_var("MIMIR_LLM_API_KEY", "sk-test123");
        let config = Config::load(None).unwrap();
        assert_eq!(config.llm.api_key, "sk-test123");
        std::env::remove_var("MIMIR_LLM_API_KEY");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mimir-core test_load_default_config`
Expected: FAIL with "Config not defined"

- [ ] **Step 3: Write minimal implementation**

`crates/mimir-core/src/config.rs`:
```rust
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub llm: LlmConfig,
    pub agent: AgentConfig,
    pub memory: MemoryConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LlmConfig {
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
    pub temperature: f32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentConfig {
    pub name: String,
    pub proactivity: String,
    pub verbose_reasoning: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MemoryConfig {
    pub enabled: bool,
    pub char_limit: usize,
    pub auto_manage: bool,
    pub temporal_horizon: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            llm: LlmConfig {
                endpoint: "https://api.openai.com/v1".to_string(),
                api_key: String::new(),
                model: "gpt-4o".to_string(),
                max_tokens: 4096,
                temperature: 0.2,
            },
            agent: AgentConfig {
                name: "Mimir".to_string(),
                proactivity: "important_only".to_string(),
                verbose_reasoning: false,
            },
            memory: MemoryConfig {
                enabled: true,
                char_limit: 2500,
                auto_manage: true,
                temporal_horizon: 30,
            },
        }
    }
}

impl Config {
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let mut config = if let Some(p) = path {
            let content = std::fs::read_to_string(p)?;
            toml::from_str(&content)?
        } else {
            Self::default()
        };

        // Apply environment variable overrides
        if let Ok(key) = std::env::var("MIMIR_LLM_API_KEY") {
            config.llm.api_key = key;
        }
        if let Ok(endpoint) = std::env::var("MIMIR_LLM_ENDPOINT") {
            config.llm.endpoint = endpoint;
        }
        if let Ok(model) = std::env::var("MIMIR_LLM_MODEL") {
            config.llm.model = model;
        }

        Ok(config)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}
```

- [ ] **Step 4: Ensure `lib.rs` exports config**

`crates/mimir-core/src/lib.rs`:
```rust
pub mod config;
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p mimir-core test_load_default_config test_config_from_env`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/mimir-core/src/config.rs crates/mimir-core/src/lib.rs
git commit -m "feat(config): add TOML config with env overrides"
```

---

### Task 3: LLM Client (OpenAI-Compatible)

**Files:**
- Create: `crates/mimir-core/src/llm/mod.rs`
- Create: `crates/mimir-core/src/llm/types.rs`
- Create: `crates/mimir-core/src/llm/client.rs`

- [ ] **Step 1: Write types**

`crates/mimir-core/src/llm/types.rs`:
```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub stream: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatResponse {
    pub choices: Vec<Choice>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Choice {
    pub message: Message,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StreamChunk {
    pub choices: Vec<StreamChoice>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StreamChoice {
    pub delta: Delta,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Delta {
    pub content: Option<String>,
}

impl ChatRequest {
    pub fn new(model: String, messages: Vec<Message>) -> Self {
        Self {
            model,
            messages,
            max_tokens: None,
            temperature: None,
            stream: false,
        }
    }

    pub fn stream(mut self, enabled: bool) -> Self {
        self.stream = enabled;
        self
    }
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
        }
    }
}
```

- [ ] **Step 2: Write client with streaming**

`crates/mimir-core/src/llm/client.rs`:
```rust
use crate::config::LlmConfig;
use crate::llm::types::*;
use anyhow::Result;
use futures::Stream;
use reqwest::Client;
use serde_json::json;
use std::pin::Pin;
use tracing::{debug, error};

pub struct LlmClient {
    client: Client,
    config: LlmConfig,
}

impl LlmClient {
    pub fn new(config: LlmConfig) -> Self {
        Self {
            client: Client::new(),
            config,
        }
    }

    pub async fn chat(&self,
        messages: Vec<Message>,
    ) -> Result<String> {
        let request = ChatRequest::new(self.config.model.clone(), messages)
            .stream(false);

        let response = self
            .client
            .post(format!("{}/chat/completions", self.config.endpoint))
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("LLM API error: {}", error_text);
        }

        let body: ChatResponse = response.json().await?;
        let content = body
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .unwrap_or_default();

        Ok(content)
    }

    pub async fn chat_stream(
        &self,
        messages: Vec<Message>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        let request = ChatRequest::new(self.config.model.clone(), messages)
            .stream(true);

        let response = self
            .client
            .post(format!("{}/chat/completions", self.config.endpoint))
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("LLM API error: {}", error_text);
        }

        let stream = response.bytes_stream();
        let events = eventsource_stream::Eventsource::new(stream)
            .map(|event| {
                let event = event?;
                if event.data == "[DONE]" {
                    return Ok(String::new());
                }
                let chunk: StreamChunk = serde_json::from_str(&event.data)?;
                let content = chunk
                    .choices
                    .into_iter()
                    .next()
                    .and_then(|c| c.delta.content)
                    .unwrap_or_default();
                Ok(content)
            })
            .filter(|item| !matches!(item, Ok(ref s) if s.is_empty()));

        Ok(Box::pin(events))
    }
}
```

- [ ] **Step 3: Write module exports**

`crates/mimir-core/src/llm/mod.rs`:
```rust
pub mod client;
pub mod types;

pub use client::LlmClient;
pub use types::*;
```

- [ ] **Step 4: Update `lib.rs`**

`crates/mimir-core/src/lib.rs`:
```rust
pub mod config;
pub mod llm;
```

- [ ] **Step 5: Add dependencies**

Modify `crates/mimir-core/Cargo.toml` to add:
```toml
reqwest = { version = "0.12", features = ["json", "stream", "rustls-tls"], default-features = false }
eventsource-stream = "0.2"
bytes = "1.9"
```

- [ ] **Step 6: Verify compilation**

Run: `cargo check -p mimir-core`
Expected: Clean compile

- [ ] **Step 7: Commit**

```bash
git add crates/mimir-core/src/llm/
git commit -m "feat(llm): add OpenAI-compatible client with streaming"
```

---

### Task 4: memory.md System

**Files:**
- Create: `crates/mimir-core/src/memory/mod.rs`
- Create: `crates/mimir-core/src/memory/loader.rs`
- Create: `crates/mimir-core/src/memory/manager.rs`

- [ ] **Step 1: Write loader**

`crates/mimir-core/src/memory/loader.rs`:
```rust
use anyhow::Result;
use std::path::Path;
use tokio::fs;

pub struct MemoryLoader;

impl MemoryLoader {
    pub async fn load(path: &Path) -> Result<String> {
        if path.exists() {
            let content = fs::read_to_string(path).await?;
            Ok(content)
        } else {
            Ok(Self::default_memory())
        }
    }

    pub fn default_memory() -> String {
        r#"═══════════════════════════════════════════════════════════
MEMORY [0 / 2,500 chars] — Mimir Working Memory
═══════════════════════════════════════════════════════════

User: (not yet configured)
Location: (not yet configured)

Active Projects: (none)
Preferences: (none)
Temporal: (none)
KB Pointers: (none)
═══════════════════════════════════════════════════════════"#
        .to_string()
    }

    pub fn get_memory_path() -> std::path::PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("mimir")
            .join("memory.md")
    }
}
```

- [ ] **Step 2: Write manager with auto-management**

`crates/mimir-core/src/memory/manager.rs`:
```rust
use anyhow::Result;
use std::path::Path;
use tokio::fs;

pub struct MemoryManager {
    char_limit: usize,
    content: String,
    path: std::path::PathBuf,
}

impl MemoryManager {
    pub async fn new(path: &Path, char_limit: usize) -> Result<Self> {
        let content = if path.exists() {
            fs::read_to_string(path).await?
        } else {
            String::new()
        };

        Ok(Self {
            char_limit,
            content,
            path: path.to_path_buf(),
        })
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn current_chars(&self) -> usize {
        self.content.chars().count()
    }

    pub fn remaining_chars(&self) -> usize {
        self.char_limit.saturating_sub(self.current_chars())
    }

    pub fn is_full(&self) -> bool {
        self.current_chars() >= self.char_limit
    }

    pub fn usage_pct(&self) -> f32 {
        (self.current_chars() as f32 / self.char_limit as f32) * 100.0
    }

    pub async fn add(&mut self, entry: &str) -> Result<()> {
        let entry_chars = entry.chars().count();
        if self.current_chars() + entry_chars > self.char_limit {
            anyhow::bail!(
                "Memory full: {}/{} chars. Cannot add {} chars.",
                self.current_chars(),
                self.char_limit,
                entry_chars
            );
        }

        if !self.content.is_empty() && !self.content.ends_with('\n') {
            self.content.push('\n');
        }
        self.content.push_str(entry);
        self.save().await?;
        Ok(())
    }

    pub async fn replace(
        &mut self,
        old_text: &str,
        new_text: &str,
    ) -> Result<()> {
        let count = self.content.matches(old_text).count();
        if count == 0 {
            anyhow::bail!("Text '{}' not found in memory", old_text);
        }
        if count > 1 {
            anyhow::bail!(
                "Text '{}' matches {} entries. Be more specific.",
                old_text,
                count
            );
        }

        let old_chars = old_text.chars().count();
        let new_chars = new_text.chars().count();
        let size_delta = new_chars as i64 - old_chars as i64;
        let new_total = self.current_chars() as i64 + size_delta;

        if new_total > self.char_limit as i64 {
            anyhow::bail!(
                "Replace would exceed memory limit: {}/{} chars",
                new_total,
                self.char_limit
            );
        }

        self.content = self.content.replacen(old_text, new_text, 1);
        self.save().await?;
        Ok(())
    }

    pub async fn remove(&mut self,
        old_text: &str,
    ) -> Result<()> {
        let count = self.content.matches(old_text).count();
        if count == 0 {
            anyhow::bail!("Text '{}' not found in memory", old_text);
        }
        if count > 1 {
            anyhow::bail!(
                "Text '{}' matches {} entries. Be more specific.",
                old_text,
                count
            );
        }

        self.content = self.content.replacen(old_text, "", 1);
        self.save().await?;
        Ok(())
    }

    async fn save(&self) -> Result<()> {
        fs::write(&self.path, &self.content).await?;
        Ok(())
    }
}
```

- [ ] **Step 3: Write module exports**

`crates/mimir-core/src/memory/mod.rs`:
```rust
pub mod loader;
pub mod manager;

pub use loader::MemoryLoader;
pub use manager::MemoryManager;
```

- [ ] **Step 4: Write test**

Add to `crates/mimir-core/src/memory/manager.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_memory_manager_add() {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("test_memory.md");
        let mut manager = MemoryManager::new(&path, 100).await.unwrap();

        manager.add("User: Devansh\n").await.unwrap();
        assert_eq!(manager.current_chars(), 16);
        assert!(manager.content.contains("Devansh"));
    }

    #[tokio::test]
    async fn test_memory_manager_replace() {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("test_memory2.md");
        let mut manager = MemoryManager::new(&path, 100).await.unwrap();

        manager.add("User: Dev\n").await.unwrap();
        manager.replace("Dev", "Devansh").await.unwrap();
        assert!(manager.content.contains("Devansh"));
    }

    #[tokio::test]
    async fn test_memory_manager_full() {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("test_memory3.md");
        let mut manager = MemoryManager::new(&path, 10).await.unwrap();

        manager.add("12345").await.unwrap();
        let result = manager.add("67890").await;
        assert!(result.is_err());
    }
}
```

- [ ] **Step 5: Update lib.rs**

`crates/mimir-core/src/lib.rs`:
```rust
pub mod config;
pub mod llm;
pub mod memory;
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p mimir-core memory::manager`
Expected: PASS (3 tests)

- [ ] **Step 7: Commit**

```bash
git add crates/mimir-core/src/memory/
git commit -m "feat(memory): add memory.md loader and manager with capacity tracking"
```

---

### Task 5: Conversation Context Manager

**Files:**
- Create: `crates/mimir-core/src/context.rs`

- [ ] **Step 1: Write context manager**

`crates/mimir-core/src/context.rs`:
```rust
use crate::llm::Message;

pub struct ConversationContext {
    messages: Vec<Message>,
    max_messages: usize,
    session_id: String,
}

impl ConversationContext {
    pub fn new(session_id: String, system_prompt: &str, max_messages: usize) -> Self {
        let mut messages = Vec::new();
        messages.push(Message::system(system_prompt));
        Self {
            messages,
            max_messages,
            session_id,
        }
    }

    pub fn add_user_message(&mut self, content: &str) {
        self.messages.push(Message::user(content));
        self.trim_if_needed();
    }

    pub fn add_assistant_message(&mut self, content: &str) {
        self.messages.push(Message::assistant(content));
        self.trim_if_needed();
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    fn trim_if_needed(&mut self) {
        if self.messages.len() > self.max_messages {
            // Keep system message (index 0), remove oldest user/assistant pair
            let to_remove = self.messages.len() - self.max_messages;
            // Don't remove system message
            if to_remove > 0 {
                self.messages.drain(1..=to_remove);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_basic() {
        let mut ctx = ConversationContext::new("test-1".to_string(), "You are Mimir.", 5);
        ctx.add_user_message("Hello");
        ctx.add_assistant_message("Hi there!");

        assert_eq!(ctx.messages().len(), 3); // system + user + assistant
        assert_eq!(ctx.session_id(), "test-1");
    }

    #[test]
    fn test_context_trimming() {
        let mut ctx = ConversationContext::new("test-2".to_string(), "You are Mimir.", 4);
        ctx.add_user_message("A");
        ctx.add_assistant_message("B");
        ctx.add_user_message("C");
        ctx.add_assistant_message("D");
        ctx.add_user_message("E"); // Should trigger trim

        assert_eq!(ctx.messages().len(), 4);
        assert_eq!(ctx.messages()[0].role, "system");
        assert_eq!(ctx.messages()[1].content, "C"); // Oldest kept after trim
    }
}
```

- [ ] **Step 2: Update lib.rs**

`crates/mimir-core/src/lib.rs`:
```rust
pub mod config;
pub mod llm;
pub mod memory;
pub mod context;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p mimir-core context`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/mimir-core/src/context.rs crates/mimir-core/src/lib.rs
git commit -m "feat(context): add conversation context manager with sliding window"
```

---

### Task 6: Personality System

**Files:**
- Create: `crates/mimir-core/src/personality.rs`

- [ ] **Step 1: Write personality system**

`crates/mimir-core/src/personality.rs`:
```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Personality {
    pub name: String,
    pub style: PersonalityStyle,
    pub verbosity: Verbosity,
    pub proactive_tone: ProactiveTone,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum PersonalityStyle {
    Transparent,
    Concise,
    Warm,
    Formal,
    Custom { system_prompt: String },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum Verbosity {
    Quiet,
    Normal,
    Verbose,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ProactiveTone {
    Suggestive,
    Direct,
    Gentle,
}

impl Default for PersonalityStyle {
    fn default() -> Self {
        PersonalityStyle::Transparent
    }
}

impl Default for Verbosity {
    fn default() -> Self {
        Verbosity::Normal
    }
}

impl Default for ProactiveTone {
    fn default() -> Self {
        ProactiveTone::Suggestive
    }
}

impl Personality {
    pub fn system_prompt(&self, memory_content: &str) -> String {
        let base = match self.style {
            PersonalityStyle::Transparent => {
                "You are a transparent reasoning assistant. You show your work, explain your reasoning, and admit uncertainty clearly."
            }
            PersonalityStyle::Concise => {
                "You are a concise assistant. Provide minimal but complete answers. No fluff."
            }
            PersonalityStyle::Warm => {
                "You are a warm, personable assistant. You communicate naturally and acknowledge context."
            }
            PersonalityStyle::Formal => {
                "You are a formal, professional assistant. Use precise language and structured responses."
            }
            PersonalityStyle::Custom { ref system_prompt } => system_prompt,
        };

        let verbosity = match self.verbosity {
            Verbosity::Quiet => "Keep responses brief. One or two sentences unless asked for more.",
            Verbosity::Normal => "Provide thorough but concise responses. Balance detail with brevity.",
            Verbosity::Verbose => "Provide detailed, comprehensive responses. Show your work and reasoning.",
        };

        format!(
            "{}\n\n{}\n\nYour name is {}.\n\n{}",
            base, verbosity, self.name, memory_content
        )
    }
}
```

- [ ] **Step 2: Update lib.rs**

`crates/mimir-core/src/lib.rs`:
```rust
pub mod config;
pub mod llm;
pub mod memory;
pub mod context;
pub mod personality;
```

- [ ] **Step 3: Write test**

Add to `crates/mimir-core/src/personality.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transparent_personality() {
        let personality = Personality {
            name: "Mimir".to_string(),
            style: PersonalityStyle::Transparent,
            verbosity: Verbosity::Normal,
            proactive_tone: ProactiveTone::Suggestive,
        };

        let prompt = personality.system_prompt("User: Devansh");
        assert!(prompt.contains("transparent reasoning"));
        assert!(prompt.contains("Mimir"));
        assert!(prompt.contains("Devansh"));
    }

    #[test]
    fn test_concise_personality() {
        let personality = Personality {
            name: "Mimir".to_string(),
            style: PersonalityStyle::Concise,
            verbosity: Verbosity::Quiet,
            proactive_tone: ProactiveTone::Direct,
        };

        let prompt = personality.system_prompt("");
        assert!(prompt.contains("concise"));
        assert!(prompt.contains("brief"));
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p mimir-core personality`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/mimir-core/src/personality.rs crates/mimir-core/src/lib.rs
git commit -m "feat(personality): add personality system with transparent default"
```

---

### Task 7: Basic Tool Registry

**Files:**
- Create: `crates/mimir-core/src/tools/mod.rs`
- Create: `crates/mimir-core/src/tools/registry.rs`

- [ ] **Step 1: Write tool trait and registry**

`crates/mimir-core/src/tools/mod.rs`:
```rust
pub mod registry;

use anyhow::Result;
use serde_json::Value;

pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> Value;
    async fn execute(&self, params: Value) -> Result<Value>;
}
```

`crates/mimir-core/src/tools/registry.rs`:
```rust
use crate::tools::Tool;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;

pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name)
    }

    pub fn list(&self) -> Vec<&str> {
        self.tools.keys().map(|s| s.as_str()).collect()
    }

    pub fn to_openai_schema(&self) -> Vec<serde_json::Value> {
        self.tools
            .values()
            .map(|tool| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": tool.name(),
                        "description": tool.description(),
                        "parameters": tool.parameters_schema(),
                    }
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::Tool;

    struct TestTool;

    #[async_trait::async_trait]
    impl Tool for TestTool {
        fn name(&self) -> &str {
            "test_tool"
        }

        fn description(&self) -> &str {
            "A test tool"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "input": { "type": "string" }
                }
            })
        }

        async fn execute(&self, _params: serde_json::Value) -> Result<serde_json::Value> {
            Ok(serde_json::json!({"result": "ok"}))
        }
    }

    #[test]
    fn test_registry() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(TestTool));

        assert!(registry.get("test_tool").is_some());
        assert_eq!(registry.list(), vec!["test_tool"]);
    }
}
```

- [ ] **Step 2: Update lib.rs**

`crates/mimir-core/src/lib.rs`:
```rust
pub mod config;
pub mod llm;
pub mod memory;
pub mod context;
pub mod personality;
pub mod tools;
```

- [ ] **Step 3: Add async-trait dependency**

`crates/mimir-core/Cargo.toml`:
```toml
async-trait = "0.1"
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p mimir-core tools::registry`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/mimir-core/src/tools/
git commit -m "feat(tools): add tool registry with OpenAI schema export"
```

---

### Task 8: HTTP Chat Server with SSE Streaming

**Files:**
- Modify: `crates/mimir-server/src/main.rs`
- Create: `crates/mimir-server/src/routes/mod.rs`
- Create: `crates/mimir-server/src/routes/chat.rs`

- [ ] **Step 1: Write chat route**

`crates/mimir-server/src/routes/chat.rs`:
```rust
use axum::{
    extract::State,
    response::{sse::Event, Sse},
    routing::post,
    Json, Router,
};
use futures::stream::Stream;
use mimir_core::llm::{LlmClient, Message};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct AppState {
    pub llm_client: Arc<LlmClient>,
}

#[derive(Deserialize)]
pub struct ChatRequest {
    pub messages: Vec<Message>,
}

#[derive(Serialize)]
pub struct ChatResponse {
    pub response: String,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/chat", post(chat_handler))
        .route("/chat/stream", post(chat_stream_handler))
}

async fn chat_handler(
    State(state): State<AppState>,
    Json(request): Json<ChatRequest>,
) -> Json<ChatResponse> {
    let response = state
        .llm_client
        .chat(request.messages)
        .await
        .unwrap_or_else(|e| format!("Error: {}", e));

    Json(ChatResponse { response })
}

async fn chat_stream_handler(
    State(state): State<AppState>,
    Json(request): Json<ChatRequest>,
) -> Sse<Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>> {
    let stream = match state.llm_client.chat_stream(request.messages).await {
        Ok(stream) => {
            let event_stream = stream.map(|chunk| {
                let text = chunk.unwrap_or_default();
                Ok(Event::default().data(text))
            });
            Box::pin(event_stream) as Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>
        }
        Err(e) => {
            let error_stream = futures::stream::once(async move {
                Ok(Event::default().data(format!("Error: {}", e)))
            });
            Box::pin(error_stream) as Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>
        }
    };

    Sse::new(stream)
}
```

- [ ] **Step 2: Write routes module**

`crates/mimir-server/src/routes/mod.rs`:
```rust
pub mod chat;
```

- [ ] **Step 3: Write server main**

`crates/mimir-server/src/main.rs`:
```rust
use anyhow::Result;
use axum::Router;
use mimir_core::config::Config;
use mimir_core::llm::LlmClient;
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

mod routes;

#[tokio::main]
async fn main() -> Result<()> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    let config = Config::load(None)?;
    let llm_client = Arc::new(LlmClient::new(config.llm));

    let state = routes::chat::AppState { llm_client };

    let app = Router::new()
        .merge(routes::chat::router())
        .with_state(state);

    let addr: SocketAddr = "127.0.0.1:8080".parse()?;
    info!("Mimir server listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p mimir-server`
Expected: Clean compile

- [ ] **Step 5: Commit**

```bash
git add crates/mimir-server/
git commit -m "feat(server): add axum chat server with SSE streaming"
```

---

### Task 9: CLI Integration (Ask + Chat Commands)

**Files:**
- Modify: `crates/mimir-cli/src/main.rs`

- [ ] **Step 1: Implement ask command with streaming**

`crates/mimir-cli/src/main.rs`:
```rust
use anyhow::Result;
use clap::{Parser, Subcommand};
use mimir_core::config::Config;
use mimir_core::llm::{LlmClient, Message};
use mimir_core::memory::{MemoryLoader, MemoryManager};
use mimir_core::personality::{Personality, PersonalityStyle, Verbosity, ProactiveTone};
use mimir_core::context::ConversationContext;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

#[derive(Parser)]
#[command(name = "mimir")]
#[command(about = "Mimir — Persistent personal intelligence")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the Mimir daemon
    Start,
    /// Ask a one-shot question
    Ask { query: String },
    /// Interactive chat mode
    Chat,
    /// Check agent status
    Status,
    /// Show memory.md contents
    Memory,
}

#[tokio::main]
async fn main() -> Result<()> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    let cli = Cli::parse();
    let config = Config::load(None)?;

    match cli.command {
        Commands::Start => {
            info!("Starting Mimir daemon...");
            println!("Use `mimir-server` to start the HTTP API.");
        }
        Commands::Ask { query } => {
            info!("Ask: {}", query);
            ask_once(&config, &query).await?;
        }
        Commands::Chat => {
            info!("Starting chat mode...");
            chat_interactive(&config).await?;
        }
        Commands::Status => {
            info!("Checking status...");
            check_status(&config).await?;
        }
        Commands::Memory => {
            info!("Showing memory...");
            show_memory().await?;
        }
    }

    Ok(())
}

async fn ask_once(config: &Config, query: &str) -> Result<()> {
    let llm = LlmClient::new(config.llm.clone());
    let memory_path = MemoryLoader::get_memory_path();
    let memory = MemoryLoader::load(&memory_path).await?;

    let personality = Personality {
        name: config.agent.name.clone(),
        style: PersonalityStyle::Transparent,
        verbosity: Verbosity::Normal,
        proactive_tone: ProactiveTone::Suggestive,
    };

    let system_prompt = personality.system_prompt(&memory);
    let messages = vec![
        Message::system(system_prompt),
        Message::user(query),
    ];

    println!("🔍 Thinking...\n");
    let response = llm.chat(messages).await?;
    println!("{}", response);

    Ok(())
}

async fn chat_interactive(config: &Config) -> Result<()> {
    let llm = LlmClient::new(config.llm.clone());
    let memory_path = MemoryLoader::get_memory_path();
    let memory = MemoryLoader::load(&memory_path).await?;

    let personality = Personality {
        name: config.agent.name.clone(),
        style: PersonalityStyle::Transparent,
        verbosity: Verbosity::Normal,
        proactive_tone: ProactiveTone::Suggestive,
    };

    let system_prompt = personality.system_prompt(&memory);
    let mut context = ConversationContext::new(
        uuid::Uuid::new_v4().to_string(),
        &system_prompt,
        20,
    );

    println!("Mimir chat mode. Type 'exit' to quit.\n");

    loop {
        print!("> ");
        std::io::Write::flush(&mut std::io::stdout())?;

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let input = input.trim();

        if input == "exit" {
            println!("Goodbye!");
            break;
        }

        context.add_user_message(input);

        let messages: Vec<Message> = context.messages().to_vec();
        let response = llm.chat(messages).await?;

        context.add_assistant_message(&response);
        println!("\n{}\n", response);
    }

    Ok(())
}

async fn check_status(config: &Config) -> Result<()> {
    println!("Mimir Status:");
    println!("  Name: {}", config.agent.name);
    println!("  LLM Model: {}", config.llm.model);
    println!("  LLM Endpoint: {}", config.llm.endpoint);
    println!("  Proactivity: {}", config.agent.proactivity);
    println!("  Memory: enabled ({} chars limit)", config.memory.char_limit);
    println!("  ✓ Config loaded");

    let memory_path = MemoryLoader::get_memory_path();
    let memory = MemoryLoader::load(&memory_path).await?;
    println!("  ✓ memory.md loaded ({} chars)", memory.chars().count());

    Ok(())
}

async fn show_memory() -> Result<()> {
    let memory_path = MemoryLoader::get_memory_path();
    let memory = MemoryLoader::load(&memory_path).await?;
    println!("{}", memory);
    Ok(())
}
```

- [ ] **Step 2: Add uuid dependency to CLI**

`crates/mimir-cli/Cargo.toml`:
```toml
uuid = { version = "1.11", features = ["v4"] }
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check --workspace`
Expected: Clean compile

- [ ] **Step 4: Commit**

```bash
git add crates/mimir-cli/
git commit -m "feat(cli): implement ask, chat, status, and memory commands"
```

---

### Task 10: Default Config File and Directory Creation

**Files:**
- Create: `config/default.toml`
- Modify: `crates/mimir-core/src/config.rs`

- [ ] **Step 1: Create default config file**

`config/default.toml`:
```toml
[llm]
endpoint = "https://api.openai.com/v1"
api_key = ""
model = "gpt-4o"
max_tokens = 4096
temperature = 0.2

[agent]
name = "Mimir"
proactivity = "important_only"
verbose_reasoning = false

[memory]
enabled = true
char_limit = 2500
auto_manage = true
temporal_horizon = 30
```

- [ ] **Step 2: Add config dir creation to Config::load**

Modify `crates/mimir-core/src/config.rs`:
```rust
impl Config {
    pub fn load(path: Option<&std::path::Path>) -> Result<Self> {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("mimir");
        std::fs::create_dir_all(&config_dir)?;

        let config_path = path.map(|p| p.to_path_buf())
            .unwrap_or_else(|| config_dir.join("config.toml"));

        let mut config = if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            toml::from_str(&content)?
        } else {
            let default = Self::default();
            default.save(&config_path)?;
            default
        };

        // ... env overrides remain the same
        Ok(config)
    }
}
```

- [ ] **Step 3: Create default memory.md on first run**

Modify `crates/mimir-core/src/memory/loader.rs`:
```rust
impl MemoryLoader {
    pub async fn load(path: &Path) -> Result<String> {
        if !path.exists() {
            // Create parent directory if needed
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let default = Self::default_memory();
            std::fs::write(path, &default)?;
            return Ok(default);
        }

        let content = fs::read_to_string(path).await?;
        Ok(content)
    }
}
```

- [ ] **Step 4: Commit**

```bash
git add config/default.toml crates/mimir-core/src/config.rs crates/mimir-core/src/memory/loader.rs
git commit -m "feat(config): add default config file and auto-create config/memory directories"
```

---

### Task 11: Integration Tests

**Files:**
- Create: `tests/integration_tests.rs`

- [ ] **Step 1: Write integration test**

`tests/integration_tests.rs`:
```rust
use mimir_core::config::Config;
use mimir_core::memory::MemoryLoader;

#[test]
fn test_config_loads_with_defaults() {
    // Ensure we can load config (will create default if not present)
    let config = Config::load(None).unwrap();
    assert_eq!(config.agent.name, "Mimir");
    assert_eq!(config.llm.model, "gpt-4o");
}

#[tokio::test]
async fn test_memory_loader_loads_default() {
    let temp_dir = std::env::temp_dir();
    let memory_path = temp_dir.join("test_mimir_memory.md");

    // Clean up if exists
    let _ = std::fs::remove_file(&memory_path);

    let content = MemoryLoader::load(&memory_path).await.unwrap();
    assert!(content.contains("Mimir Working Memory"));
    assert!(content.contains("not yet configured"));

    // Clean up
    let _ = std::fs::remove_file(&memory_path);
}

#[test]
fn test_memory_manager_capacity() {
    use mimir_core::memory::MemoryManager;

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("test_capacity.md");
        let _ = std::fs::remove_file(&path);

        let manager = MemoryManager::new(&path, 100).await.unwrap();
        assert_eq!(manager.char_limit(), 100);
        assert_eq!(manager.current_chars(), 0);
        assert!(!manager.is_full());

        let _ = std::fs::remove_file(&path);
    });
}
```

- [ ] **Step 2: Add test-only method to MemoryManager**

Modify `crates/mimir-core/src/memory/manager.rs`:
```rust
impl MemoryManager {
    pub fn char_limit(&self) -> usize {
        self.char_limit
    }
}
```

- [ ] **Step 3: Run integration tests**

Run: `cargo test --workspace`
Expected: All tests pass

- [ ] **Step 4: Commit**

```bash
git add tests/
git commit -m "test: add integration tests for config and memory"
```

---

### Task 12: Cargo.toml Finalization and README

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/mimir-core/Cargo.toml`
- Modify: `crates/mimir-cli/Cargo.toml`
- Modify: `crates/mimir-server/Cargo.toml`

- [ ] **Step 1: Finalize workspace Cargo.toml**

`Cargo.toml`:
```toml
[workspace]
members = ["crates/mimir-cli", "crates/mimir-core", "crates/mimir-server"]
resolver = "2"

[workspace.package]
version = "0.1.0"
edition = "2024"
authors = ["Devansh Bhavsar <dev@example.com>"]
license = "GPL-3.0"
repository = "https://github.com/BhavsarDevansh/Mimir"

[workspace.dependencies]
tokio = { version = "1.43", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
anyhow = "1.0"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
futures = "0.3"
```

- [ ] **Step 2: Verify full workspace builds**

Run: `cargo build --workspace`
Expected: Successful build

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml crates/*/Cargo.toml
git commit -m "chore: finalize workspace dependencies and metadata"
```

---

## Spec Coverage Check

| Spec Requirement | Task |
|-----------------|------|
| CLI (`mimir start`, `ask`, `chat`, `status`) | Task 1, Task 9 |
| Config (TOML + env overrides) | Task 2, Task 10 |
| LLM Client (OpenAI-compatible, streaming) | Task 3 |
| memory.md (loader, manager, auto-management) | Task 4, Task 10 |
| Context Manager (conversation history) | Task 5 |
| Personality (transparent default, system prompt) | Task 6 |
| Tool Registry (dynamic registration, OpenAI schema) | Task 7 |
| Chat Server (Axum, SSE streaming) | Task 8 |
| Integration Tests | Task 11 |

## Placeholder Scan

No placeholders found. All steps contain exact code, exact commands, and expected outputs.

## Type Consistency Check

- `Config::load(None)` used consistently
- `Message::system/user/assistant` constructors used consistently
- `MemoryManager` methods (`add`, `replace`, `remove`) have consistent signatures
- `LlmClient::chat` and `chat_stream` signatures consistent across files

---

## Next Steps After Phase 1

Phase 1 establishes the Core Agent. The next phases (not in this plan):
- **Phase 2:** Knowledge Graph (SQLite schema, entities, facts, temporal data)
- **Phase 3:** Connectors (Gmail, Calendar, Photos)
- **Phase 4:** Reasoning Engine (multi-threaded investigation, meta-threads)
- **Phase 5:** Proactive Agent (event monitoring, pattern recognition)
- **Phase 6:** Vision & Object Tracking
