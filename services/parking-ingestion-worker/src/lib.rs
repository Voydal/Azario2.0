use std::{error::Error, time::Duration};

use async_nats::jetstream::{
    self, AckKind,
    consumer::{self, AckPolicy, DeliverPolicy, PullConsumer},
    message::Message,
    stream::{RetentionPolicy, StorageType},
};
use parking_domain::SpotObservation;
use parking_events::{
    PARKING_INGESTION_V1_CONSUMER, PARKING_OBSERVATIONS_STREAM, PARKING_OBSERVATIONS_V1_SUBJECT,
    SpotObservationV1,
};
use parking_persistence::{ParkingRepository, RepositoryError, StoreObservationResult};
use tracing::{info, warn};

pub const CONSUMER_ACK_WAIT: Duration = Duration::from_secs(5);
pub const CONSUMER_MAX_DELIVER: i64 = 5;
pub const RETRY_DELAY: Duration = Duration::from_secs(1);
pub const STREAM_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

pub type BoxError = Box<dyn Error + Send + Sync>;

pub async fn provision_jetstream(context: &jetstream::Context) -> Result<PullConsumer, BoxError> {
    let stream = context
        .get_or_create_stream(jetstream::stream::Config {
            name: PARKING_OBSERVATIONS_STREAM.into(),
            subjects: vec![PARKING_OBSERVATIONS_V1_SUBJECT.into()],
            storage: StorageType::File,
            retention: RetentionPolicy::Limits,
            max_age: STREAM_MAX_AGE,
            ..Default::default()
        })
        .await?;

    let consumer = stream
        .get_or_create_consumer(
            PARKING_INGESTION_V1_CONSUMER,
            consumer::pull::Config {
                durable_name: Some(PARKING_INGESTION_V1_CONSUMER.into()),
                filter_subject: PARKING_OBSERVATIONS_V1_SUBJECT.into(),
                deliver_policy: DeliverPolicy::All,
                ack_policy: AckPolicy::Explicit,
                ack_wait: CONSUMER_ACK_WAIT,
                max_deliver: CONSUMER_MAX_DELIVER,
                ..Default::default()
            },
        )
        .await?;

    Ok(consumer)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProcessingResult {
    Processed,
    Duplicate,
    RetryableFailure(String),
    PermanentFailure(String),
}

pub async fn process_payload(repository: &ParkingRepository, payload: &[u8]) -> ProcessingResult {
    let event: SpotObservationV1 = match serde_json::from_slice(payload) {
        Ok(event) => event,
        Err(error) => {
            return ProcessingResult::PermanentFailure(format!("invalid JSON payload: {error}"));
        }
    };
    let observation = match SpotObservation::try_from(event) {
        Ok(observation) => observation,
        Err(error) => return ProcessingResult::PermanentFailure(error.to_string()),
    };

    match repository.store_observation(&observation).await {
        Ok(StoreObservationResult::Stored) => ProcessingResult::Processed,
        Ok(StoreObservationResult::Duplicate) => ProcessingResult::Duplicate,
        Err(RepositoryError::SequenceOutOfRange(_)) => {
            ProcessingResult::PermanentFailure("sequence does not fit PostgreSQL BIGINT".into())
        }
        Err(RepositoryError::Database(_)) => {
            ProcessingResult::RetryableFailure("persistence failure".into())
        }
    }
}

pub async fn handle_message(
    repository: &ParkingRepository,
    message: &Message,
) -> Result<ProcessingResult, BoxError> {
    let result = process_payload(repository, &message.message.payload).await;

    match &result {
        ProcessingResult::Processed => {
            message.ack().await?;
            info!(
                service = "parking-ingestion-worker",
                event = "message_acked",
                "event processed"
            );
        }
        ProcessingResult::Duplicate => {
            message.ack().await?;
            info!(
                service = "parking-ingestion-worker",
                event = "duplicate_acked",
                "duplicate event"
            );
        }
        ProcessingResult::RetryableFailure(reason) => {
            warn!(
                service = "parking-ingestion-worker",
                event = "message_nak",
                reason,
                "event NAK"
            );
            message.ack_with(AckKind::Nak(Some(RETRY_DELAY))).await?;
        }
        ProcessingResult::PermanentFailure(reason) => {
            warn!(
                service = "parking-ingestion-worker",
                event = "message_term",
                reason,
                "invalid event TERM"
            );
            message.ack_with(AckKind::Term).await?;
        }
    }

    Ok(result)
}
