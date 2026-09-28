use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{BufWriter, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use chrono::{TimeZone, Utc};
use clap::Args;
use parking_edge_agent::{
    config::EdgeConfig,
    frame_source::{FrameSource, GstreamerFileSource},
    onnx::OnnxDetector,
    sampling::FrameSampler,
};
use parking_perception::Detector;
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::data::{BoxRecord, DetectionEntry, DetectionRecord, Result};

#[derive(Args)]
pub struct GenerateArgs {
    /// Reuse edge-camera.toml; explicit CLI values override it.
    #[arg(long)]
    pub config: Option<PathBuf>,
    #[arg(long)]
    pub video: Option<PathBuf>,
    #[arg(long)]
    pub model: Option<PathBuf>,
    #[arg(long)]
    pub camera_id: Option<Uuid>,
    #[arg(long)]
    pub video_id: String,
    #[arg(long)]
    pub sample_interval_ms: Option<u64>,
    #[arg(long)]
    pub input_width: Option<u32>,
    #[arg(long)]
    pub input_height: Option<u32>,
    #[arg(long, default_value_t = 0.0)]
    pub detector_output_floor: f32,
    #[arg(long)]
    pub output: PathBuf,
    #[arg(long)]
    pub force: bool,
}

#[derive(Clone, Debug)]
pub struct GenerationConfig {
    pub video_path: PathBuf,
    pub model_path: PathBuf,
    pub camera_id: Uuid,
    pub video_id: String,
    pub sample_interval_ms: u64,
    pub input_width: u32,
    pub input_height: u32,
    pub detector_output_floor: f32,
    pub output_path: PathBuf,
    pub force: bool,
}

impl GenerationConfig {
    pub fn from_args(args: GenerateArgs) -> Result<Self> {
        let edge = args.config.as_deref().map(EdgeConfig::load).transpose()?;
        let video_path = args
            .video
            .or_else(|| env::var_os("PARKING_VIDEO_PATH").map(PathBuf::from))
            .or_else(|| edge.as_ref().map(|config| config.video_file.clone()))
            .ok_or("provide --video, --config, or PARKING_VIDEO_PATH")?;
        let model_path = args
            .model
            .or_else(|| env::var_os("PARKING_MODEL_PATH").map(PathBuf::from))
            .or_else(|| edge.as_ref().map(|config| config.model_path.clone()))
            .ok_or("provide --model, --config, or PARKING_MODEL_PATH")?;
        let camera_id = args
            .camera_id
            .or_else(|| edge.as_ref().map(|config| config.camera_id))
            .ok_or("provide --camera-id or --config")?;
        let config = Self {
            video_path,
            model_path,
            camera_id,
            video_id: args.video_id,
            sample_interval_ms: args
                .sample_interval_ms
                .or_else(|| edge.as_ref().map(|config| config.sample_interval_ms))
                .unwrap_or(500),
            input_width: args
                .input_width
                .or_else(|| edge.as_ref().map(|config| config.input_width))
                .unwrap_or(640),
            input_height: args
                .input_height
                .or_else(|| edge.as_ref().map(|config| config.input_height))
                .unwrap_or(640),
            detector_output_floor: args.detector_output_floor,
            output_path: args.output,
            force: args.force,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.video_id.trim().is_empty() {
            return Err("video_id must not be empty".into());
        }
        if self.sample_interval_ms == 0 || self.sample_interval_ms > i64::MAX as u64 {
            return Err("sample_interval_ms must be positive and fit in i64".into());
        }
        if self.input_width == 0 || self.input_height == 0 {
            return Err("input dimensions must be positive".into());
        }
        if !self.detector_output_floor.is_finite()
            || !(0.0..=1.0).contains(&self.detector_output_floor)
        {
            return Err("detector_output_floor must be finite in 0..=1".into());
        }
        if self
            .output_path
            .extension()
            .is_none_or(|extension| extension != "jsonl")
        {
            return Err("output path must end in .jsonl".into());
        }
        if self.output_path == self.video_path || self.output_path == self.model_path {
            return Err("output path must differ from video and model inputs".into());
        }
        if !self.video_path.is_file() {
            return Err(format!("video file does not exist: {}", self.video_path.display()).into());
        }
        if !self.model_path.is_file() {
            return Err(format!("model file does not exist: {}", self.model_path.display()).into());
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
pub struct DetectionMetadata {
    pub schema_version: u32,
    pub video_id: String,
    pub camera_id: Uuid,
    pub model_filename: String,
    pub model_sha256: String,
    pub video_filename: String,
    pub video_sha256: String,
    pub model_contract: &'static str,
    pub sample_interval_ms: u64,
    pub detector_output_floor: f32,
    pub input_width: u32,
    pub input_height: u32,
    pub generated_at: String,
    pub detection_records: u64,
    pub wall_clock_generation_duration_ms: u128,
}

fn filename(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let size = file.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        hasher.update(&buffer[..size]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn metadata(config: &GenerationConfig, count: u64, started: Instant) -> Result<DetectionMetadata> {
    Ok(DetectionMetadata {
        schema_version: 1,
        video_id: config.video_id.clone(),
        camera_id: config.camera_id,
        model_filename: filename(&config.model_path),
        model_sha256: sha256_file(&config.model_path)?,
        video_filename: filename(&config.video_path),
        video_sha256: sha256_file(&config.video_path)?,
        model_contract: "ParkingDetectorV1",
        sample_interval_ms: config.sample_interval_ms,
        detector_output_floor: config.detector_output_floor,
        input_width: config.input_width,
        input_height: config.input_height,
        generated_at: Utc::now().to_rfc3339(),
        detection_records: count,
        wall_clock_generation_duration_ms: started.elapsed().as_millis(),
    })
}

pub fn generate_records<S: FrameSource, D: Detector, W: Write>(
    source: &mut S,
    detector: &mut D,
    writer: &mut W,
    config: &GenerationConfig,
) -> Result<u64> {
    let mut sampler = FrameSampler::new(Duration::from_millis(config.sample_interval_ms));
    let mut count = 0_u64;
    let mut last_pts = None;
    while let Some(frame) = source.next_frame()? {
        let pts = frame
            .video_timestamp_ms
            .ok_or("recorded-video frame has no GStreamer PTS; refusing wall-clock fallback")?;
        if last_pts.is_some_and(|previous| pts < previous) {
            return Err("non-monotonic GStreamer PTS in recorded video".into());
        }
        last_pts = Some(pts);
        let timestamp_ms = i64::try_from(pts).map_err(|_| "video PTS exceeds i64 milliseconds")?;
        let sample_time = Utc
            .timestamp_millis_opt(timestamp_ms)
            .single()
            .ok_or("video PTS outside supported timestamp range")?;
        if !sampler.should_sample(sample_time) {
            continue;
        }
        let detections = detector.detect(&frame)?;
        if detections.iter().any(|detection| {
            !detection.confidence.is_finite() || !(0.0..=1.0).contains(&detection.confidence)
        }) {
            return Err("detector returned invalid confidence".into());
        }
        let entries = detections
            .into_iter()
            .filter(|detection| detection.confidence >= config.detector_output_floor)
            .map(|detection| {
                let [x_min, y_min, x_max, y_max] = detection.bbox.coordinates();
                DetectionEntry {
                    class_id: detection.class_id,
                    confidence: detection.confidence,
                    bbox: BoxRecord {
                        x_min,
                        y_min,
                        x_max,
                        y_max,
                    },
                }
            })
            .collect();
        let record = DetectionRecord {
            schema_version: 1,
            video_id: config.video_id.clone(),
            camera_id: config.camera_id,
            frame_index: count,
            timestamp_ms,
            detections: entries,
        };
        serde_json::to_writer(&mut *writer, &record)?;
        writer.write_all(b"\n")?;
        count = count.checked_add(1).ok_or("sample count overflow")?;
        if count.is_multiple_of(100) {
            eprintln!("generate-detections: {count} sampled frames written");
        }
    }
    source.close()?;
    Ok(count)
}

struct TemporaryFiles {
    paths: Vec<PathBuf>,
}
impl Drop for TemporaryFiles {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = fs::remove_file(path);
        }
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".tmp");
    PathBuf::from(name)
}

fn metadata_path(output: &Path) -> PathBuf {
    output.with_file_name("detections-metadata.json")
}

pub fn write_artifacts<S: FrameSource, D: Detector>(
    source: &mut S,
    detector: &mut D,
    config: &GenerationConfig,
) -> Result<DetectionMetadata> {
    config.validate()?;
    let final_data = &config.output_path;
    let final_metadata = metadata_path(final_data);
    if !config.force && (final_data.exists() || final_metadata.exists()) {
        return Err(
            "output or detections-metadata.json already exists; use --force to replace".into(),
        );
    }
    if let Some(parent) = final_data.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let temp_data = temporary_path(final_data);
    let temp_metadata = temporary_path(&final_metadata);
    let mut cleanup = TemporaryFiles {
        paths: vec![temp_data.clone(), temp_metadata.clone()],
    };
    let data_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_data)?;
    let started = Instant::now();
    let mut writer = BufWriter::new(data_file);
    let count = generate_records(source, detector, &mut writer, config)?;
    writer.flush()?;
    writer.into_inner()?.sync_all()?;

    let run_metadata = metadata(config, count, started)?;
    let metadata_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_metadata)?;
    let mut metadata_writer = BufWriter::new(metadata_file);
    serde_json::to_writer_pretty(&mut metadata_writer, &run_metadata)?;
    metadata_writer.write_all(b"\n")?;
    metadata_writer.flush()?;
    metadata_writer.into_inner()?.sync_all()?;

    if !config.force && (final_data.exists() || final_metadata.exists()) {
        return Err("output appeared during generation; refusing overwrite".into());
    }
    fs::rename(&temp_metadata, &final_metadata)?;
    cleanup.paths.retain(|path| path != &temp_metadata);
    if let Err(error) = fs::rename(&temp_data, final_data) {
        let _ = fs::remove_file(&final_metadata);
        return Err(error.into());
    }
    cleanup.paths.clear();
    Ok(run_metadata)
}

pub fn run(args: GenerateArgs) -> Result<()> {
    let config = GenerationConfig::from_args(args)?;
    let metadata_file = metadata_path(&config.output_path);
    if !config.force && (config.output_path.exists() || metadata_file.exists()) {
        return Err("output or metadata already exists; use --force to replace".into());
    }
    eprintln!("generate-detections: starting video_id={}", config.video_id);
    let mut detector =
        OnnxDetector::load(&config.model_path, config.input_width, config.input_height).map_err(
            |error| format!("cannot load ParkingDetectorV1 model (check ORT_DYLIB_PATH): {error}"),
        )?;
    eprintln!("generate-detections: model loaded");
    let mut source = GstreamerFileSource::open(&config.video_path)?;
    eprintln!("generate-detections: video opened");
    let result = write_artifacts(&mut source, &mut detector, &config)?;
    eprintln!(
        "generate-detections: finished; {} records written",
        result.detection_records
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use chrono::Utc;
    use parking_edge_agent::frame_source::FrameSourceError;
    use parking_perception::{BoundingBox, Detection, DetectionError, Frame};
    use tempfile::TempDir;

    use super::*;

    const CAMERA_ID: &str = "11111111-1111-1111-1111-111111111111";

    struct FakeSource {
        frames: Vec<Frame>,
    }

    impl FrameSource for FakeSource {
        fn next_frame(&mut self) -> std::result::Result<Option<Frame>, FrameSourceError> {
            if self.frames.is_empty() {
                Ok(None)
            } else {
                Ok(Some(self.frames.remove(0)))
            }
        }

        fn close(&mut self) -> std::result::Result<(), FrameSourceError> {
            Ok(())
        }
    }

    struct FakeDetector {
        detections: Vec<Detection>,
        fail: bool,
    }

    impl Detector for FakeDetector {
        fn detect(
            &mut self,
            _frame: &Frame,
        ) -> std::result::Result<Vec<Detection>, DetectionError> {
            if self.fail {
                Err(DetectionError::new("synthetic inference failure"))
            } else {
                Ok(self.detections.clone())
            }
        }
    }

    fn frame(pts: Option<u64>) -> Frame {
        let mut frame = Frame::rgb(2, 2, vec![0; 12], Utc::now());
        frame.video_timestamp_ms = pts;
        frame
    }

    fn source() -> FakeSource {
        FakeSource {
            frames: vec![
                frame(Some(0)),
                frame(Some(250)),
                frame(Some(500)),
                frame(Some(1000)),
            ],
        }
    }

    fn detector() -> FakeDetector {
        FakeDetector {
            detections: vec![
                Detection {
                    class_id: 2,
                    confidence: 0.9,
                    bbox: BoundingBox::new(0.1, 0.1, 0.4, 0.4).unwrap(),
                },
                Detection {
                    class_id: 17,
                    confidence: 0.15,
                    bbox: BoundingBox::new(0.6, 0.1, 0.9, 0.4).unwrap(),
                },
            ],
            fail: false,
        }
    }

    fn config(directory: &TempDir) -> GenerationConfig {
        let model_path = directory.path().join("fixture.onnx");
        let video_path = directory.path().join("fixture.mp4");
        fs::write(&model_path, b"fake-model-hash-input").unwrap();
        fs::write(&video_path, b"fake-video-hash-input").unwrap();
        GenerationConfig {
            video_path,
            model_path,
            camera_id: CAMERA_ID.parse().unwrap(),
            video_id: "synthetic-01".into(),
            sample_interval_ms: 500,
            input_width: 640,
            input_height: 640,
            detector_output_floor: 0.0,
            output_path: directory.path().join("detections.jsonl"),
            force: false,
        }
    }

    fn records(bytes: &[u8]) -> Vec<DetectionRecord> {
        std::str::from_utf8(bytes)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn generation_writes_one_jsonl_record_per_sample() {
        let directory = tempfile::tempdir().unwrap();
        let config = config(&directory);
        let mut bytes = Vec::new();
        let count = generate_records(&mut source(), &mut detector(), &mut bytes, &config).unwrap();
        let records = records(&bytes);
        assert_eq!(count, 3);
        assert_eq!(records.len(), 3);
        assert_eq!(
            records.iter().map(|r| r.frame_index).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(
            records.iter().map(|r| r.timestamp_ms).collect::<Vec<_>>(),
            vec![0, 500, 1000]
        );
    }

    #[test]
    fn generation_writes_reproducibility_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let config = config(&directory);
        let metadata = write_artifacts(&mut source(), &mut detector(), &config).unwrap();
        assert_eq!(metadata.schema_version, 1);
        assert_eq!(metadata.video_id, "synthetic-01");
        assert_eq!(metadata.camera_id, config.camera_id);
        assert_eq!(metadata.sample_interval_ms, 500);
        assert_eq!(metadata.detection_records, 3);
        assert_eq!(metadata.model_sha256.len(), 64);
        assert_eq!(metadata.video_sha256.len(), 64);
        assert_eq!(metadata.model_contract, "ParkingDetectorV1");
        assert!(config.output_path.exists());
        assert!(directory.path().join("detections-metadata.json").exists());
    }

    #[test]
    fn generation_does_not_filter_non_vehicle_classes() {
        let directory = tempfile::tempdir().unwrap();
        let config = config(&directory);
        let mut bytes = Vec::new();
        generate_records(&mut source(), &mut detector(), &mut bytes, &config).unwrap();
        assert_eq!(records(&bytes)[0].detections[1].class_id, 17);
    }

    #[test]
    fn generation_preserves_low_confidence_detections() {
        let directory = tempfile::tempdir().unwrap();
        let config = config(&directory);
        let mut bytes = Vec::new();
        generate_records(&mut source(), &mut detector(), &mut bytes, &config).unwrap();
        assert_eq!(records(&bytes)[0].detections[1].confidence, 0.15);
    }

    #[test]
    fn generation_fails_on_inference_error() {
        let directory = tempfile::tempdir().unwrap();
        let config = config(&directory);
        let mut bad = detector();
        bad.fail = true;
        assert!(write_artifacts(&mut source(), &mut bad, &config).is_err());
        assert!(!config.output_path.exists());
        assert!(!temporary_path(&config.output_path).exists());
        assert!(!directory.path().join("detections-metadata.json").exists());
    }

    #[test]
    fn existing_output_is_not_overwritten_without_force() {
        let directory = tempfile::tempdir().unwrap();
        let config = config(&directory);
        fs::write(&config.output_path, b"prior-data").unwrap();
        assert!(write_artifacts(&mut source(), &mut detector(), &config).is_err());
        assert_eq!(fs::read(&config.output_path).unwrap(), b"prior-data");
    }

    #[test]
    fn same_fake_frames_produce_same_timestamps() {
        let directory = tempfile::tempdir().unwrap();
        let config = config(&directory);
        let mut first = Vec::new();
        let mut second = Vec::new();
        generate_records(&mut source(), &mut detector(), &mut first, &config).unwrap();
        generate_records(&mut source(), &mut detector(), &mut second, &config).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn missing_pts_is_rejected_without_wall_clock_fallback() {
        let directory = tempfile::tempdir().unwrap();
        let config = config(&directory);
        let mut bytes = Vec::new();
        let mut source = FakeSource {
            frames: vec![frame(None)],
        };
        assert!(generate_records(&mut source, &mut detector(), &mut bytes, &config).is_err());
        assert!(bytes.is_empty());
    }

    fn generate_compatible_dataset() {
        let directory = tempfile::tempdir().unwrap();
        let config = config(&directory);
        write_artifacts(&mut source(), &mut detector(), &config).unwrap();
        fs::write(
            directory.path().join("dataset.toml"),
            include_str!("../../../fixtures/perception-eval/dataset.toml"),
        )
        .unwrap();
        fs::write(
            directory.path().join("ground_truth.jsonl"),
            include_str!("../../../fixtures/perception-eval/ground_truth.jsonl"),
        )
        .unwrap();
        fs::write(
            directory.path().join("edge-camera.toml"),
            include_str!("../../../fixtures/perception-eval/edge-camera.toml"),
        )
        .unwrap();
        let loaded = crate::data::load(&directory.path().join("dataset.toml"), 250).unwrap();
        assert_eq!(loaded.video_count, 1);
        assert_eq!(loaded.samples.len(), 6);
    }

    #[test]
    fn generated_jsonl_is_accepted_by_perception_evaluator_format() {
        generate_compatible_dataset();
    }

    #[test]
    fn fake_source_fake_detector_generate_valid_dataset() {
        generate_compatible_dataset();
    }

    #[test]
    #[ignore = "requires native GStreamer and videotestsrc/videoconvert plugins"]
    fn gstreamer_synthetic_source_generates_jsonl() {
        let directory = tempfile::tempdir().unwrap();
        let config = config(&directory);
        let mut source = GstreamerFileSource::test_pattern(64, 48).unwrap();
        let mut bytes = Vec::new();
        let count = generate_records(&mut source, &mut detector(), &mut bytes, &config).unwrap();
        assert_eq!(count, 1);
        let parsed = records(&bytes);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].frame_index, 0);
        assert_eq!(parsed[0].schema_version, 1);
    }
}
