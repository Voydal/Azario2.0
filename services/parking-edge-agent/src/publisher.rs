use std::{error::Error, fmt, time::Duration};

use async_nats::jetstream::{self, context::traits::Publisher, message::PublishMessage};
use async_trait::async_trait;
use parking_domain::{CameraId, ObservedState};
use parking_events::{
    ObservedStateV1, PARKING_OBSERVATIONS_V1_SUBJECT, SPOT_OBSERVATION_V1_SCHEMA_VERSION,
    SpotObservationV1,
};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{pipeline::PendingObservation, sequence::SequenceStore};

#[async_trait]
pub trait ObservationPublisher: Send + Sync {
    async fn publish(&self, event: &SpotObservationV1) -> Result<(), PublishError>;
}

pub struct JetStreamPublisher {
    context: jetstream::Context,
    subject: String,
}

impl JetStreamPublisher {
    #[must_use]
    pub fn new(context: jetstream::Context) -> Self {
        Self {
            context,
            subject: PARKING_OBSERVATIONS_V1_SUBJECT.into(),
        }
    }

    #[must_use]
    pub fn with_subject(context: jetstream::Context, subject: impl Into<String>) -> Self {
        Self {
            context,
            subject: subject.into(),
        }
    }
}

#[async_trait]
impl ObservationPublisher for JetStreamPublisher {
    async fn publish(&self, event: &SpotObservationV1) -> Result<(), PublishError> {
        let payload = serde_json::to_vec(event).map_err(PublishError::from_error)?;
        let message = PublishMessage::build()
            .payload(payload.into())
            .message_id(event.event_id.to_string())
            .outbound_message(self.subject.clone());
        self.context
            .publish_message(message)
            .await
            .map_err(PublishError::from_error)?
            .await
            .map_err(PublishError::from_error)?;
        Ok(())
    }
}

pub async fn publish_with_retry(
    publisher: &dyn ObservationPublisher,
    sequences: &mut SequenceStore,
    camera_id: CameraId,
    pending: PendingObservation,
    model_version: &str,
    max_attempts: u32,
    initial_delay: Duration,
) -> Result<SpotObservationV1, PublishError> {
    if max_attempts == 0 {
        return Err(PublishError::new("max_attempts must be positive"));
    }
    let sequence = sequences
        .allocate(camera_id)
        .map_err(PublishError::from_error)?;
    let event = SpotObservationV1 {
        schema_version: SPOT_OBSERVATION_V1_SCHEMA_VERSION,
        event_id: Uuid::new_v4(),
        camera_id: camera_id.into_uuid(),
        spot_id: pending.spot_id.into_uuid(),
        sequence,
        observed_at: pending.observed_at,
        state: match pending.state {
            ObservedState::Free => ObservedStateV1::Free,
            ObservedState::Occupied => ObservedStateV1::Occupied,
            ObservedState::Uncertain => ObservedStateV1::Uncertain,
        },
        model_score: Some(pending.score),
        model_version: Some(model_version.into()),
    };

    let mut delay = initial_delay;
    for attempt in 1..=max_attempts {
        match publisher.publish(&event).await {
            Ok(()) => {
                info!(
                    event_id = %event.event_id,
                    sequence = event.sequence,
                    spot_id = %event.spot_id,
                    "event published"
                );
                return Ok(event);
            }
            Err(error) if attempt < max_attempts => {
                warn!(attempt, error = %error, "publish retry");
                tokio::time::sleep(delay).await;
                delay = delay.saturating_mul(2);
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("positive attempt count always returns from loop")
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishError(String);

impl PublishError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    fn from_error(error: impl Error) -> Self {
        Self(error.to_string())
    }
}

impl fmt::Display for PublishError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for PublishError {}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use chrono::Utc;
    use parking_domain::{ObservedState, ParkingSpotId};
    use parking_perception::EmissionReason;

    use super::*;

    struct FailingPublisher {
        sequences: Mutex<Vec<u64>>,
    }

    #[async_trait]
    impl ObservationPublisher for FailingPublisher {
        async fn publish(&self, event: &SpotObservationV1) -> Result<(), PublishError> {
            self.sequences.lock().unwrap().push(event.sequence);
            Err(PublishError::new("offline"))
        }
    }

    fn pending() -> PendingObservation {
        PendingObservation {
            spot_id: ParkingSpotId::new(),
            state: ObservedState::Free,
            score: 0.0,
            observed_at: Utc::now(),
            reason: EmissionReason::StableChange,
        }
    }

    #[tokio::test]
    async fn failed_publish_does_not_reuse_sequence() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = SequenceStore::open(&directory.path().join("state.db")).unwrap();
        let publisher = FailingPublisher {
            sequences: Mutex::new(Vec::new()),
        };
        let camera_id = CameraId::new();

        assert!(
            publish_with_retry(
                &publisher,
                &mut store,
                camera_id,
                pending(),
                "test",
                1,
                Duration::ZERO,
            )
            .await
            .is_err()
        );
        assert!(
            publish_with_retry(
                &publisher,
                &mut store,
                camera_id,
                pending(),
                "test",
                1,
                Duration::ZERO,
            )
            .await
            .is_err()
        );
        assert_eq!(*publisher.sequences.lock().unwrap(), vec![1, 2]);
    }
}
