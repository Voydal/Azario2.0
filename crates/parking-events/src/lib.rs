use std::{error::Error, fmt};

use chrono::{DateTime, Utc};
use parking_domain::{CameraId, EventId, ObservedState, ParkingSpotId, SpotObservation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const SPOT_OBSERVATION_V1_SCHEMA_VERSION: u16 = 1;
pub const PARKING_OBSERVATIONS_STREAM: &str = "PARKING_OBSERVATIONS";
pub const PARKING_OBSERVATIONS_V1_SUBJECT: &str = "parking.observations.v1";
pub const PARKING_OBSERVATIONS_INVALID_V1_SUBJECT: &str = "parking.observations.invalid.v1";
pub const PARKING_INGESTION_V1_CONSUMER: &str = "parking-ingestion-v1";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpotObservationV1 {
    pub schema_version: u16,
    pub event_id: Uuid,
    pub camera_id: Uuid,
    pub spot_id: Uuid,
    pub sequence: u64,
    pub observed_at: DateTime<Utc>,
    pub state: ObservedStateV1,
    pub model_score: Option<f32>,
    pub model_version: Option<String>,
}

impl TryFrom<SpotObservationV1> for SpotObservation {
    type Error = EventConversionError;

    fn try_from(event: SpotObservationV1) -> Result<Self, Self::Error> {
        if event.schema_version != SPOT_OBSERVATION_V1_SCHEMA_VERSION {
            return Err(EventConversionError::UnsupportedSchemaVersion(
                event.schema_version,
            ));
        }

        Ok(Self {
            event_id: EventId::from_uuid(event.event_id),
            camera_id: CameraId::from_uuid(event.camera_id),
            spot_id: ParkingSpotId::from_uuid(event.spot_id),
            sequence: event.sequence,
            observed_at: event.observed_at,
            state: event.state.into(),
            model_score: event.model_score,
            model_version: event.model_version,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ObservedStateV1 {
    Free,
    Occupied,
    Uncertain,
}

impl From<ObservedStateV1> for ObservedState {
    fn from(state: ObservedStateV1) -> Self {
        match state {
            ObservedStateV1::Free => Self::Free,
            ObservedStateV1::Occupied => Self::Occupied,
            ObservedStateV1::Uncertain => Self::Uncertain,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventConversionError {
    UnsupportedSchemaVersion(u16),
}

impl fmt::Display for EventConversionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchemaVersion(version) => {
                write!(formatter, "unsupported schema_version: {version}")
            }
        }
    }
}

impl Error for EventConversionError {}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn event(schema_version: u16) -> SpotObservationV1 {
        SpotObservationV1 {
            schema_version,
            event_id: Uuid::new_v4(),
            camera_id: Uuid::new_v4(),
            spot_id: Uuid::new_v4(),
            sequence: 42,
            observed_at: Utc
                .with_ymd_and_hms(2026, 9, 21, 10, 0, 0)
                .single()
                .expect("test timestamp should be valid"),
            state: ObservedStateV1::Free,
            model_score: Some(0.97),
            model_version: Some("simulator-v1".into()),
        }
    }

    #[test]
    fn spot_observation_v1_roundtrip_json() {
        let event = event(SPOT_OBSERVATION_V1_SCHEMA_VERSION);

        let json = serde_json::to_string(&event).expect("event should serialize");
        let decoded = serde_json::from_str(&json).expect("event should deserialize");

        assert_eq!(event, decoded);
    }

    #[test]
    fn unsupported_schema_version_is_rejected() {
        let result = SpotObservation::try_from(event(999));

        assert_eq!(
            result,
            Err(EventConversionError::UnsupportedSchemaVersion(999))
        );
    }
}
