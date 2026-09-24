use std::{collections::HashSet, error::Error, fmt};

use parking_domain::ObservedState;

use crate::{Detection, ParkingSpotRoi};

#[derive(Clone, Debug)]
pub struct OccupancyConfig {
    vehicle_class_ids: HashSet<u32>,
    detection_confidence_threshold: f32,
    free_threshold: f32,
    occupied_threshold: f32,
}

impl OccupancyConfig {
    pub fn new(
        vehicle_class_ids: impl IntoIterator<Item = u32>,
        detection_confidence_threshold: f32,
        free_threshold: f32,
        occupied_threshold: f32,
    ) -> Result<Self, OccupancyConfigError> {
        let values = [
            detection_confidence_threshold,
            free_threshold,
            occupied_threshold,
        ];
        if !values
            .into_iter()
            .all(|value| value.is_finite() && (0.0..=1.0).contains(&value))
        {
            return Err(OccupancyConfigError::ThresholdOutOfRange);
        }
        if free_threshold >= occupied_threshold {
            return Err(OccupancyConfigError::InvalidThresholdOrder);
        }
        Ok(Self {
            vehicle_class_ids: vehicle_class_ids.into_iter().collect(),
            detection_confidence_threshold,
            free_threshold,
            occupied_threshold,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScoredState {
    pub state: ObservedState,
    pub score: f32,
}

pub struct OccupancyEngine {
    config: OccupancyConfig,
}

impl OccupancyEngine {
    #[must_use]
    pub const fn new(config: OccupancyConfig) -> Self {
        Self { config }
    }

    #[must_use]
    pub fn classify(&self, spot: &ParkingSpotRoi, detections: &[Detection]) -> ScoredState {
        let score = detections
            .iter()
            .filter(|detection| {
                self.config.vehicle_class_ids.contains(&detection.class_id)
                    && detection.confidence.is_finite()
                    && detection.confidence >= self.config.detection_confidence_threshold
            })
            .map(|detection| detection.confidence * spot.overlap_ratio(detection.bbox) as f32)
            .fold(0.0_f32, f32::max);
        let state = if score <= self.config.free_threshold {
            ObservedState::Free
        } else if score >= self.config.occupied_threshold {
            ObservedState::Occupied
        } else {
            ObservedState::Uncertain
        };
        ScoredState { state, score }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OccupancyConfigError {
    ThresholdOutOfRange,
    InvalidThresholdOrder,
}

impl fmt::Display for OccupancyConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ThresholdOutOfRange => formatter.write_str("thresholds must be finite in 0..=1"),
            Self::InvalidThresholdOrder => {
                formatter.write_str("free_threshold must be below occupied_threshold")
            }
        }
    }
}

impl Error for OccupancyConfigError {}

#[cfg(test)]
mod tests {
    use parking_domain::ParkingSpotId;

    use super::*;
    use crate::{BoundingBox, NormalizedPoint};

    fn roi() -> ParkingSpotRoi {
        ParkingSpotRoi::new(
            ParkingSpotId::new(),
            vec![
                NormalizedPoint::new(0.1, 0.1).unwrap(),
                NormalizedPoint::new(0.5, 0.1).unwrap(),
                NormalizedPoint::new(0.5, 0.5).unwrap(),
                NormalizedPoint::new(0.1, 0.5).unwrap(),
            ],
        )
        .unwrap()
    }

    fn engine() -> OccupancyEngine {
        OccupancyEngine::new(OccupancyConfig::new([2], 0.5, 0.1, 0.3).unwrap())
    }

    fn detection(class_id: u32, confidence: f32, bbox: [f64; 4]) -> Detection {
        Detection {
            class_id,
            confidence,
            bbox: BoundingBox::new(bbox[0], bbox[1], bbox[2], bbox[3]).unwrap(),
        }
    }

    #[test]
    fn vehicle_outside_roi_is_free() {
        let scored = engine().classify(&roi(), &[detection(2, 0.9, [0.6, 0.6, 0.9, 0.9])]);
        assert_eq!(scored.state, ObservedState::Free);
        assert_eq!(scored.score, 0.0);
    }

    #[test]
    fn vehicle_overlapping_roi_is_occupied() {
        let scored = engine().classify(&roi(), &[detection(2, 0.9, [0.1, 0.1, 0.5, 0.5])]);
        assert_eq!(scored.state, ObservedState::Occupied);
        assert!((scored.score - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn score_between_thresholds_is_uncertain() {
        let scored = engine().classify(&roi(), &[detection(2, 0.8, [0.1, 0.1, 0.2, 0.5])]);
        assert_eq!(scored.state, ObservedState::Uncertain);
        assert!((scored.score - 0.2).abs() < 0.0001);
    }

    #[test]
    fn non_vehicle_class_is_ignored() {
        let scored = engine().classify(&roi(), &[detection(9, 1.0, [0.1, 0.1, 0.5, 0.5])]);
        assert_eq!(scored.state, ObservedState::Free);
    }

    #[test]
    fn low_confidence_detection_is_ignored() {
        let scored = engine().classify(&roi(), &[detection(2, 0.49, [0.1, 0.1, 0.5, 0.5])]);
        assert_eq!(scored.state, ObservedState::Free);
    }
}
