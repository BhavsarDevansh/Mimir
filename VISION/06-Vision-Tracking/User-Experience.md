# Vision & Object Tracking — User Experience

## Purpose
Enable the agent to understand the physical world through cameras and answer spatial questions like "Where did I put the TV remote?"

## Supported Inputs
- Home security cameras (IP cameras, Home Assistant streams)
- Phone/laptop camera (on-demand capture)
- Uploaded photos
- Video clips

## Object Tracking

### Setup
```bash
$ agent vision track "tv-remote"
📷 Teaching agent about "tv-remote"
1. Hold the TV remote in front of the camera
2. Capture 3-5 angles:
   [Capture] [Capture] [Capture]
3. Agent is learning the object...
✅ "tv-remote" is now tracked. I will look for it in camera feeds.
```

### Querying
```bash
$ agent ask "Where is the TV remote?"
🔍 Checking recent camera footage...

Last seen: Living room coffee table
Time: 10 minutes ago
Camera: Living room cam
Confidence: 0.92

Would you like me to keep looking? [Yes] [No]
```

### Spatial Memory
The agent maintains a "mental map" of object locations over time:
```bash
$ agent vision history "tv-remote"
2025-05-20 14:30: Living room coffee table (cam-1)
2025-05-20 12:15: Kitchen counter (cam-2)
2025-05-20 09:00: Bedroom nightstand (cam-3)
2025-05-19 22:00: Living room sofa (cam-1)
```

### Object Inventory
```bash
$ agent vision inventory
Tracked objects: 12
  ● tv-remote      (last seen: 10m ago, living room)
  ● car-keys       (last seen: 2h ago, hallway)
  ● wallet         (last seen: 3h ago, bedroom)
  ● airpods-case   (last seen: 1d ago, unknown)
  ⚠️ glasses       (not seen for 3 days)
```

## Privacy
- All image processing is local
- Raw footage is not stored unless explicitly requested
- Only object detections and embeddings are kept
- User can delete all vision data instantly
