# Azario - parking spot guide

I am developing parking-platform as a research and learning project: a distributed Rust system for estimating parking-space availability, with video processing at the edge and frontend where app provides you map with trace to the destination. The application is designed with production concerns in mind, but it is **not production-ready**.

## Project Background

This project grew out of my engineering thesis. The thesis version was a much smaller MVP focused on proving the core idea. In this repository, I am deliberately extending that idea into a larger system to explore Rust, distributed systems, edge computer vision, cloud-native architecture, and SRE/DevOps. This repository is a continuation of the work, not a claim that the thesis had the same scope.

## AI-Assisted Development

I use AI coding tools, including OpenAI Codex, to help implement the application layer. Part of my goal is to examine how AI-assisted development can accelerate the evolution of an academic MVP into a larger system without replacing engineering judgment. I define the requirements, architecture, invariants, acceptance criteria, and design decisions, and I review the results. I do not treat generated code as correct by default; changes are checked through tests, static analysis, and review.

I intend to do the later infrastructure and reliability work **hands-on myself**: containerization, observability, Kubernetes, IaC and cloud deployment, CI/CD and GitOps, security, SLOs, failure testing, and backup/disaster recovery. These are planned learning stages, not capabilities already present in the repository. The separation between AI-assisted application implementation and hands-on platform engineering is deliberate.

## Current Status

My application currently includes Rust services, PostgreSQL/PostGIS, NATS JetStream, idempotent ingestion, parking search, Google Routes integration, a React frontend, an edge perception pipeline, an ONNX adapter, offline evaluation, and detection generation. Domain, integration, and synthetic tests cover these paths. I have prepared an application operability contract for future deployment; it does not constitute a production deployment.

### Current Limitations

I have not completed real-world validation of an ONNX model on recorded parking footage. RTSP input and offline edge resilience are not implemented. Production infrastructure, cloud deployment, SLOs, and disaster recovery are also pending. Synthetic and integration tests must not be confused with validation on real-world data.

## Architecture Overview

```text
Camera / simulator / recorded video
             |
             v
     Edge perception
             |
             v
       NATS JetStream
             |
             v
     Ingestion worker
             |
             v
    PostgreSQL/PostGIS
             |
             v
        Parking API
         /       \
 Google Routes  React frontend
```

The simulator publishes observations without processing images. The edge agent processes local recordings; raw video is not sent to the cloud by default. Google Routes is an optional dependency of the search path, not of the health checks.

## Engineering Principles

- A stale FREE observation becomes UNKNOWN; a failure is never interpreted as FREE.
- Producer sequence numbers, not timestamps, determine event order.
- At-least-once messaging is paired with idempotent persistence.
- Raw video stays at the edge by default.
- Local geospatial filtering runs before paid routing API calls.
- Failure and retry semantics are explicit; invalid requests and configuration are not retried indefinitely.

## Application Operability

The API exposes `GET /health/live` (process-only liveness), `GET /health/ready` (PostgreSQL `SELECT 1` with `DATABASE_CHECK_TIMEOUT_MS`, default 1500 ms), `GET /version`, and the legacy `GET /health`. If the database is unavailable, `/ready` returns 503 while `/live` remains 200. A liveness failure may call for a process restart; a readiness failure means the instance should stop receiving traffic. Neither Google APIs nor NATS are artificial readiness dependencies of the API.

On SIGINT/SIGTERM, the API stops accepting new requests and allows active requests up to 10 seconds to finish. The worker stops fetching messages, gives the current message up to 10 seconds, and preserves ACK/NAK/TERM semantics; shutdown alone never causes an ACK. The edge agent handles SIGINT/SIGTERM with a 15-second shutdown limit, closes the frame source and local state store, and does not start another publish after it detects shutdown. A blocking frame read may delay its response until the source returns.

All three services support `LOG_FORMAT=pretty|json` (default: pretty) and `RUST_LOG` (default: info). The API generates or propagates a valid UUID in `x-request-id`; its access log records the ID, method, path, status, and duration. Normal INFO logs do not include request bodies, complete headers, or a user's exact coordinates. Secrets such as `GOOGLE_MAPS_API_KEY`, `DATABASE_URL`, and NATS credentials must not appear in logs. `/version` returns the package version compiled into the binary and an optional `PARKING_GIT_SHA` supplied **at build time**; it does not invoke Git at runtime. Invalid startup configuration exits non-zero.

Important configuration: `API_BIND_ADDRESS` (default `0.0.0.0:3000`), `DATABASE_URL`, `DATABASE_MAX_CONNECTIONS` (10), `DATABASE_ACQUIRE_TIMEOUT_MS` (2000), `DATABASE_CHECK_TIMEOUT_MS` (1500), `DATABASE_OPERATION_TIMEOUT_MS` (5000), `PARKING_SEARCH_TIMEOUT_MS` (15000), `FRONTEND_ORIGIN` (development default `http://localhost:5173`), `GOOGLE_MAPS_HTTP_TIMEOUT_MS` (3000), `PARKING_SEARCH_RADIUS_M` (800), `PARKING_CANDIDATE_LIMIT` (25, maximum 625), `NATS_URL` for the worker and edge agent, `LOG_FORMAT`, and `RUST_LOG`. Localhost appears only as an explicit development default. The worker limits startup DB/NATS connections to 5 seconds and an individual message to 10 seconds. Google operations use the configured HTTP timeout. There is no unbounded startup retry loop.

I run migrations explicitly with `cargo run -p parking-api -- migrate` before starting API replicas or the worker. Success exits 0; failure exits non-zero. Neither service migrates automatically at startup. Persistent application state lives in PostgreSQL; the API has no critical in-memory-only user cache.

## Roadmap

**Application and perception:** 7.2B — validate with a real model and recording; 8 — RTSP camera integration; 8.1 — offline edge resilience. These items are planned, not complete.

**Platform and SRE, which I plan to implement hands-on:** 9 — containerization; 10 — observability; 11 — Kubernetes; 12 — IaC and cloud; 12.1 — CI/CD and GitOps; 13 — security hardening; 14 — SLOs, alerting, and runbooks; 15 — load and chaos testing; 16 — backup and disaster recovery. This is a roadmap, not a list of deployed infrastructure.

## Local Development

You need Rust and Docker with Docker Compose. The local services use the pinned `postgis/postgis:17-3.5` and `nats:2.15.0-alpine` images.

```bash
docker compose up -d
docker compose ps
export DATABASE_URL=postgres://parking:parking@localhost:5432/parking
export NATS_URL=nats://127.0.0.1:4222
export FRONTEND_ORIGIN=http://localhost:5173
cargo run -p parking-api -- migrate
cargo run -p parking-api
```

Google integration is optional at startup. Without `GOOGLE_MAPS_API_KEY`, health checks and other endpoints still work; `POST /v1/parking/search` returns a controlled 503. Configuration:

```bash
export GOOGLE_MAPS_API_KEY='...'          # secret; do not commit it
export GOOGLE_MAPS_HTTP_TIMEOUT_MS=3000   # default 3000
export PARKING_SEARCH_RADIUS_M=800        # default 800
export PARKING_CANDIDATE_LIMIT=25         # default 25, maximum 625
```

Run migrations explicitly with `parking-api migrate`; starting the API or worker does not change the schema.

Health checks and version:

```bash
curl -i http://127.0.0.1:3000/health/live
curl -i http://127.0.0.1:3000/health/ready
curl -i http://127.0.0.1:3000/version
```

Port 8222 exposes the local NATS diagnostic endpoint, for example `http://127.0.0.1:8222/healthz`.

Stop the local dependencies:

```bash
docker compose down
```

To reset **all local Compose data**:

```bash
docker compose down -v
```

## Asynchronous Ingestion Flow

```text
camera-simulator
  -> NATS JetStream
  -> durable pull consumer
  -> parking-ingestion-worker
  -> PostgreSQL/PostGIS
```

The `PARKING_OBSERVATIONS` stream stores `parking.observations.v1` on disk using `LimitsPolicy` and seven-day retention. The durable `parking-ingestion-v1` consumer uses `DeliverPolicy::All`, so a newly created consumer starts with the oldest message still retained.

Delivery is at least once. The worker ACKs only after the PostgreSQL commit. A transient database failure causes NAK and redelivery; malformed JSON or an unsupported contract version causes TERM. Redelivery after a commit is safe because of the `event_id` primary key, the unique `(camera_id, spot_id, sequence)` constraint, and a conditional UPSERT.

## Manual Smoke Test

Create a parking spot:

```bash
curl -i -X POST http://127.0.0.1:3000/v1/parking-spots \
  -H 'content-type: application/json' \
  -d '{
    "id": "019c1234-1234-7000-8000-123456789abe",
    "latitude": 52.2297,
    "longitude": 21.0122
  }'
```

In a second terminal, start the worker:

```bash
export DATABASE_URL=postgres://parking:parking@localhost:5432/parking
export NATS_URL=nats://127.0.0.1:4222
cargo run -p parking-ingestion-worker
```

In a third terminal, publish a FREE observation:

```bash
export NATS_URL=nats://127.0.0.1:4222
cargo run -p camera-simulator -- \
  --spot-id 019c1234-1234-7000-8000-123456789abe \
  --camera-id 019c1234-1234-7000-8000-123456789abd \
  --sequence 101 \
  --state free
```

The search should return the spot:

```bash
curl -i 'http://127.0.0.1:3000/v1/parking-spots/free?lat=52.2297&lon=21.0122&radius_m=500'
```

Then publish an OCCUPIED observation:

```bash
cargo run -p camera-simulator -- \
  --spot-id 019c1234-1234-7000-8000-123456789abe \
  --camera-id 019c1234-1234-7000-8000-123456789abd \
  --sequence 102 \
  --state occupied
```

The next search should no longer return that spot.

The HTTP endpoint remains a separate development/debug ingestion path. It does not publish to NATS and does not perform a dual write:

```bash
curl -i -X POST http://127.0.0.1:3000/v1/observations \
  -H 'content-type: application/json' \
  -d "{
    \"event_id\": \"019c1234-1234-7000-8000-123456789abc\",
    \"camera_id\": \"019c1234-1234-7000-8000-123456789abd\",
    \"spot_id\": \"019c1234-1234-7000-8000-123456789abe\",
    \"sequence\": 42,
    \"observed_at\": \"$(date -u +%Y-%m-%dT%H:%M:%SZ)\",
    \"state\": \"free\",
    \"model_score\": 0.97,
    \"model_version\": \"occupancy-v1\"
  }"
```

## Parking Search and Ranking

```text
POST /v1/parking/search
        |
        v
Geocoding API v4
        |
        v
PostGIS: fresh, free spots within radius
        |
        v
Routes API v2: Compute Route Matrix
DRIVE: vehicle position -> spots
        |
        v
exclude spots unreachable by car
        |
        v
Routes API v2: Compute Route Matrix
WALK: reachable spots -> destination
        |
        v
exclude spots unreachable on foot
        |
        v
DRIVE + WALK ranking
        |
        v
Routes API v2: Compute Routes for the winner
        |
        v
response with ETA and encoded polyline
```

PostGIS bounds the number of paid matrix elements before calling Google. A search performs at most two Route Matrix calls (one DRIVE and one WALK) and one DRIVE Compute Routes call for the winner. If geocoding returns several matches, the first is used; interactive address disambiguation is not implemented.

The previous straight-line walking proxy has been removed from ranking. Walking duration now comes from the Google matrix with `travelMode=WALK`. The score is:

```text
score_s = driving_duration_s + walking_duration_s
```

Ties are broken by shorter actual walking time, shorter PostGIS distance from the spot to the destination, shorter driving time, and finally a stable ordering by spot ID.

With `N` candidates, the DRIVE matrix has at most `N` elements and the WALK matrix at most another `N`. WALK runs only for spots reachable by car, so the actual count may be lower. The default candidate limit of 25 means at most 50 matrix elements in total, without a separate request per spot.

Google labels walking routes as beta. A successful response includes the `walking_routes_beta` warning; a frontend showing a walking route must tell users that some sidewalks and paths may be missing.

A manual test requires running dependencies, a fresh `free` observation, and a real key enabled for Geocoding API v4 and Routes API:

```bash
curl -i -X POST http://127.0.0.1:3000/v1/parking/search \
  -H 'content-type: application/json' \
  -d '{
    "origin": {"latitude": 52.2297, "longitude": 21.0122},
    "destination_address": "Marszałkowska 10, Warszawa"
  }'
```

The server sends its key only in the `X-Goog-Api-Key` header. Do not put it in a URL or commit `.env` files; Git ignores them.

## Web Frontend

The React/TypeScript frontend lives in `web/`. Routing stays in the backend: the browser renders returned markers and an encoded driving polyline, but it does not call Route Matrix, Compute Routes, or Directions Service.

```text
SearchForm
  -> parkingApi
  -> POST /v1/parking/search
  -> SearchResult
  -> ParkingMap
```

Development setup:

```bash
cd web
npm install
cp .env.example .env.local

export VITE_GOOGLE_MAPS_API_KEY='browser-key'
export VITE_GOOGLE_MAP_ID=DEMO_MAP_ID
export VITE_PARKING_API_BASE_URL=http://127.0.0.1:3000
npm run dev
```

The app will be available at `http://localhost:5173`. Start the backend with:

```bash
export FRONTEND_ORIGIN=http://localhost:5173
```

`GOOGLE_MAPS_API_KEY` is a server-side secret for Geocoding and Routes API; it is never sent to the browser. `VITE_GOOGLE_MAPS_API_KEY` is a public browser key for Maps JavaScript API. Restrict it in Google Cloud to that API and add HTTP referrer restrictions for the actual frontend domains. Do not commit `.env.local` or real keys.

Advanced Markers require a map ID. `DEMO_MAP_ID` is for development only; a deployed frontend should supply its own `VITE_GOOGLE_MAP_ID`. The frontend loads only the `maps`, `marker`, and `geometry` libraries. A missing key or Maps JavaScript API failure does not block the form or text result; the map shows a controlled message instead.

Manual smoke test:

1. Start PostGIS/NATS, the backend with `GOOGLE_MAPS_API_KEY` and `FRONTEND_ORIGIN`, then the frontend with the `VITE_*` settings above.
2. Open `http://localhost:5173`.
3. Choose “Use my location” or enter coordinates manually, provide an address, and search.
4. Check the origin/destination/parking markers, DRIVE route, DRIVE/WALK summary, and `walking_routes_beta` warning.
5. Search again and confirm that the markers and polyline are replaced.

Google walking routes remain in beta. Every client presenting them must show the backend's warning about potentially missing sidewalks and paths.

## Edge Perception for Recorded Video

Stage 7 adds an alternative producer of the same event contract. `camera-simulator` remains available to test the backend without computer vision.

```text
recorded video
      |
GStreamer (RGB)
      |
frame sampling
      |
ONNX ParkingDetectorV1 (CPU)
      |
vehicle detections
      |
parking-perception: normalized ROI occupancy
      |
temporal stabilizer
      |
SpotObservationV1
      |
NATS JetStream
      |
parking-ingestion-worker
      |
PostgreSQL
```

`parking-perception` is a pure library for CV domain logic. It has no dependency on NATS, GStreamer, ONNX Runtime, SQLx, or Axum. `parking-edge-agent` owns the frame source, model adapter, configuration, persistent sequence, and publishing.

### System Dependencies and Startup

GStreamer 0.25.x bindings require Rust 1.92 and GStreamer development libraries. Package names vary by distribution. On Debian/Ubuntu, an example is:

```bash
sudo apt install pkg-config libglib2.0-dev libgstreamer1.0-dev \
  libgstreamer-plugins-base1.0-dev gstreamer1.0-tools \
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-libav
```

On Fedora, the equivalents are typically `pkgconf-pkg-config`, `gstreamer1-devel`, `gstreamer1-plugins-base-devel`, and the required plugin sets. Use your distribution's system packages.

Copy and adapt the example configuration. Do not commit the local config, model, video, or state database:

```bash
cp config/edge-camera.example.toml config/edge-camera.toml
export NATS_URL=nats://127.0.0.1:4222
export PARKING_MODEL_PATH=/absolute/path/to/parking-detector-v1.onnx # optional override
export PARKING_VIDEO_PATH=/absolute/path/to/parking-camera.mp4 # optional override
cargo run -p parking-edge-agent -- --config config/edge-camera.toml
```

Start NATS and `parking-ingestion-worker` before the edge agent; the worker provisions the `PARKING_OBSERVATIONS` stream. The agent publishes to the existing `parking.observations.v1` subject, waits for the JetStream publish ACK, and uses the new `event_id` as `Nats-Msg-Id`. It does not download a model at startup; a missing model file is a clear startup error. CPU is the default and only supported provider for Stage 7. ONNX Runtime is loaded dynamically so builds and tests do not download a platform-specific binary. At runtime, point `ORT_DYLIB_PATH` to a compatible CPU `libonnxruntime.so`; a missing library produces a controlled startup error.

### Occupancy and Stabilization Algorithm

Spot ROIs and bounding boxes use normalized coordinates in `0..=1`. For each detection whose `class_id` is in the configured `vehicle_class_ids` and whose confidence clears the threshold, the engine computes:

```text
spot_overlap_ratio = intersection_area(spot_polygon, vehicle_bbox) / spot_polygon_area
occupancy_score = max(detection.confidence * spot_overlap_ratio)
```

This deliberately is not conventional IoU. A score `<= free_threshold` means `Free`; `>= occupied_threshold` means `Occupied`; the gap between thresholds means `Uncertain`. No matching detection gives score 0, but only for a frame processed successfully.

Inference runs at most once per `sample_interval_ms`. A classification becomes stable after `stable_samples_required` consecutive matching samples. An event is emitted immediately when the stable state changes, or after `observation_refresh_interval_ms` without a change. Validation requires the refresh interval to be shorter than the backend's 15-second TTL; the example uses 5 seconds.

Example values—confidence `0.50`, free `0.10`, occupied `0.30`, 500 ms sampling, and three stable samples—are starting experimental parameters, not scientifically optimal settings. I still need to calibrate them against labeled validation data and the chosen model's label map. The example class IDs `[2, 3, 5, 7]` must likewise match that model's label map.

### ParkingDetectorV1 Model Contract

The adapter does not claim compatibility with an arbitrary raw YOLO export. It accepts a post-NMS model with this exact contract:

```text
input:  float32 [1, 3, H, W], RGB, CHW, values [0,1]
output: float32 [N, 6]
row:    [x_min, y_min, x_max, y_max, confidence, class_id]
bbox:   coordinates normalized to the original frame
```

Preprocessing takes packed RGB from GStreamer, applies explicit nearest-neighbor stretch resizing to `input_width` × `input_height`, normalizes `[0,255] -> [0,1]`, converts HWC to CHW, and adds a batch dimension. Stretching is a deliberate MVP limitation and can distort aspect ratios. Because both input and output axes are normalized independently, bounding boxes need no letterbox transform. Raw YOLO output that requires anchor decoding, tensor mapping, or NMS needs a separate postprocessor. Invalid model boxes, confidence, or class IDs fail the frame's inference without panicking. A valid empty `[0,6]` output is a distinct case.

### Failure and Restart Semantics

- Missing or stopped video does not create `Free` events. Refresh stops, and the backend changes stale state to `UNKNOWN` after its TTL.
- A single inference error is logged without updating the stabilizer. A prolonged error also stops refresh and ultimately produces backend `UNKNOWN`.
- Stable `Uncertain` is explicitly published and mapped to `UNKNOWN` rather than preserving the previous state indefinitely.
- Publishing is retried at most five times, with 100/200/400/800 ms delays before the fifth attempt. A lasting outage is logged; Stage 7 has no offline spool.
- EOF ends the pipeline and closes GStreamer. SIGINT/SIGTERM is handled between operations; a frame wait is capped at 5 seconds, so a stalled source is not mistaken for EOF. Shutdown has an overall 15-second limit.
- A missing or incompatible model file stops startup instead of simulating a healthy agent.

A camera-wide `sequence` is persisted in the local SQLite file named by `state_database`. Each number is allocated and committed before the first publish attempt. After a restart, the next number exceeds every number previously allocated. A failed publish may leave a gap, but numbers are never reused: the backend requires monotonicity, not continuity.

The edge agent publishes no frames, crops, faces, or license plates. Only `SpotObservationV1` occupancy metadata goes to NATS.

The `videotestsrc` GStreamer test is ignored by default because it needs system libraries and plugins. After installing them, run:

```bash
cargo test -p parking-edge-agent gstreamer_pipeline_produces_rgb_frames -- --ignored
```

An optional live smoke test requires a compatible `ParkingDetectorV1` model, a local video, a running worker, and correctly configured ROIs. Large `.onnx` and `.mp4` files, `artifacts/`, local config, and `edge-state.db*` are ignored by Git.

## Tests and Quality Checks

Domain tests, Postgres/NATS integration tests, Google adapter HTTP mocks, frontend tests, perception tests, edge tests, and evaluator tests cover different layers. GStreamer infrastructure tests are ignored by default and run explicitly where system libraries are available. PostGIS and NATS from Compose are required for the full integration path. The automated tests make no paid Google calls and need neither a real ONNX model nor a real video; real-data validation is separate work.

```bash
export DATABASE_URL=postgres://parking:parking@localhost:5432/parking
export NATS_URL=nats://127.0.0.1:4222
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace

cd web
npm run lint
npm test
npm run build
```

## MVP Assumption

I currently assume one authoritative observation source per parking spot at a time. The system does not implement sensor fusion or advanced camera ownership. `sequence` determines event ordering; `observed_at` determines observation freshness only.

## Perception Evaluation

Stage 7.1 uses a separate `perception-evaluator`. It reads previously recorded detections and independent ground truth, while reusing the same `parking-perception` crate for ROI scoring and stabilization as the edge agent. It does not run ONNX or change runtime configuration. Generate `detections.jsonl` once at a fixed sampling cadence, prepare independent `ground_truth.jsonl`, then compare multiple threshold settings against those same inputs. The “Real-Data Evaluation Workflow” below covers detection generation from video.

The `dataset.toml` manifest contains `schema_version = 1`, name, version, description, and `[[videos]]` entries with `video_id`, `camera_id`, `detections_path`, `ground_truth_path`, and `roi_config_path`. Paths are relative to the manifest. The ROI file points to an ordinary edge configuration using the same `[[spots]]`, `polygon`, `vehicle_class_ids`, and refresh interval. Each JSONL row has `schema_version: 1`. A detection row contains `video_id`, `camera_id`, `frame_index`, `timestamp_ms`, and detections with `class_id`, `confidence`, and a normalized bounding box (`x_min`, `y_min`, `x_max`, `y_max`). Ground truth contains `video_id`, `spot_id`, `timestamp_ms`, and a `free`, `occupied`, or `unknown` state. The small synthetic example in `fixtures/perception-eval/` is not a representative parking dataset. Do not commit large videos, frames, or dumps; `datasets/raw/`, `datasets/generated/`, and `evaluation-output/` are ignored.

```bash
cargo run -p perception-evaluator -- evaluate \
  --dataset fixtures/perception-eval/dataset.toml \
  --output evaluation-output/fixture-evaluate \
  --confidence 0.5 --free 0.1 --occupied 0.3 --stable-samples 3

cargo run -p perception-evaluator -- sweep \
  --dataset fixtures/perception-eval/dataset.toml \
  --output evaluation-output/fixture-sweep \
  --confidence-values 0.4,0.5,0.6 \
  --free-values 0.05,0.10,0.15 \
  --occupied-values 0.25,0.30,0.35 \
  --stable-samples-values 2,3,4
```

`--ground-truth-tolerance-ms` defaults to 250, and `--transition-timeout-ms` to 10000. Labels are matched by `video_id + spot_id + nearest timestamp`, breaking ties with the earlier annotation. Unlabeled and `unknown` samples are reported but excluded from classification. `Unknown` breaks a run of known labels when detecting transitions, so a change across an unknown gap has no exact latency. Initial samples without a stable state are scored as `UNCERTAIN` predictions.

`results.csv` has one row per valid configuration with separate `frame_*` and stabilized-state metrics. `per_camera.csv` breaks results down by camera. `summary.json` contains the schema version, class balance, frame/stabilized metrics for an evaluation or selected sweep configuration, per-camera metrics, input SHA-256 hashes, and a count of skipped combinations. `evaluation-config.json` records every parameter. For reproducibility, `evaluation_timestamp` defaults to `null`; pass an explicit RFC3339 time with `--evaluation-timestamp 2026-01-01T00:00:00Z` if needed.

I prioritize false-free errors (an occupied spot shown as free), coverage, both classes' F1 scores, and transition latency. `UNCERTAIN` lowers coverage but a wider uncertainty band may prevent costly false decisions. Strict accuracy is secondary and can mislead on imbalanced data. Without constraints, the tool does not name a winner; `--max-false-free-rate` and/or `--min-coverage` enable an explicit choice of the highest macro F1 among configurations that satisfy the limits. I do not impose those limits in advance.

A sweep result applies only to its dataset and detection cadence; it is not a globally optimal threshold set. Tuning and reporting on the same data would inflate the evaluation. I ultimately need separate calibration/validation data and a held-out test set.

## Real-Data Evaluation Workflow

Stage 7.2A connects existing components:

```text
recorded video -> GStreamer -> frame sampling -> existing OnnxDetector
               -> raw Detection[] -> detections.jsonl -> perception-evaluator
```

1. Prepare a model that matches the `ParkingDetectorV1` contract above **exactly** (float32 `[1,3,H,W]` input, RGB/CHW/stretch, post-NMS float32 `[N,6]` output). An arbitrary YOLO ONNX export is not enough.
2. Prepare a local recording. The model and video may live outside the repository. You also need a local CPU ONNX Runtime library selected by `ORT_DYLIB_PATH`.
3. Generate detections once:

```bash
export ORT_DYLIB_PATH=/path/to/libonnxruntime.so
cargo run -p perception-evaluator -- generate-detections \
  --video /path/to/recording.mp4 \
  --model /path/to/parking-detector-v1.onnx \
  --camera-id 11111111-1111-1111-1111-111111111111 \
  --video-id real-run-001 \
  --sample-interval-ms 500 \
  --input-width 640 --input-height 640 \
  --output datasets/real-run-001/detections.jsonl
```

You may use `--config config/edge-camera.toml` instead of separate paths and parameters. `--video-id` and `--output` are still required. Precedence is explicit CLI flags > `PARKING_MODEL_PATH`/`PARKING_VIDEO_PATH` > edge config; without a config, sampling defaults to 500 ms and input dimensions to 640×640. `--detector-output-floor` defaults to 0.0 and is not the runtime `detection_confidence_threshold`. The generator does not filter `vehicle_class_ids`; it retains every class and low-confidence detection so a later sweep needs no repeat inference. Overwriting is refused by default; `--force` permits it explicitly.

4. Prepare independent `ground_truth.jsonl` labels at spot-and-timestamp level, manually or through an external process. The generator does not create ground truth.
5. Create or update a Stage 7.1 `dataset.toml` pointing to the generated JSONL, ground truth, and the same ROI configuration.
6. Run `evaluate`, then a separate `sweep` using the earlier examples.
7. Analyze false-free rate, coverage, both F1 scores, and transition latency; do not choose thresholds using accuracy alone.

Suggested layout:

```text
datasets/real-run-001/
├── dataset.toml
├── detections.jsonl
├── detections-metadata.json
└── ground_truth.jsonl
```

Each JSONL record has `schema_version: 1`. `frame_index` counts **saved samples** from 0, not source-video frames. `timestamp_ms` comes from GStreamer PTS (video presentation time), not wall time. Missing or decreasing PTS is an error; there is no wall-clock fallback. With a deterministic model, the same video and sampling interval produce the same records. `detections-metadata.json` records the model and video names and SHA-256 hashes, contract, input size, cadence, output floor, generation time, and record count. `generated_at` and execution time naturally vary between runs; `detections.jsonl` is the artifact intended for comparison and reevaluation.

Output streams to `detections.jsonl.tmp`. Only a successful EOF, flush, metadata write, and rename publish the final file. Missing model/video/ORT, an incompatible model contract, inference failure, invalid model output, or a write error fails generation rather than presenting an incomplete dataset as complete. This research workflow is intentionally **fail-fast**, unlike the edge runtime, which can skip a bad frame. The generator does not connect to NATS or PostgreSQL, use SQLite sequences, publish `SpotObservationV1`, or automatically run `evaluate`.

`detections.jsonl` describes object presence and location, not raw frames, crops, faces, or license plates. Artifacts are not automatically uploaded to the cloud; large data and models stay out of Git. Fake-source/fake-detector and synthetic GStreamer tests exercise the pipeline without a real model or recording. Stage 7.2A prepares a real-data workflow but does **not** validate a particular model's quality or correctness on real footage.

Synthetic GStreamer → fake detector → JSONL test (requires GStreamer libraries and plugins; ignored by default):

```bash
cargo test -p perception-evaluator gstreamer_synthetic_source_generates_jsonl -- --ignored
```
