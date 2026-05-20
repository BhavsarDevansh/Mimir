# Journey: TV Remote Finder

## Trigger
User asks: "Where is the TV remote?"

## Flow

### 1. Core Agent Classifies Query
- Query type: `spatial_object_location`
- Object: "TV remote"
- Confidence: High (named object, tracked)

### 2. Vision System Query
```
Vision Tracking checks Spatial Memory for object_id "tv-remote"
```

**Most recent detection:**
- Camera: living-room-cam
- Zone: coffee_table
- Time: 10 minutes ago
- Confidence: 0.92

### 3. Answer Synthesis
> I last saw the TV remote on the **living room coffee table**, about 10 minutes ago.

### 4. If Not Found Recently
- Activate live camera search
- Scan all connected cameras for 30 seconds
- If found: report location
- If not found: "I haven't seen the TV remote recently. It might be in a blind spot or outside camera view."

### 5. Knowledge Graph Update
- Fact: `tv-remote located_at coffee_table` (2025-05-20T14:30:00Z)
- Confidence: 0.92
