# Vision & Object Tracking — Technical Design

## Architecture

### Pipeline
```
Camera Feed → Frame Extraction → Object Detection → Embedding → 
  Spatial Memory → Knowledge Graph Integration
```

## Components

### 1. Frame Extractor
Extracts frames from video streams.
```rust
struct FrameExtractor {
    source: VideoSource,  // RTSP, HTTP, file, camera
    sample_rate: f32,     // Frames per second to analyze
    resolution: (u32, u32),
}
```

### 2. Object Detector
Detects objects in frames.
```rust
trait ObjectDetector {
    fn detect(&self, frame: &Image) -> Vec<Detection>;
}

struct Detection {
    bounding_box: (f32, f32, f32, f32),  // x, y, w, h (normalized)
    label: String,
    confidence: f32,
    embedding: Vec<f32>,  // Vector representation for re-identification
}
```

**Implementation options:**
- **YOLOv8 / YOLOv11:** Fast, accurate, runs on CPU/GPU
- **MobileSAM:** For segmentation if needed
- **Custom fine-tuned model:** For specific household objects

### 3. Object Re-Identification
Matches detected objects across frames and cameras.

```rust
struct ObjectTracker {
    object_id: String,
    class_label: String,
    embeddings: Vec<Vec<f32>>,  // Rolling window of embeddings
    last_seen: DateTime,
    last_location: String,  // Camera ID + zone
    trajectory: Vec<Location>,
}
```

**Matching strategy:**
- Cosine similarity between embeddings
- Hungarian algorithm for multi-object tracking
- Temporal consistency filtering

### 4. Spatial Memory
Stores where objects have been seen.

```rust
struct SpatialMemory {
    object_id: String,
    camera_id: String,
    zone: String,           // e.g., "coffee_table", "kitchen_counter"
    first_seen: DateTime,
    last_seen: DateTime,
    detection_count: u32,
    confidence: f32,
}
```

Zones can be defined manually or learned from frequent detection locations.

### 5. User-Defined Object Learning
When a user wants to track a new object:
1. Capture 3-5 images from different angles
2. Run through object detector to get bounding boxes
3. Extract embeddings from the region of interest
4. Store as prototype embeddings
5. Match future detections against prototype

## Data Storage

### Frame Storage (Optional)
Raw frames are NOT stored by default. Only:
- Detection metadata (bounding box, label, confidence)
- Embeddings (compact vectors)
- Thumbnails (small, optional)

### Spatial Memory Schema
```sql
CREATE TABLE object_detections (
    id TEXT PRIMARY KEY,
    object_id TEXT NOT NULL,
    object_label TEXT NOT NULL,
    camera_id TEXT NOT NULL,
    zone TEXT,
    bounding_box TEXT, -- JSON [x, y, w, h]
    confidence REAL NOT NULL,
    embedding TEXT, -- JSON array of floats
    detected_at TIMESTAMP NOT NULL,
    frame_reference TEXT -- Optional: path to stored frame
);

CREATE INDEX idx_detections_object ON object_detections(object_id, detected_at);
CREATE INDEX idx_detections_camera ON object_detections(camera_id, detected_at);
CREATE INDEX idx_detections_zone ON object_detections(zone, detected_at);
```

## Camera Integration

### Home Assistant
- Subscribe to camera entity state changes
- Access stream URL via HA API
- Leverage existing HA object detection (if available)

### RTSP / IP Cameras
- Direct stream access
- ffmpeg for frame extraction
- Supports most security cameras

### Local Files / Uploads
- Process uploaded images/videos on demand
- Extract EXIF metadata for temporal/spatial context

## Query Processing

When user asks "Where is X?":
1. Check Spatial Memory for most recent detection of object matching "X"
2. If found: return location + timestamp + confidence
3. If not found in recent history:
   a. Activate cameras to look for object
   b. Run detection on live frames for N seconds
   c. Return result or "not found"

## Technology Stack
- **Object Detection:** YOLO (ultralytics crate or ONNX Runtime)
- **Embeddings:** Local vision transformer (e.g., CLIP-style model)
- **Frame Processing:** OpenCV or image crate
- **Storage:** SQLite (shared with Knowledge Graph)
- **Optional GPU:** CUDA via tch-rs or ONNX Runtime GPU
