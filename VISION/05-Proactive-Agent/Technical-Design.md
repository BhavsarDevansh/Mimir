# Proactive Agent — Technical Design

## Architecture

The Proactive Agent is an event-driven system that monitors streams, detects patterns, predicts needs, and initiates actions. It operates on a trust ladder where permissions are earned, not assumed.

## Components

### 1. Event Monitor
Watches for triggers from connectors and internal systems.

**Event Types:**
```rust
enum ProactiveEvent {
    // Temporal
    UpcomingEvent { event_id: String, minutes_until: i64 },
    CalendarConflict { event_a: String, event_b: String },
    DeadlineApproaching { task_id: String, hours_remaining: i64 },
    
    // Spatial
    LocationChange { from: Location, to: Location },
    NearPlaceOfInterest { place_id: String, distance_meters: f64 },
    
    // Behavioral
    PatternBreak { pattern: String, expected: DateTime, actual: Option<DateTime> },
    UnusualActivity { description: String, confidence: f32 },
    
    // Environmental
    HomeEvent { entity_id: String, state_change: String },
    WeatherAlert { alert: WeatherAlert },
    
    // Cross-connector
    NewDataAvailable { connector_id: String, summary: String },
    InferredFact { fact: ExtractedFact, confidence: f32 },
    
    // Trust ladder
    PermissionOpportunity { category: String, evidence: Vec<String>, confidence: f32 },
}
```

### 2. Trust Manager
Manages the user's position on the trust ladder and decides when to offer permissions.

```rust
struct TrustManager {
    stage: TrustStage,  // Observation | GentleOffers | PatternPermissions | Autonomous
    permissions: Vec<Permission>,
    interaction_history: Vec<ProactiveInteraction>,
    observation_start: DateTime,
}

impl TrustManager {
    /// Decide if the agent is ready to offer a permission
    fn should_offer_permission(
        &self,
        category: &str,
        evidence_count: u32,
        acceptance_rate: f32,
    ) -> bool {
        match self.stage {
            TrustStage::Observation => false,
            TrustStage::GentleOffers => {
                // First offer: at least 1 opportunity, any confidence
                evidence_count >= 1 && self.interaction_history.is_empty()
            }
            TrustStage::PatternPermissions => {
                // Category permission: strong pattern, high acceptance
                evidence_count >= 5 && acceptance_rate > 0.7
            }
            TrustStage::Autonomous => {
                // Can offer domain-level permissions
                evidence_count >= 10 && acceptance_rate > 0.8
            }
        }
    }
}
```

### 3. Pattern Recognizer
Learns routines from historical data.

**Pattern Types:**
- **Temporal:** "User checks email at 9 AM weekdays"
- **Sequential:** "After gym, user showers and then listens to podcasts"
- **Correlational:** "Low sleep → high coffee consumption next day"
- **Preparatory:** "Before flights, user packs the night before"

**Pattern Storage:**
```rust
struct Pattern {
    id: String,
    pattern_type: PatternType,
    description: String,
    confidence: f32,
    occurrence_count: u32,
    last_observed: DateTime,
    conditions: Vec<Condition>,
    predicted_outcome: String,
}
```

### 4. Permission Opportunity Detector
Identifies moments where a permission offer would be timely and well-received.

```rust
struct PermissionOpportunity {
    id: String,
    category: String,        // e.g., "calendar_auto_add"
    trigger_event: ProactiveEvent,
    evidence: Vec<String>,    // Fact IDs supporting the offer
    user_acceptance_rate: f32,
    confidence: f32,
    urgency: f32,
}
```

### 5. Prediction Engine
Scores upcoming opportunities for proactivity.

```rust
struct ProactiveOpportunity {
    id: String,
    event: ProactiveEvent,
    pattern: Option<Pattern>,
    suggested_action: ProactiveAction,
    urgency: f32,           // Time-sensitivity
    relevance: f32,         // User-specific importance
    confidence: f32,        // Likelihood user will find helpful
    intrusiveness: f32,     // How disruptive the notification is
    final_score: f32,       // Weighted combination
}
```

**Scoring formula:**
```
final_score = urgency × relevance × confidence × (1 - intrusiveness) × user_preference_multiplier × trust_stage_multiplier
```

### 6. Action Executor
Executes approved or auto-approved proactive actions.

```rust
enum ProactiveAction {
    Notify { title: String, body: String, actions: Vec<UserAction> },
    Suggest { query: String, context: String },
    OfferPermission { permission: Permission, evidence: Vec<String> },
    AutoAct { action: ConnectorAction, requires_confirmation: bool },
    SilentUpdate { fact_update: Vec<ExtractedFact> },
}
```

### 7. Feedback Loop
Records outcomes to improve future predictions.

```rust
struct ProactiveOutcome {
    opportunity_id: String,
    user_response: UserResponse,  // Ignored | Dismissed | Accepted | Modified | Rejected | PermissionGranted
    response_time_seconds: Option<i64>,
    was_helpful: Option<bool>,
    timestamp: DateTime,
}
```

## Trigger Conditions

### First Offer Trigger (Gentle Offers Stage)
```
IF: TrustStage == GentleOffers
AND: New opportunity detected (e.g., flight email not in calendar)
AND: No prior interaction for this category
THEN: Offer to help (one time, explicit ask)
```

### Permission Offer Trigger (Pattern Permissions Stage)
```
IF: TrustStage == PatternPermissions
AND: Category has 5+ past interactions
AND: Acceptance rate > 70%
AND: New opportunity in same category
THEN: Offer blanket permission for category
```

### Flight Preparation Trigger (Autonomous Stage)
```
IF: TrustStage == Autonomous
AND: Calendar event with location != home AND type = "flight" AND hours_until < 24
AND: Permission "calendar_auto_add" is active
AND: No packing reminder exists
THEN: Create packing reminder (time = departure_time - 4h)
WITH: Context about weather at destination, long-haul items, storage box locations
```

### Calendar Conflict Trigger
```
IF: Two calendar events overlap AND both have high priority
THEN: Notify user with resolution options
```

### Location-Based Reminder Trigger
```
IF: User location within 500m of grocery_store AND kb.contains("needed: milk")
THEN: Notify "You are near a grocery store. You needed milk."
```

## Scheduling

The Proactive Agent runs on a schedule:
- **Every minute:** Check temporal triggers (upcoming events, deadlines)
- **Every 5 minutes:** Check connector syncs for new data
- **Every hour:** Run pattern recognition on recent data
- **Daily:** Full pattern re-evaluation and confidence updates
- **Weekly:** Trust stage review (escalate or de-escalate based on interaction quality)

## User Preference Integration

Proactive behavior is modulated by user preferences stored in the Knowledge Graph:

```rust
// Example preferences
{
  "proactive.level": "important_only",
  "proactive.quiet_hours": "22:00-08:00",
  "proactive.suppressed_types": ["music_suggestions"],
  "proactive.auto_confirm_actions": ["calendar_add_from_flight_email"],
  "proactive.preparation_lead_time": { "flight": "4h", "meeting": "15m" }
}
```

## Sensitivity Engine Integration

The Proactive Agent consults the Sensitivity Engine before every action:

```rust
struct SensitivityCheck {
    topic: String,
    sensitivity_level: SensitivityLevel,  // public | private | restricted
    user_present: Context,  // alone | with_others | unknown
    allowed: bool,
    modulated_action: Option<ProactiveAction>,
}
```

**Examples:**
- Medical reminder + user with others → Suppress or generalize ("Remember your appointment")
- Financial alert + public context → Delay until private
- Relationship message + shared device → Require explicit unlock

## Technology Stack
- **Event Bus:** Internal async channel (tokio::sync::broadcast or similar)
- **Pattern Recognition:** LLM-based pattern extraction + statistical correlation
- **Scheduling:** tokio::time intervals or cron-like scheduler
- **Storage:** SQLite via Knowledge Graph
