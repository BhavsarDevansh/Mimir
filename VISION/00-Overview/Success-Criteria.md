# Success Criteria

## Phase 1: Core Agent (MVP)
- [ ] CLI and chat interface operational
- [ ] OpenAI-compatible API endpoint configurable
- [ ] Basic tool-calling framework implemented
- [ ] Can hold a coherent multi-turn conversation with context

## Phase 2: Knowledge Graph
- [ ] Entities, relationships, and temporal facts stored persistently
- [ ] User can inspect and edit knowledge via CLI/Chat
- [ ] Confidence scores attached to facts
- [ ] Versioning and provenance tracking

## Phase 3: Connectors
- [ ] At least 3 core connectors operational (e.g., Email, Calendar, Photos)
- [ ] Normalized event/fact extraction from raw service data
- [ ] OAuth or token-based authentication handled securely
- [ ] Rate limiting and backoff implemented

## Phase 4: Reasoning Engine
- [ ] Multi-step investigation for complex queries
- [ ] Evidence gathering across multiple connectors
- [ ] Hypothesis generation and confidence scoring
- [ ] Transparent reasoning trail presented to user

## Phase 5: Proactive Agent
- [ ] Detects upcoming events and alerts with context
- [ ] Learns from user corrections and adjusts future behavior
- [ ] User-configurable proactivity levels (never / important only / always)

## Phase 6: Vision & Object Tracking
- [ ] Object detection and spatial memory for tracked items
- [ ] Can answer "where is X?" questions with high confidence
- [ ] Integrates with home camera feeds or uploaded images

## Long-Term Success
- [ ] Agent correctly answers "When was I last at X?" using 3+ data sources
- [ ] Agent proactively reminds user of contextually relevant things they would have forgotten
- [ ] User trusts agent enough to let it auto-add calendar events from emails
- [ ] Knowledge base feels like a genuine external memory
