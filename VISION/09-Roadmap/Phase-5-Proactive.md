# Phase 5: Proactive Agent

## Goal
Build the event monitoring, pattern recognition, and proactive suggestion system.

## Duration
4–6 weeks

## Deliverables

### 5.1 Event Monitor
- [ ] Subscribe to connector event streams
- [ ] Temporal trigger detection (upcoming events, deadlines)
- [ ] Pattern break detection
- [ ] Cross-connector event correlation

### 5.2 Pattern Recognizer
- [ ] Temporal pattern extraction (routines, schedules)
- [ ] Sequential pattern extraction (event chains)
- [ ] Correlational pattern extraction
- [ ] Pattern confidence scoring
- [ ] Pattern storage in Knowledge Graph

### 5.3 Prediction Engine
- [ ] Opportunity scoring algorithm
- [ ] Urgency, relevance, confidence, intrusiveness weighting
- [ ] User preference modulation
- [ ] Proactivity level enforcement

### 5.4 Action Executor
- [ ] Notification generation
- [ ] Suggestion generation
- [ ] Auto-act with confirmation gate
- [ ] Silent update execution

### 5.5 Feedback Loop
- [ ] Track user responses to proactive suggestions
- [ ] Update pattern confidence from outcomes
- [ ] Preference learning from dismissals/acceptances
- [ ] Proactive history log

### 5.6 CLI Integration
- [ ] `agent proactive history`
- [ ] `agent proactive pause 2h`
- [ ] `agent proactive disable <category>`
- [ ] `agent proactive preview`

### 5.7 Testing
- [ ] Unit tests for pattern recognition
- [ ] Mock event injection
- [ ] End-to-end proactive flow tests
- [ ] User preference learning tests

## Success Criteria
- Agent detects upcoming events and alerts appropriately
- Patterns are learned from 2+ weeks of data
- User corrections update future behavior
- Proactivity levels are respected
- False positive rate < 20%

## Dependencies
- Phase 1 (Core Agent)
- Phase 2 (Knowledge Graph)
- Phase 3 (Connectors)

## Risks
- Pattern recognition requires significant historical data
- False positives can annoy users
- Event correlation across connectors is complex
- Privacy concerns with behavioral tracking
