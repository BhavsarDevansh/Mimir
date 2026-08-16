# Skills and Tools

## Philosophy
The agent should not rely solely on an LLM for everything. Structured tools and skills — deterministic, fast, and reliable — handle the tasks they are good at, while the LLM handles reasoning, synthesis, and ambiguity. This hybrid approach is cheaper, faster, and more correct.

## What Is a Tool?
A tool is a deterministic function the agent can invoke. It has a name, a schema, and an implementation. Tools are for operations, calculations, and data retrieval.

### Examples of Simple Tools
- `get_current_date()` → Returns today's date
- `multiply(a: f64, b: f64)` → Returns a × b
- `format_iso_date(date: &str, timezone: &str)` → Returns formatted date string
- `calculate_time_difference(start: &str, end: &str)` → Returns duration
- `read_file(path: &str)` → Returns file contents
- `search_local_files(query: &str)` → Returns matching file paths

### Examples of Complex Tools
- `query_knowledge_graph(subject: &str, predicate: &str, object: Option<&str>)` → Returns matching facts
- `search_photos_by_gps(lat: f64, lon: f64, radius_m: u32)` → Returns photo metadata
- `extract_entities_from_text(text: &str)` → Returns structured entities
- `generate_embedding(text: &str)` → Returns vector embedding
- `run_web_search(query: &str)` → Returns search results
- `detect_objects_in_image(image_path: &str)` → Returns bounding boxes and labels

## What Is a Skill?
A skill is a higher-level capability composed of one or more tools, possibly with LLM-based reasoning steps in between. Skills are for workflows and multi-step tasks.

### Examples of Skills
- **Flight Extraction Skill:** Reads email → detects flight confirmations → extracts structured data (airline, flight number, dates, airports) → creates KB facts
- **Calendar Conflict Resolution Skill:** Scans calendar → detects overlaps → queries attendees → suggests new times → adds proposed events
- **Packing Suggestion Skill:** Reads upcoming trip → checks weather → checks home inventory → generates packing list
- **Photo Location Inference Skill:** Reads photo EXIF → reverse geocodes GPS → creates "visited_place" facts
- **Research Synthesis Skill:** Takes a topic → searches web → extracts key facts → builds causal chain → synthesizes narrative

## How Skills Evolve

### Phase 1: LLM-Only
Initially, a new capability is handled entirely by the LLM with generic tools:
```
User: "When was I last in Rome?"
LLM uses: query_knowledge_graph + search_photos + search_calendar + search_email
LLM reasons across results and synthesizes answer.
```

### Phase 2: Heuristic Skill
As patterns emerge, the agent extracts a heuristic skill:
```rust
struct TemporalLocationQuerySkill {
    // Optimized query order based on learned patterns
    query_order: Vec<QuerySource>,  // [KnowledgeGraph, Photos, Calendar, Email]
    // Pre-filters based on common patterns
    temporal_window: Duration,     // Look back 2 years by default
    min_confidence: f32,           // 0.70
    // When to escalate to deep investigation
    deep_investigation_threshold: u32, // < 2 sources found
}
```

### Phase 3: Native Tool
For high-frequency, well-understood tasks, the skill becomes a native, deterministic tool:
```rust
fn when_was_user_in_location(
    user_id: &str,
    location: &str,
    kb: &KnowledgeGraph,
    connectors: &ConnectorManager,
) -> Result<LocationVisit> {
    // Direct SQL query with optimized joins
    // No LLM invocation needed
}
```

### Phase 4: Composable Skill
Complex skills compose simpler skills:
```
Travel Preparation Skill
├── Flight Monitoring Skill (detects upcoming flights)
├── Weather Lookup Skill (checks destination weather)
├── Wardrobe Suggestion Skill (checks closet inventory + weather)
├── Calendar Sync Skill (adds reminders)
└── Notification Skill (delivers proactive message)
```

## Skill Registry

Skills are registered alongside tools in the Tool Registry:

```rust
trait Skill: Send + Sync {
    fn id(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> serde_json::Value;   // JSON Schema
    fn output_schema(&self) -> serde_json::Value;  // JSON Schema
    fn complexity(&self) -> SkillComplexity;       // Simple | Heuristic | Composite
    async fn execute(&self, input: SkillInput) -> Result<SkillOutput>;
}
```

## Skill Discovery

The agent can discover new skills it needs:

### User-Driven
User says: "Can you summarize my weekly Spotify listening?" Agent does not have this skill, so:
1. Checks if existing skills can compose to achieve this
2. If not, attempts with generic LLM + tools (Phase 1)
3. If used repeatedly, extracts a heuristic (Phase 2)
4. Eventually, proposes a native skill

### Pattern-Driven
After 10 uses of generic web search + synthesis for historical questions, the agent notices:
```
> "I have answered 10 historical research questions using the same pattern.
> Should I create a dedicated Research Synthesis skill for faster, cheaper responses?"
[Create skill] [Keep using generic approach] [Remind me later]
```

## Skill Improvement

Skills improve over time based on usage and error data:

```rust
struct SkillMetrics {
    skill_id: String,
    invocation_count: u32,
    avg_latency_ms: u32,
    success_rate: f32,
    user_correction_rate: f32,
    avg_token_cost: Option<u32>,  // If LLM-involved
    last_improved: DateTime,
}
```

When a skill has high correction rate:
1. Agent analyzes failed invocations
2. Identifies common error patterns
3. Proposes skill update or replacement

## Tool Calling vs. Skill Invocation

The LLM sees both tools and skills in its context:

```json
{
  "tools": [
    {
      "type": "function",
      "function": {
        "name": "get_current_date",
        "description": "Returns today's date. Use for any date calculation.",
        "parameters": { "type": "object", "properties": {} }
      }
    },
    {
      "type": "function",
      "function": {
        "name": "flight_extraction_skill",
        "description": "Extracts structured flight data from an email or message. Optimized for airline confirmations.",
        "parameters": { "type": "object", "properties": { "text": { "type": "string" } } }
      }
    }
  ]
}
```

The LLM chooses whether to call a raw tool (for novel tasks) or a skill (for well-understood workflows).

## Technology Stack
- **Tools:** Native Rust functions, compiled into the agent
- **Skills:** Rust structs implementing the `Skill` trait
- **Skill Storage:** TOML/JSON definitions + WASM modules (for sandboxed custom skills)
- **Metrics:** SQLite, updated on every invocation
- **Hot Reload:** Skills can be updated without restarting the agent (future)
