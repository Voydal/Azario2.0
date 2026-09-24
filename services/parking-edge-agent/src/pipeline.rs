use std::{collections::HashMap, time::Duration};

use chrono::{DateTime, Utc};
use parking_domain::{ObservedState, ParkingSpotId};
use parking_perception::{
    Detection, EmissionReason, OccupancyEngine, ParkingSpotRoi, SpotStabilizer,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PendingObservation {
    pub spot_id: ParkingSpotId,
    pub state: ObservedState,
    pub score: f32,
    pub observed_at: DateTime<Utc>,
    pub reason: EmissionReason,
}

pub struct PerceptionPipeline {
    occupancy: OccupancyEngine,
    spots: Vec<ParkingSpotRoi>,
    stabilizers: HashMap<ParkingSpotId, SpotStabilizer>,
    sample_interval: Duration,
    last_sampled_at: Option<DateTime<Utc>>,
}

impl PerceptionPipeline {
    #[must_use]
    pub fn new(
        occupancy: OccupancyEngine,
        spots: Vec<ParkingSpotRoi>,
        stabilizer: impl Fn() -> SpotStabilizer,
        sample_interval: Duration,
    ) -> Self {
        let stabilizers = spots
            .iter()
            .map(|spot| (spot.spot_id, stabilizer()))
            .collect();
        Self {
            occupancy,
            spots,
            stabilizers,
            sample_interval,
            last_sampled_at: None,
        }
    }

    #[must_use]
    pub fn should_sample(&mut self, captured_at: DateTime<Utc>) -> bool {
        if self.last_sampled_at.is_some_and(|last| {
            captured_at
                .signed_duration_since(last)
                .to_std()
                .is_ok_and(|elapsed| elapsed < self.sample_interval)
        }) {
            return false;
        }
        self.last_sampled_at = Some(captured_at);
        true
    }

    pub fn process_detections(
        &mut self,
        detections: &[Detection],
        observed_at: DateTime<Utc>,
    ) -> Vec<PendingObservation> {
        let mut observations = Vec::new();
        for spot in &self.spots {
            let scored = self.occupancy.classify(spot, detections);
            let stabilizer = self
                .stabilizers
                .get_mut(&spot.spot_id)
                .expect("every configured ROI must have a stabilizer");
            if let Some(emission) = stabilizer.observe(scored.state, observed_at) {
                observations.push(PendingObservation {
                    spot_id: spot.spot_id,
                    state: emission.state,
                    score: scored.score,
                    observed_at,
                    reason: emission.reason,
                });
            }
        }
        observations
    }

    #[must_use]
    pub const fn missing_frame(&self) -> Vec<PendingObservation> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration as ChronoDuration, TimeZone};
    use parking_perception::{NormalizedPoint, OccupancyConfig};

    use super::*;

    fn pipeline(required: u32) -> PerceptionPipeline {
        let spot = ParkingSpotRoi::new(
            ParkingSpotId::new(),
            vec![
                NormalizedPoint::new(0.0, 0.0).unwrap(),
                NormalizedPoint::new(1.0, 0.0).unwrap(),
                NormalizedPoint::new(1.0, 1.0).unwrap(),
            ],
        )
        .unwrap();
        PerceptionPipeline::new(
            OccupancyEngine::new(OccupancyConfig::new([2], 0.5, 0.1, 0.3).unwrap()),
            vec![spot],
            || SpotStabilizer::new(required, ChronoDuration::seconds(5)),
            Duration::from_millis(500),
        )
    }

    #[test]
    fn missing_frame_does_not_emit_free() {
        assert!(pipeline(1).missing_frame().is_empty());
    }

    #[test]
    fn sampled_frames_are_rate_limited() {
        let mut pipeline = pipeline(1);
        let start = Utc.timestamp_opt(0, 0).single().unwrap();
        assert!(pipeline.should_sample(start));
        assert!(!pipeline.should_sample(start + ChronoDuration::milliseconds(499)));
        assert!(pipeline.should_sample(start + ChronoDuration::milliseconds(500)));
    }
}
