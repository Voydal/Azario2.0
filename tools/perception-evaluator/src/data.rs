use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs::{self, File},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

use parking_perception::{BoundingBox, Detection, ParkingSpotRoi, SpotConfig};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub type AnyError = Box<dyn Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, AnyError>;

#[derive(Debug, Deserialize, Serialize)]
pub struct DatasetManifest {
    pub schema_version: u32,
    pub dataset_name: String,
    pub version: String,
    pub description: String,
    #[serde(default)]
    pub model_name: Option<String>,
    #[serde(default)]
    pub model_version: Option<String>,
    #[serde(default)]
    pub model_hash: Option<String>,
    pub videos: Vec<VideoEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct VideoEntry {
    pub video_id: String,
    pub camera_id: Uuid,
    pub detections_path: PathBuf,
    pub ground_truth_path: PathBuf,
    pub roi_config_path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct RoiConfig {
    camera_id: Uuid,
    vehicle_class_ids: Vec<u32>,
    observation_refresh_interval_ms: u64,
    spots: Vec<SpotConfig>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct BoxRecord {
    pub x_min: f64,
    pub y_min: f64,
    pub x_max: f64,
    pub y_max: f64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DetectionEntry {
    pub class_id: u32,
    pub confidence: f32,
    pub bbox: BoxRecord,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DetectionRecord {
    pub schema_version: u32,
    pub video_id: String,
    pub camera_id: Uuid,
    pub frame_index: u64,
    pub timestamp_ms: i64,
    pub detections: Vec<DetectionEntry>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Truth {
    Free,
    Occupied,
    Unknown,
}

#[derive(Debug, Deserialize)]
struct TruthRecord {
    schema_version: u32,
    video_id: String,
    spot_id: Uuid,
    timestamp_ms: i64,
    state: Truth,
}

#[derive(Clone, Debug)]
pub struct Sample {
    pub video_id: String,
    pub camera_id: Uuid,
    pub spot_id: Uuid,
    pub frame_index: u64,
    pub timestamp_ms: i64,
    pub detections: Vec<Detection>,
    pub truth: Option<Truth>,
}

pub struct Dataset {
    pub manifest: DatasetManifest,
    pub samples: Vec<Sample>,
    pub truth_records: BTreeMap<(String, Uuid), Vec<(i64, Truth)>>,
    pub rois: BTreeMap<(String, Uuid), ParkingSpotRoi>,
    pub vehicle_class_ids: Vec<u32>,
    pub refresh_interval_ms: u64,
    pub hashes: BTreeMap<String, String>,
    pub input_was_reordered: bool,
    pub video_count: usize,
    pub spot_count: usize,
}

fn err(message: impl Into<String>) -> AnyError {
    message.into().into()
}

fn read_jsonl<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Vec<T>> {
    let file = File::open(path).map_err(|e| err(format!("{}: {e}", path.display())))?;
    BufReader::new(file)
        .lines()
        .enumerate()
        .filter_map(|(index, line)| match line {
            Ok(line) if line.trim().is_empty() => None,
            other => Some((index + 1, other)),
        })
        .map(|(line_number, line)| {
            let line = line.map_err(|e| err(format!("{}:{line_number}: {e}", path.display())))?;
            serde_json::from_str(&line)
                .map_err(|e| err(format!("{}:{line_number}: {e}", path.display())))
        })
        .collect()
}

fn resolve(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn hash_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub fn load(path: &Path, tolerance_ms: i64) -> Result<Dataset> {
    if tolerance_ms < 0 {
        return Err(err("ground_truth_tolerance_ms must be non-negative"));
    }
    let manifest: DatasetManifest = toml::from_str(&fs::read_to_string(path)?)?;
    if manifest.schema_version != 1 || manifest.version.trim().is_empty() {
        return Err(err(
            "dataset manifest requires schema_version = 1 and non-empty version",
        ));
    }
    if manifest.videos.is_empty() {
        return Err(err("dataset manifest has no videos"));
    }
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let mut hashes = BTreeMap::new();
    hashes.insert(path.display().to_string(), hash_file(path)?);
    let mut samples = Vec::new();
    let mut truth_records = BTreeMap::new();
    let mut rois = BTreeMap::new();
    let mut video_ids = BTreeSet::new();
    let mut vehicle_class_ids: Option<Vec<u32>> = None;
    let mut refresh_interval_ms: Option<u64> = None;
    let mut reordered = false;

    for video in &manifest.videos {
        if !video_ids.insert(video.video_id.clone()) {
            return Err(err(format!("duplicate video_id: {}", video.video_id)));
        }
        let detections_path = resolve(base, &video.detections_path);
        let ground_truth_path = resolve(base, &video.ground_truth_path);
        let roi_path = resolve(base, &video.roi_config_path);
        for file in [&detections_path, &ground_truth_path, &roi_path] {
            hashes.insert(file.display().to_string(), hash_file(file)?);
        }
        let roi_config: RoiConfig = toml::from_str(&fs::read_to_string(&roi_path)?)?;
        if roi_config.camera_id != video.camera_id {
            return Err(err(format!(
                "camera_id differs between manifest and {}",
                roi_path.display()
            )));
        }
        if roi_config.vehicle_class_ids.is_empty()
            || roi_config.observation_refresh_interval_ms == 0
        {
            return Err(err(format!(
                "invalid vehicle classes or refresh interval in {}",
                roi_path.display()
            )));
        }
        if let Some(existing) = &vehicle_class_ids {
            if *existing != roi_config.vehicle_class_ids {
                return Err(err("all ROI configs must use the same vehicle_class_ids"));
            }
        } else {
            vehicle_class_ids = Some(roi_config.vehicle_class_ids.clone());
        }
        if let Some(existing) = refresh_interval_ms {
            if existing != roi_config.observation_refresh_interval_ms {
                return Err(err(
                    "all ROI configs must use the same observation_refresh_interval_ms",
                ));
            }
        } else {
            refresh_interval_ms = Some(roi_config.observation_refresh_interval_ms);
        }
        if roi_config.spots.is_empty() {
            return Err(err(format!("no spots in {}", roi_path.display())));
        }
        let mut spot_ids = BTreeSet::new();
        for spot in roi_config.spots {
            if !spot_ids.insert(spot.spot_id) {
                return Err(err(format!(
                    "duplicate spot_id {} in {}",
                    spot.spot_id,
                    roi_path.display()
                )));
            }
            let roi = spot
                .roi()
                .map_err(|e| err(format!("{}: {e}", roi_path.display())))?;
            rois.insert((video.video_id.clone(), spot.spot_id), roi);
        }

        let mut truths: Vec<TruthRecord> = read_jsonl(&ground_truth_path)?;
        for truth in &truths {
            if truth.schema_version != 1
                || truth.video_id != video.video_id
                || !spot_ids.contains(&truth.spot_id)
                || truth.timestamp_ms < 0
            {
                return Err(err(format!(
                    "invalid ground truth record in {}",
                    ground_truth_path.display()
                )));
            }
        }
        truths.sort_by_key(|r| (r.spot_id, r.timestamp_ms));
        for truth in truths {
            let entries = truth_records
                .entry((video.video_id.clone(), truth.spot_id))
                .or_insert_with(Vec::new);
            if entries
                .last()
                .is_some_and(|(t, _)| *t == truth.timestamp_ms)
            {
                return Err(err(format!(
                    "duplicate ground truth timestamp for spot {}",
                    truth.spot_id
                )));
            }
            entries.push((truth.timestamp_ms, truth.state));
        }

        let records: Vec<DetectionRecord> = read_jsonl(&detections_path)?;
        let mut seen = BTreeSet::new();
        let mut previous_key: Option<(i64, u64)> = None;
        for record in records {
            if record.schema_version != 1
                || record.video_id != video.video_id
                || record.camera_id != video.camera_id
                || record.timestamp_ms < 0
            {
                return Err(err(format!(
                    "invalid detection record in {}",
                    detections_path.display()
                )));
            }
            if !seen.insert((record.frame_index, record.timestamp_ms)) {
                return Err(err(format!(
                    "duplicate frame_index and timestamp in {}",
                    detections_path.display()
                )));
            }
            let key = (record.timestamp_ms, record.frame_index);
            if previous_key.is_some_and(|last| key < last) {
                reordered = true;
            }
            previous_key = Some(key);
            let detections = record
                .detections
                .into_iter()
                .map(|d| {
                    if !d.confidence.is_finite() || !(0.0..=1.0).contains(&d.confidence) {
                        return Err(err("detection confidence must be finite in 0..=1"));
                    }
                    let bbox =
                        BoundingBox::new(d.bbox.x_min, d.bbox.y_min, d.bbox.x_max, d.bbox.y_max)?;
                    Ok(Detection {
                        class_id: d.class_id,
                        confidence: d.confidence,
                        bbox,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            for spot_id in &spot_ids {
                let truth = truth_records
                    .get(&(video.video_id.clone(), *spot_id))
                    .and_then(|entries| nearest_truth(entries, record.timestamp_ms, tolerance_ms));
                samples.push(Sample {
                    video_id: video.video_id.clone(),
                    camera_id: video.camera_id,
                    spot_id: *spot_id,
                    frame_index: record.frame_index,
                    timestamp_ms: record.timestamp_ms,
                    detections: detections.clone(),
                    truth,
                });
            }
        }
    }
    samples.sort_by_key(|s| (s.video_id.clone(), s.spot_id, s.timestamp_ms, s.frame_index));
    let spot_count = rois.len();
    Ok(Dataset {
        manifest,
        samples,
        truth_records,
        rois,
        vehicle_class_ids: vehicle_class_ids.unwrap_or_default(),
        refresh_interval_ms: refresh_interval_ms.unwrap_or(1),
        hashes,
        input_was_reordered: reordered,
        video_count: video_ids.len(),
        spot_count,
    })
}

fn nearest_truth(entries: &[(i64, Truth)], timestamp_ms: i64, tolerance_ms: i64) -> Option<Truth> {
    let position = entries.partition_point(|(time, _)| *time < timestamp_ms);
    let candidates = [position.checked_sub(1), Some(position)];
    candidates
        .into_iter()
        .flatten()
        .filter_map(|index| entries.get(index))
        .filter(|(time, _)| time.abs_diff(timestamp_ms) <= tolerance_ms as u64)
        .min_by_key(|(time, _)| (time.abs_diff(timestamp_ms), *time))
        .map(|(_, truth)| *truth)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_prefers_nearest_then_earlier() {
        let values = [(100, Truth::Free), (200, Truth::Occupied)];
        assert_eq!(nearest_truth(&values, 150, 50), Some(Truth::Free));
        assert_eq!(nearest_truth(&values, 175, 50), Some(Truth::Occupied));
        assert_eq!(nearest_truth(&values, 260, 50), None);
    }
}
