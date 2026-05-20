# Phase 6: Vision & Object Tracking

## Goal
Enable spatial understanding and object tracking through cameras.

## Duration
6–8 weeks

## Deliverables

### 6.1 Frame Extraction
- [ ] RTSP stream support
- [ ] HTTP stream support
- [ ] Local file/video support
- [ ] Home Assistant camera integration
- [ ] Configurable sample rate

### 6.2 Object Detection
- [ ] YOLO integration (ONNX Runtime)
- [ ] Common object detection (80 COCO classes)
- [ ] Detection confidence thresholding
- [ ] Bounding box extraction

### 6.3 Object Re-Identification
- [ ] Embedding extraction from detections
- [ ] Cosine similarity matching
- [ ] Multi-object tracking across frames
- [ ] Cross-camera matching

### 6.4 User-Defined Object Learning
- [ ] Capture workflow (3-5 angles)
- [ ] Prototype embedding storage
- [ ] Matching against prototypes
- [ ] Object naming and management

### 6.5 Spatial Memory
- [ ] Zone definition (manual or learned)
- [ ] Object location history
- [ ] Trajectory tracking
- [ ] Query API for object locations

### 6.6 CLI Integration
- [ ] `agent vision track <object-name>`
- [ ] `agent vision untrack <object-name>`
- [ ] `agent vision inventory`
- [ ] `agent vision history <object-name>`
- [ ] `agent vision add-camera <source>`

### 6.7 Testing
- [ ] Unit tests for detection pipeline
- [ ] Mock camera streams
- [ ] Object re-identification accuracy tests
- [ ] End-to-end: "Where is X?" flow

## Success Criteria
- Agent can track 5+ user-defined objects
- Location queries answered with >80% accuracy
- Cross-camera tracking works
- Real-time processing at 1+ FPS per camera
- Privacy: no raw footage stored by default

## Dependencies
- Phase 1 (Core Agent)
- Phase 2 (Knowledge Graph)

## Risks
- YOLO performance on CPU may be slow
- Object re-identification accuracy in cluttered environments
- Camera network configuration complexity
- Privacy concerns with home cameras
