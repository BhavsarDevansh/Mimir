# Integration Points

## LLM API Integration

### OpenAI-Compatible Endpoint
The agent uses any OpenAI-compatible API:
- OpenAI (GPT-4, GPT-5)
- Anthropic (via compatibility layer or direct)
- Local models (Ollama, LM Studio, llama.cpp)
- Azure OpenAI

### Configuration
```toml
[llm]
endpoint = "https://api.openai.com/v1"
api_key = "sk-..."
model = "gpt-5"
max_tokens = 4096
temperature = 0.2  # Low for reasoning, higher for chat

# Optional: separate model for embeddings
[llm.embeddings]
endpoint = "https://api.openai.com/v1"
model = "text-embedding-3-large"
```

### Fallback Strategy
If primary LLM fails:
1. Retry with exponential backoff
2. Switch to fallback endpoint (if configured)
3. Degrade to cached/pre-computed responses
4. Notify user of degraded mode

## Connector Integration Points

### Standard Interface
All connectors implement the `Connector` trait (see Connectors/Technical-Design.md).

### Discovery
Connectors are loaded dynamically from:
- Built-in connectors (compiled into binary)
- Plugin connectors (shared libraries or WASM modules)
- Custom connectors (user-provided scripts)

### Communication
- Connectors run in separate async tasks
- Communicate with Core Agent via internal message bus
- Each connector has isolated state and error handling

## Knowledge Graph Integration

### Read API
```rust
impl KnowledgeGraph {
    async fn get_entity(&self, id: &str) -> Result<Entity>;
    async fn query_facts(&self, query: FactQuery) -> Result<Vec<Fact>>;
    async fn search(&self, query: &str, options: SearchOptions) -> Result<Vec<Entity>>;
    async fn get_related(&self, entity_id: &str, depth: u32) -> Result<Vec<Fact>>;
}
```

### Write API
```rust
impl KnowledgeGraph {
    async fn insert_entity(&self, entity: Entity) -> Result<String>; // returns ID
    async fn insert_fact(&self, fact: Fact) -> Result<String>;
    async fn update_fact(&self, id: &str, updates: FactUpdate) -> Result<()>;
    async fn delete_fact(&self, id: &str, reason: &str) -> Result<()>;
    async fn upsert_preference(&self, preference: Preference) -> Result<()>;
}
```

### Event Stream
Knowledge Graph emits events for subscribers:
```rust
enum KbEvent {
    EntityInserted { entity: Entity },
    FactInserted { fact: Fact },
    FactUpdated { fact: Fact, old_confidence: f32 },
    FactDeleted { fact_id: String },
    PreferenceChanged { preference: Preference },
}
```

## Reasoning Engine Integration

### Invocation
```rust
impl ReasoningEngine {
    async fn investigate(
        &self,
        query: &str,
        options: InvestigationOptions,
    ) -> Result<Investigation>;
}
```

### Streaming
Investigations can stream progress:
```rust
let mut stream = reasoning_engine.investigate_stream(query).await;
while let Some(update) = stream.next().await {
    match update {
        Progress { step, description } => println!("Step {}: {}", step, description),
        HypothesisGenerated { hypothesis } => {},
        EvidenceFound { evidence } => {},
        Complete { investigation } => break,
    }
}
```

## Vision System Integration

### Camera Registration
```rust
impl VisionTracker {
    async fn register_camera(&self,
        camera_id: &str,
        source: VideoSource,
        zones: Vec<Zone>,
    ) -> Result<()>;
}
```

### Object Query
```rust
impl VisionTracker {
    async fn locate_object(
        &self,
        object_label: &str,
    ) -> Result<Option<ObjectLocation>>;
    
    async fn search_live(
        &self,
        object_label: &str,
        duration: Duration,
    ) -> Result<Vec<ObjectLocation>>;
}
```

## External Tool Integration

### Web Search
Pluggable search backends:
- DuckDuckGo (default, no API key)
- Brave Search (API key required)
- SerpAPI (paid, comprehensive)
- Google Custom Search (API key required)

### Browser Automation (Future)
- Headless browser for JavaScript-heavy sites
- Not in initial scope

### Home Assistant
- Native connector using HA WebSocket API
- Bidirectional: read states, trigger actions
- Leverages HA's existing device ecosystem
