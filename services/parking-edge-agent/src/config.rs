use std::{
    collections::HashSet,
    env,
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
    time::Duration as StdDuration,
};

use chrono::Duration;
use parking_domain::CameraId;
use parking_perception::{OccupancyConfig, SpotConfig, SpotStabilizer};
use serde::Deserialize;
use uuid::Uuid;

const BACKEND_OBSERVATION_TTL_MS: u64 = 15_000;

#[derive(Clone, Debug, Deserialize)]
pub struct EdgeConfig {
    pub camera_id: Uuid,
    pub video_file: PathBuf,
    pub sample_interval_ms: u64,
    pub observation_refresh_interval_ms: u64,
    pub stable_samples_required: u32,
    pub model_path: PathBuf,
    pub model_version: String,
    pub input_width: u32,
    pub input_height: u32,
    pub vehicle_class_ids: Vec<u32>,
    pub detection_confidence_threshold: f32,
    pub free_threshold: f32,
    pub occupied_threshold: f32,
    #[serde(default = "default_state_database")]
    pub state_database: PathBuf,
    pub spots: Vec<SpotConfig>,
}

pub use parking_perception::PointConfig;

fn default_state_database() -> PathBuf {
    PathBuf::from("edge-state.db")
}

impl EdgeConfig {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let contents = fs::read_to_string(path).map_err(|error| {
            ConfigError::new(format!("cannot read {}: {error}", path.display()))
        })?;
        let mut config: Self = toml::from_str(&contents)
            .map_err(|error| ConfigError::new(format!("invalid edge config: {error}")))?;
        if let Some(model_path) = env::var_os("PARKING_MODEL_PATH") {
            if let Some(video_path) = env::var_os("PARKING_VIDEO_PATH") {
                config.video_file = PathBuf::from(video_path);
            }
            config.model_path = PathBuf::from(model_path);
        }
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.sample_interval_ms == 0 {
            return Err(ConfigError::new("sample_interval_ms must be positive"));
        }
        if self.observation_refresh_interval_ms == 0
            || self.observation_refresh_interval_ms >= BACKEND_OBSERVATION_TTL_MS
        {
            return Err(ConfigError::new(
                "observation_refresh_interval_ms must be positive and below backend TTL (15000 ms)",
            ));
        }
        if self.stable_samples_required == 0 {
            return Err(ConfigError::new("stable_samples_required must be positive"));
        }
        if self.input_width == 0 || self.input_height == 0 {
            return Err(ConfigError::new("model input dimensions must be positive"));
        }
        if self.vehicle_class_ids.is_empty() {
            return Err(ConfigError::new("vehicle_class_ids must not be empty"));
        }
        OccupancyConfig::new(
            self.vehicle_class_ids.iter().copied(),
            self.detection_confidence_threshold,
            self.free_threshold,
            self.occupied_threshold,
        )
        .map_err(|error| ConfigError::new(error.to_string()))?;

        let mut ids = HashSet::new();
        for spot in &self.spots {
            if !ids.insert(spot.spot_id) {
                return Err(ConfigError::new(format!(
                    "duplicate spot_id in configuration: {}",
                    spot.spot_id
                )));
            }
            spot.roi().map_err(|error| {
                ConfigError::new(format!("invalid ROI for spot {}: {error}", spot.spot_id))
            })?;
        }
        if self.spots.is_empty() {
            return Err(ConfigError::new("at least one parking spot is required"));
        }
        Ok(())
    }

    pub fn validate_artifacts(&self) -> Result<(), ConfigError> {
        if !self.video_file.is_file() {
            return Err(ConfigError::new(format!(
                "video file does not exist: {}",
                self.video_file.display()
            )));
        }
        if !self.model_path.is_file() {
            return Err(ConfigError::new(format!(
                "ONNX model does not exist: {}; set PARKING_MODEL_PATH or model_path",
                self.model_path.display()
            )));
        }
        Ok(())
    }

    #[must_use]
    pub const fn camera_id(&self) -> CameraId {
        CameraId::from_uuid(self.camera_id)
    }

    #[must_use]
    pub const fn sample_interval(&self) -> StdDuration {
        StdDuration::from_millis(self.sample_interval_ms)
    }

    pub fn occupancy_config(&self) -> OccupancyConfig {
        OccupancyConfig::new(
            self.vehicle_class_ids.iter().copied(),
            self.detection_confidence_threshold,
            self.free_threshold,
            self.occupied_threshold,
        )
        .expect("validated edge config must contain valid thresholds")
    }

    pub fn stabilizer(&self) -> SpotStabilizer {
        SpotStabilizer::new(
            self.stable_samples_required,
            Duration::milliseconds(self.observation_refresh_interval_ms as i64),
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigError(String);

impl ConfigError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> EdgeConfig {
        EdgeConfig {
            camera_id: Uuid::new_v4(),
            video_file: "video.mp4".into(),
            sample_interval_ms: 500,
            observation_refresh_interval_ms: 5_000,
            stable_samples_required: 3,
            model_path: "model.onnx".into(),
            model_version: "parking-detector-v1".into(),
            input_width: 640,
            input_height: 640,
            vehicle_class_ids: vec![2, 3, 5, 7],
            detection_confidence_threshold: 0.5,
            free_threshold: 0.1,
            occupied_threshold: 0.3,
            state_database: "edge-state.db".into(),
            spots: vec![SpotConfig {
                spot_id: Uuid::new_v4(),
                polygon: vec![
                    PointConfig { x: 0.1, y: 0.1 },
                    PointConfig { x: 0.5, y: 0.1 },
                    PointConfig { x: 0.5, y: 0.5 },
                ],
            }],
        }
    }

    #[test]
    fn duplicate_spot_is_rejected() {
        let mut config = config();
        config.spots.push(config.spots[0].clone());
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("duplicate")
        );
    }

    #[test]
    fn refresh_must_precede_backend_ttl() {
        let mut config = config();
        config.observation_refresh_interval_ms = 15_000;
        assert!(config.validate().is_err());
    }
}
