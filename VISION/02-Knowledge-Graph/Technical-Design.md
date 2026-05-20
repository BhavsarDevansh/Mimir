# Knowledge Graph — Technical Design

## Data Model

### Entity
The fundamental node in the graph.
```rust
struct Entity {
    id: String,           // UUID
    name: String,         // Human-readable label
    entity_type: EntityType,  // Person, Place, Event, Object, Concept, etc.
    aliases: Vec<String>, // Alternative names
    created_at: DateTime,
    updated_at: DateTime,
}
```

### Fact (Relationship)
A directed, temporal, attributed edge between entities.
```rust
struct Fact {
    id: String,
    subject_id: String,   // Entity ID
    predicate: String,      // e.g., "visited", "owns", "works_as"
    object_id: String,    // Entity ID or literal value
    object_literal: Option<String>, // For non-entity objects
    
    // Temporal bounding
    valid_from: Option<DateTime>,
    valid_until: Option<DateTime>,
    
    // Confidence and provenance
    confidence: f32,      // 0.0 to 1.0
    sources: Vec<Source>,
    inferred: bool,       // Was this inferred or directly observed?
    inference_chain: Option<Vec<String>>, // Fact IDs that led to this
    
    created_at: DateTime,
    updated_at: DateTime,
}
```

### Source
Provenance for every fact.
```rust
struct Source {
    source_type: SourceType,  // email, calendar, photo, message, inference, user_edit
    connector_id: String,     // Which connector provided it
    raw_reference: String,    // e.g., email_id, photo_path, event_id
    extracted_at: DateTime,
    extraction_method: String, // e.g., "llm_extraction", "structured_parse"
}
```

### Preference
User preferences are special facts with higher weight in decision-making.
```rust
struct Preference {
    id: String,
    category: String,     // e.g., "calendar_auto_add", "notification_style"
    key: String,
    value: PreferenceValue,
    confidence: f32,
    learned_from: Vec<String>, // Interaction IDs
    overridden_by_user: bool,
}
```

## Storage Backend

### Primary: SQLite (Local-First)
- Single-file database
- Full-text search via FTS5
- JSON columns for flexible metadata
- Graph traversal via recursive CTEs

### Scale-Out Path: RDF/Graph Database
If the graph grows beyond SQLite's comfort zone, support migration to:
- **RDFLib / Oxigraph** for SPARQL queries
- **DuckDB** for analytical workloads
- **Neo4j** (self-hosted) for heavy graph traversal

## Schema Design

```sql
-- Entities
CREATE TABLE entities (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    entity_type TEXT NOT NULL,
    aliases TEXT, -- JSON array
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Facts (the core graph edges)
CREATE TABLE facts (
    id TEXT PRIMARY KEY,
    subject_id TEXT NOT NULL REFERENCES entities(id),
    predicate TEXT NOT NULL,
    object_id TEXT REFERENCES entities(id),
    object_literal TEXT,
    valid_from TIMESTAMP,
    valid_until TIMESTAMP,
    confidence REAL NOT NULL DEFAULT 0.5,
    inferred BOOLEAN NOT NULL DEFAULT FALSE,
    inference_chain TEXT, -- JSON array of fact IDs
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Sources (provenance)
CREATE TABLE sources (
    id TEXT PRIMARY KEY,
    fact_id TEXT NOT NULL REFERENCES facts(id),
    source_type TEXT NOT NULL,
    connector_id TEXT,
    raw_reference TEXT,
    extracted_at TIMESTAMP,
    extraction_method TEXT
);

-- Preferences
CREATE TABLE preferences (
    id TEXT PRIMARY KEY,
    category TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL, -- JSON
    confidence REAL NOT NULL DEFAULT 0.5,
    learned_from TEXT, -- JSON array
    overridden_by_user BOOLEAN DEFAULT FALSE,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Full-text search on entity names and aliases
CREATE VIRTUAL TABLE entity_fts USING fts5(name, aliases, content='entities', content_rowid='rowid');
```

## Query Patterns

### 1. Entity Resolution
Before inserting a fact, resolve whether entities already exist:
```sql
SELECT id FROM entities 
WHERE name = ? OR json_array_contains(aliases, ?)
ORDER BY 
  (name = ?) DESC,
  length(name) ASC;
```

### 2. Temporal Fact Retrieval
```sql
SELECT * FROM facts 
WHERE subject_id = ? AND predicate = ?
  AND (valid_from IS NULL OR valid_from <= ?)
  AND (valid_until IS NULL OR valid_until >= ?)
ORDER BY confidence DESC;
```

### 3. Graph Traversal
```sql
WITH RECURSIVE graph AS (
  SELECT subject_id, predicate, object_id, 1 as depth
  FROM facts
  WHERE subject_id = ?
  UNION ALL
  SELECT f.subject_id, f.predicate, f.object_id, g.depth + 1
  FROM facts f
  JOIN graph g ON f.subject_id = g.object_id
  WHERE g.depth < ?
)
SELECT * FROM graph;
```

### 4. Confidence-Weighted Search
```sql
SELECT f.*, e.name as object_name 
FROM facts f
JOIN entities e ON f.object_id = e.id
WHERE f.subject_id = ? 
  AND f.confidence > ?
ORDER BY f.confidence DESC, f.valid_from DESC;
```

## Inference Engine (Lightweight)
The Knowledge Graph includes a small rule engine for deriving new facts:

**Example rules:**
- If `A visited B` and `B is_in C`, then `A visited C` (with reduced confidence)
- If `email_from X contains flight_confirmation` and `X has_date Y`, then `user has_flight on Y`
- If `user rejected_action A` (3+ times), then create `preference: reject A`

Rules are stored as declarative JSON and evaluated periodically or on insertion.

## Learning Mechanism

### Pattern Extraction
After each interaction, extract potential facts and preferences:
1. Run conversation through LLM with extraction prompt
2. LLM returns structured facts with confidence
3. Merge with existing graph (upsert with source=interaction)

### Preference Learning
Track user corrections:
- User says "don't add emails from X to calendar" → Extract preference
- User deletes auto-added event → Reduce confidence of that extraction rule
- User confirms/ignores proactive suggestion → Adjust proactivity threshold

## Synchronization with Obsidian

### Export
- Periodic or on-demand export to `~/AgentKnowledge/`
- Each entity becomes a Markdown file
- Relationships become wiki-links
- YAML frontmatter contains metadata

### Import
- Watch `~/AgentKnowledge/` for changes
- Parse Markdown frontmatter and wiki-links
- Treat user edits as high-confidence facts with `source_type: user_edit`

## Technology Stack
- **Storage:** SQLite (rusqlite or sqlx)
- **Migration:** sqlx migrate or refinery
- **Serialization:** serde
- **FTS:** sqlite-fts5
- **Export:** Custom Markdown generator
