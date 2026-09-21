use chrono::{DateTime, Utc};

use crate::{CameraId, EventId, ObservedState, ParkingSpotId};

#[derive(Clone, Debug, PartialEq)]
pub struct SpotObservation {
    pub event_id: EventId,
    pub camera_id: CameraId,
    pub spot_id: ParkingSpotId,
    pub sequence: u64,
    pub observed_at: DateTime<Utc>,
    pub state: ObservedState,
    pub model_score: Option<f32>,
    pub model_version: Option<String>,
}

impl SpotObservation {
    #[must_use]
    pub fn new(
        event_id: EventId,
        camera_id: CameraId,
        spot_id: ParkingSpotId,
        sequence: u64,
        observed_at: DateTime<Utc>,
        state: ObservedState,
    ) -> Self {
        Self {
            event_id,
            camera_id,
            spot_id,
            sequence,
            observed_at,
            state,
            model_score: None,
            model_version: None,
        }
    }
}
