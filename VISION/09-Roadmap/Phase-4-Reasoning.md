# Phase 4: Reasoning Engine

## Goal
Build the multi-step investigation system for complex queries.

## Duration
6–8 weeks

## Deliverables

### 4.1 Investigation Framework
- [ ] Investigation struct and lifecycle
- [ ] Hypothesis generation (LLM-based)
- [ ] Evidence gathering orchestration
- [ ] Confidence scoring system
- [ ] Investigation depth control
- [ ] Streaming progress updates

### 4.2 Query Parsing
- [ ] Intent classification (temporal, spatial, factual, research)
- [ ] Entity extraction from queries
- [ ] Query embedding generation

### 4.3 Evidence Gatherers
- [ ] Knowledge Graph evidence gatherer
- [ ] Connector evidence gatherer (delegates to active connectors)
- [ ] Web search evidence gatherer
- [ ] Evidence relevance scoring
- [ ] Evidence reliability scoring

### 4.4 Hypothesis Evaluation
- [ ] Match evidence to hypotheses
- [ ] Confidence update algorithm
- [ ] Contradiction detection
- [ ] Sub-hypothesis generation

### 4.5 Synthesis
- [ ] LLM-based answer synthesis
- [ ] Source citation in answers
- [ ] Uncertainty admission
- [ ] Verbose mode with reasoning trail

### 4.6 Caching
- [ ] Investigation result cache
- [ ] Query hash-based lookup
- [ ] TTL-based invalidation

### 4.7 CLI Integration
- [ ] `agent ask "..." --verbose`
- [ ] `agent ask "..." --depth 3`
- [ ] `agent audit last-question`
- [ ] `agent investigate "..."` (explicit investigation mode)

### 4.8 Testing
- [ ] Unit tests for confidence scoring
- [ ] Mock evidence gatherers
- [ ] End-to-end tests with known answers
- [ ] Performance tests (investigation duration)
- [ ] Edge cases: no evidence, contradictory evidence, ambiguous queries

## Success Criteria
- Multi-step investigations functional for complex queries
- Evidence from 3+ sources can be combined
- Confidence scores reflect actual reliability
- Verbose mode shows clear reasoning trail
- Answers are accurate and well-cited

## Dependencies
- Phase 1 (Core Agent)
- Phase 2 (Knowledge Graph)
- Phase 3 (Connectors)

## Risks
- LLM hallucination in hypothesis generation
- Investigation loops (generating hypotheses indefinitely)
- Token cost for complex investigations
- Timeout handling for long-running queries
