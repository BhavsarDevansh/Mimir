# Phase 2: Knowledge Graph

## Goal
Build the persistent memory system: entities, facts, temporal reasoning, confidence scoring, and user inspection.

## Duration
4–6 weeks

## Deliverables

### 2.1 Database Schema
- [ ] Entities table
- [ ] Facts table with temporal bounds
- [ ] Sources/provenance table
- [ ] Preferences table
- [ ] Full-text search (FTS5)
- [ ] Migration system (refinery or sqlx migrate)

### 2.2 Entity Management
- [ ] CRUD operations for entities
- [ ] Alias resolution
- [ ] Entity deduplication
- [ ] Entity type system

### 2.3 Fact Management
- [ ] Insert facts with temporal bounds
- [ ] Query facts by subject/predicate/object
- [ ] Temporal queries ("what was true at time T?")
- [ ] Confidence scoring and updates
- [ ] Soft deletes with reason tracking

### 2.4 Provenance
- [ ] Every fact tracks its source
- [ ] Source types: user_edit, connector, inference, interaction
- [ ] Audit trail for fact changes

### 2.5 Preference System
- [ ] Store user preferences as typed facts
- [ ] Preference confidence tracking
- [ ] Override flags (user explicitly set)
- [ ] Preference inference from behavior

### 2.6 CLI Inspection
- [ ] `agent kb query "..."`
- [ ] `agent kb show <fact-id>`
- [ ] `agent kb edit <fact-id>`
- [ ] `agent kb forget <fact-id>`
- [ ] `agent kb browse --entity "..." --depth N`

### 2.7 Obsidian Export
- [ ] Export entities to Markdown files
- [ ] YAML frontmatter with metadata
- [ ] Wiki-link relationships
- [ ] Watch for external edits and re-import

### 2.8 Inference Engine (Basic)
- [ ] Declarative rule format (JSON)
- [ ] Rule evaluation on fact insertion
- [ ] Confidence propagation
- [ ] Example rules:
  - `visited(X) + is_in(X, Y) → visited(Y)`

### 2.9 Testing
- [ ] Unit tests for all CRUD operations
- [ ] Temporal query tests
- [ ] Graph traversal tests
- [ ] FTS search tests
- [ ] Performance tests with 10k+ facts

## Success Criteria
- Agent can store and retrieve facts persistently
- User can inspect and edit knowledge via CLI
- Temporal queries work correctly
- Confidence scores influence retrieval
- Obsidian export functional

## Dependencies
- Phase 1 (Core Agent)

## Risks
- SQLite performance with complex graph traversals
- Temporal logic edge cases (overlapping intervals, open-ended facts)
- Obsidian sync conflict resolution
