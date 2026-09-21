use std::{env, time::Duration};

use async_nats::jetstream::{
    self,
    consumer::{self, AckPolicy, DeliverPolicy, PullConsumer},
    context::traits::Publisher,
    message::{Message, PublishMessage},
    stream::StorageType,
};
use chrono::Utc;
use futures_util::StreamExt;
use parking_domain::ParkingSpotId;
use parking_events::{ObservedStateV1, SPOT_OBSERVATION_V1_SCHEMA_VERSION, SpotObservationV1};
use parking_ingestion_worker::{ProcessingResult, handle_message, process_payload};
use parking_persistence::ParkingRepository;
use sqlx::Row;
use uuid::Uuid;

const TEST_ACK_WAIT: Duration = Duration::from_secs(1);
const RECEIVE_TIMEOUT: Duration = Duration::from_secs(3);

struct NatsFixture {
    context: jetstream::Context,
    consumer: PullConsumer,
    stream_name: String,
    subject: String,
}

impl NatsFixture {
    async fn new() -> Self {
        let nats_url =
            env::var("NATS_URL").expect("NATS_URL must point to the integration-test NATS server");
        let client = async_nats::connect(&nats_url)
            .await
            .expect("test NATS server should be reachable");
        let context = jetstream::new(client);
        let suffix = Uuid::new_v4().simple().to_string();
        let stream_name = format!("TEST_{suffix}").to_uppercase();
        let consumer_name = format!("consumer-{suffix}");
        let subject = format!("test.parking.observations.{suffix}");
        let stream = context
            .create_stream(jetstream::stream::Config {
                name: stream_name.clone(),
                subjects: vec![subject.clone()],
                storage: StorageType::Memory,
                ..Default::default()
            })
            .await
            .expect("test stream should be created");
        let consumer = stream
            .create_consumer(consumer::pull::Config {
                durable_name: Some(consumer_name),
                filter_subject: subject.clone(),
                deliver_policy: DeliverPolicy::All,
                ack_policy: AckPolicy::Explicit,
                ack_wait: TEST_ACK_WAIT,
                max_deliver: 5,
                ..Default::default()
            })
            .await
            .expect("test consumer should be created");

        Self {
            context,
            consumer,
            stream_name,
            subject,
        }
    }

    async fn publish(&self, event: &SpotObservationV1, message_id: Uuid) {
        let payload = serde_json::to_vec(event).expect("event should serialize");
        self.publish_raw(payload, Some(message_id)).await;
    }

    async fn publish_raw(&self, payload: Vec<u8>, message_id: Option<Uuid>) {
        let mut publish = PublishMessage::build().payload(payload.into());
        if let Some(message_id) = message_id {
            publish = publish.message_id(message_id.to_string());
        }
        let message = publish.outbound_message(self.subject.clone());
        self.context
            .publish_message(message)
            .await
            .expect("publish should reach NATS")
            .await
            .expect("JetStream should acknowledge the publish");
    }

    async fn receive(&self) -> Message {
        let mut messages = self
            .consumer
            .fetch()
            .max_messages(1)
            .expires(RECEIVE_TIMEOUT)
            .messages()
            .await
            .expect("fetch should start");
        tokio::time::timeout(RECEIVE_TIMEOUT + Duration::from_secs(1), messages.next())
            .await
            .expect("timed out waiting for a JetStream message")
            .expect("consumer returned no message")
            .expect("message delivery should succeed")
    }

    async fn assert_no_message(&self) {
        let mut messages = self
            .consumer
            .fetch()
            .max_messages(1)
            .expires(Duration::from_secs(1))
            .messages()
            .await
            .expect("fetch should start");
        let result = tokio::time::timeout(Duration::from_secs(2), messages.next())
            .await
            .expect("empty fetch should finish");
        assert!(result.is_none(), "message was unexpectedly redelivered");
    }

    async fn cleanup(self) {
        self.context
            .delete_stream(&self.stream_name)
            .await
            .expect("test stream should be deleted");
    }
}

async fn repository() -> ParkingRepository {
    let database_url = env::var("DATABASE_URL")
        .expect("DATABASE_URL must point to the integration-test PostgreSQL database");
    let repository = ParkingRepository::connect(&database_url)
        .await
        .expect("test database should be reachable");
    repository
        .migrate()
        .await
        .expect("database migrations should succeed");
    repository
}

async fn create_spot(repository: &ParkingRepository) -> ParkingSpotId {
    let spot_id = ParkingSpotId::new();
    repository
        .create_parking_spot(spot_id, 52.2297, 21.0122)
        .await
        .expect("parking spot should be created");
    spot_id
}

fn event(spot_id: ParkingSpotId, sequence: u64, state: ObservedStateV1) -> SpotObservationV1 {
    SpotObservationV1 {
        schema_version: SPOT_OBSERVATION_V1_SCHEMA_VERSION,
        event_id: Uuid::new_v4(),
        camera_id: Uuid::new_v4(),
        spot_id: spot_id.into_uuid(),
        sequence,
        observed_at: Utc::now(),
        state,
        model_score: Some(0.97),
        model_version: Some("integration-test".into()),
    }
}

async fn current_state(repository: &ParkingRepository, spot_id: ParkingSpotId) -> (String, i64) {
    let row = sqlx::query("SELECT state, last_sequence FROM spot_current_state WHERE spot_id = $1")
        .bind(spot_id.into_uuid())
        .fetch_one(repository.pool())
        .await
        .expect("current state should exist");
    (
        row.try_get("state").expect("state should decode"),
        row.try_get("last_sequence")
            .expect("last_sequence should decode"),
    )
}

#[tokio::test]
async fn published_observation_is_received_by_durable_consumer() {
    let fixture = NatsFixture::new().await;
    let event = event(ParkingSpotId::new(), 1, ObservedStateV1::Free);
    fixture.publish(&event, event.event_id).await;

    let message = fixture.receive().await;
    let received: SpotObservationV1 =
        serde_json::from_slice(&message.message.payload).expect("payload should deserialize");
    message.ack().await.expect("message should ACK");

    assert_eq!(received, event);
    fixture.cleanup().await;
}

#[tokio::test]
async fn worker_processes_observation_into_postgres() {
    let fixture = NatsFixture::new().await;
    let repository = repository().await;
    let spot_id = create_spot(&repository).await;
    let event = event(spot_id, 42, ObservedStateV1::Free);
    fixture.publish(&event, event.event_id).await;

    let message = fixture.receive().await;
    let result = handle_message(&repository, &message)
        .await
        .expect("worker should handle message");

    assert_eq!(result, ProcessingResult::Processed);
    assert_eq!(
        current_state(&repository, spot_id).await,
        ("free".into(), 42)
    );
    fixture.cleanup().await;
}

#[tokio::test]
async fn unacked_message_is_redelivered() {
    let fixture = NatsFixture::new().await;
    let event = event(ParkingSpotId::new(), 1, ObservedStateV1::Free);
    fixture.publish(&event, event.event_id).await;

    let first = fixture.receive().await;
    let first_payload = first.message.payload.clone();
    drop(first);
    tokio::time::sleep(TEST_ACK_WAIT + Duration::from_millis(300)).await;

    let redelivered = fixture.receive().await;
    assert_eq!(redelivered.message.payload, first_payload);
    redelivered
        .ack()
        .await
        .expect("redelivered message should ACK");
    fixture.cleanup().await;
}

#[tokio::test]
async fn redelivery_after_database_commit_is_idempotent() {
    let fixture = NatsFixture::new().await;
    let repository = repository().await;
    let spot_id = create_spot(&repository).await;
    let event = event(spot_id, 55, ObservedStateV1::Free);
    fixture.publish(&event, event.event_id).await;

    let first = fixture.receive().await;
    assert_eq!(
        process_payload(&repository, &first.message.payload).await,
        ProcessingResult::Processed
    );
    drop(first);
    tokio::time::sleep(TEST_ACK_WAIT + Duration::from_millis(300)).await;

    let redelivered = fixture.receive().await;
    assert_eq!(
        handle_message(&repository, &redelivered)
            .await
            .expect("redelivery should be handled"),
        ProcessingResult::Duplicate
    );
    let history_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM spot_observations WHERE event_id = $1")
            .bind(event.event_id)
            .fetch_one(repository.pool())
            .await
            .expect("history should be queryable");
    assert_eq!(history_count, 1);
    assert_eq!(
        current_state(&repository, spot_id).await,
        ("free".into(), 55)
    );
    fixture.cleanup().await;
}

#[tokio::test]
async fn out_of_order_messages_leave_highest_sequence_in_projection() {
    let fixture = NatsFixture::new().await;
    let repository = repository().await;
    let spot_id = create_spot(&repository).await;
    let sequence_102 = event(spot_id, 102, ObservedStateV1::Occupied);
    let mut sequence_101 = event(spot_id, 101, ObservedStateV1::Free);
    sequence_101.camera_id = sequence_102.camera_id;
    fixture.publish(&sequence_102, sequence_102.event_id).await;
    fixture.publish(&sequence_101, sequence_101.event_id).await;

    for _ in 0..2 {
        let message = fixture.receive().await;
        handle_message(&repository, &message)
            .await
            .expect("message should be handled");
    }

    assert_eq!(
        current_state(&repository, spot_id).await,
        ("occupied".into(), 102)
    );
    fixture.cleanup().await;
}

#[tokio::test]
async fn duplicate_event_id_is_idempotent_through_worker() {
    let fixture = NatsFixture::new().await;
    let repository = repository().await;
    let spot_id = create_spot(&repository).await;
    let event = event(spot_id, 7, ObservedStateV1::Free);
    fixture.publish(&event, Uuid::new_v4()).await;
    fixture.publish(&event, Uuid::new_v4()).await;

    let first = fixture.receive().await;
    assert_eq!(
        handle_message(&repository, &first)
            .await
            .expect("first message should be handled"),
        ProcessingResult::Processed
    );
    let second = fixture.receive().await;
    assert_eq!(
        handle_message(&repository, &second)
            .await
            .expect("duplicate should be handled"),
        ProcessingResult::Duplicate
    );
    fixture.cleanup().await;
}

#[tokio::test]
async fn invalid_json_is_terminated_not_retried_forever() {
    let fixture = NatsFixture::new().await;
    let repository = repository().await;
    fixture.publish_raw(b"not-json".to_vec(), None).await;

    let message = fixture.receive().await;
    let result = handle_message(&repository, &message)
        .await
        .expect("invalid message should be terminated");
    assert!(matches!(result, ProcessingResult::PermanentFailure(_)));
    tokio::time::sleep(TEST_ACK_WAIT + Duration::from_millis(300)).await;
    fixture.assert_no_message().await;
    fixture.cleanup().await;
}

#[tokio::test]
async fn unsupported_schema_version_is_permanent_failure() {
    let fixture = NatsFixture::new().await;
    let repository = repository().await;
    let mut event = event(ParkingSpotId::new(), 1, ObservedStateV1::Free);
    event.schema_version = 999;
    fixture.publish(&event, event.event_id).await;

    let message = fixture.receive().await;
    let result = handle_message(&repository, &message)
        .await
        .expect("unsupported event should be terminated");
    assert_eq!(
        result,
        ProcessingResult::PermanentFailure("unsupported schema_version: 999".into())
    );
    tokio::time::sleep(TEST_ACK_WAIT + Duration::from_millis(300)).await;
    fixture.assert_no_message().await;
    fixture.cleanup().await;
}
