use std::{env, sync::Arc, time::Duration};

use async_nats::jetstream::{
    self,
    consumer::{self, AckPolicy, DeliverPolicy, PullConsumer},
    stream::StorageType,
};
use chrono::Utc;
use futures_util::StreamExt;
use parking_domain::{CameraId, ObservedState, ParkingSpotId};
use parking_edge_agent::{
    frame_source::{FrameSource, FrameSourceError},
    pipeline::{PendingObservation, PerceptionPipeline},
    publisher::{JetStreamPublisher, publish_with_retry},
    sequence::SequenceStore,
};
use parking_events::{ObservedStateV1, SpotObservationV1};
use parking_ingestion_worker::{ProcessingResult, handle_message};
use parking_perception::{
    BoundingBox, Detection, DetectionError, Detector, Frame, NormalizedPoint, OccupancyConfig,
    OccupancyEngine, ParkingSpotRoi, SpotStabilizer,
};
use parking_persistence::ParkingRepository;
use sqlx::Row;
use uuid::Uuid;

const RECEIVE_TIMEOUT: Duration = Duration::from_secs(3);

struct Fixture {
    context: jetstream::Context,
    consumer: PullConsumer,
    stream_name: String,
    subject: String,
}

impl Fixture {
    async fn new() -> Self {
        let nats_url = env::var("NATS_URL").expect("NATS_URL must point to the test NATS server");
        let client = async_nats::connect(nats_url).await.unwrap();
        let context = jetstream::new(client);
        let suffix = Uuid::new_v4().simple().to_string();
        let stream_name = format!("EDGE_TEST_{suffix}").to_uppercase();
        let subject = format!("test.edge.observations.{suffix}");
        let stream = context
            .create_stream(jetstream::stream::Config {
                name: stream_name.clone(),
                subjects: vec![subject.clone()],
                storage: StorageType::Memory,
                ..Default::default()
            })
            .await
            .unwrap();
        let consumer = stream
            .create_consumer(consumer::pull::Config {
                durable_name: Some(format!("edge-{suffix}")),
                filter_subject: subject.clone(),
                deliver_policy: DeliverPolicy::All,
                ack_policy: AckPolicy::Explicit,
                ..Default::default()
            })
            .await
            .unwrap();
        Self {
            context,
            consumer,
            stream_name,
            subject,
        }
    }

    async fn receive(&self) -> async_nats::jetstream::message::Message {
        let mut messages = self
            .consumer
            .fetch()
            .max_messages(1)
            .expires(RECEIVE_TIMEOUT)
            .messages()
            .await
            .unwrap();
        tokio::time::timeout(RECEIVE_TIMEOUT + Duration::from_secs(1), messages.next())
            .await
            .expect("timed out waiting for edge event")
            .expect("consumer ended before edge event")
            .expect("edge event delivery failed")
    }

    async fn cleanup(self) {
        self.context.delete_stream(&self.stream_name).await.unwrap();
    }
}

struct FakeFrameSource(Option<Frame>);

impl FrameSource for FakeFrameSource {
    fn next_frame(&mut self) -> Result<Option<Frame>, FrameSourceError> {
        Ok(self.0.take())
    }

    fn close(&mut self) -> Result<(), FrameSourceError> {
        Ok(())
    }
}

struct FakeDetector;

impl Detector for FakeDetector {
    fn detect(&mut self, _frame: &Frame) -> Result<Vec<Detection>, DetectionError> {
        Ok(vec![Detection {
            class_id: 2,
            confidence: 0.9,
            bbox: BoundingBox::new(0.1, 0.1, 0.9, 0.9).unwrap(),
        }])
    }
}

fn fake_perception(spot_id: ParkingSpotId) -> PendingObservation {
    let frame = Frame {
        width: 2,
        height: 2,
        pixels: Arc::from([0_u8; 12]),
        captured_at: Utc::now(),
    };
    let mut source = FakeFrameSource(Some(frame));
    let frame = source.next_frame().unwrap().unwrap();
    let detections = FakeDetector.detect(&frame).unwrap();
    let roi = ParkingSpotRoi::new(
        spot_id,
        vec![
            NormalizedPoint::new(0.0, 0.0).unwrap(),
            NormalizedPoint::new(1.0, 0.0).unwrap(),
            NormalizedPoint::new(1.0, 1.0).unwrap(),
            NormalizedPoint::new(0.0, 1.0).unwrap(),
        ],
    )
    .unwrap();
    let mut pipeline = PerceptionPipeline::new(
        OccupancyEngine::new(OccupancyConfig::new([2], 0.5, 0.1, 0.3).unwrap()),
        vec![roi],
        || SpotStabilizer::new(1, chrono::Duration::seconds(5)),
        Duration::from_millis(500),
    );
    let pending = pipeline.process_detections(&detections, frame.captured_at);
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].state, ObservedState::Occupied);
    pending[0]
}

#[tokio::test]
async fn edge_publishes_valid_spot_observation_v1() {
    let fixture = Fixture::new().await;
    let publisher = JetStreamPublisher::with_subject(fixture.context.clone(), &fixture.subject);
    let directory = tempfile::tempdir().unwrap();
    let mut sequences = SequenceStore::open(&directory.path().join("state.db")).unwrap();
    let camera_id = CameraId::new();
    let spot_id = ParkingSpotId::new();
    let published = publish_with_retry(
        &publisher,
        &mut sequences,
        camera_id,
        fake_perception(spot_id),
        "fake-parking-detector-v1",
        1,
        Duration::ZERO,
    )
    .await
    .unwrap();

    let message = fixture.receive().await;
    let received: SpotObservationV1 = serde_json::from_slice(&message.message.payload).unwrap();
    message.ack().await.unwrap();
    assert_eq!(received.event_id, published.event_id);
    assert_eq!(received.camera_id, camera_id.into_uuid());
    assert_eq!(received.spot_id, spot_id.into_uuid());
    assert_eq!(received.sequence, 1);
    assert_eq!(received.state, ObservedStateV1::Occupied);
    assert_eq!(
        received.model_version.as_deref(),
        Some("fake-parking-detector-v1")
    );
    assert!(received.model_score.is_some());
    fixture.cleanup().await;
}

#[tokio::test]
async fn perception_event_reaches_postgres() {
    let database_url = env::var("DATABASE_URL")
        .expect("DATABASE_URL must point to the integration-test PostgreSQL database");
    let repository = ParkingRepository::connect(&database_url).await.unwrap();
    repository.migrate().await.unwrap();
    let spot_id = ParkingSpotId::new();
    repository
        .create_parking_spot(spot_id, 52.2297, 21.0122)
        .await
        .unwrap();

    let fixture = Fixture::new().await;
    let publisher = JetStreamPublisher::with_subject(fixture.context.clone(), &fixture.subject);
    let directory = tempfile::tempdir().unwrap();
    let mut sequences = SequenceStore::open(&directory.path().join("state.db")).unwrap();
    publish_with_retry(
        &publisher,
        &mut sequences,
        CameraId::new(),
        fake_perception(spot_id),
        "fake-parking-detector-v1",
        1,
        Duration::ZERO,
    )
    .await
    .unwrap();

    let message = fixture.receive().await;
    assert_eq!(
        handle_message(&repository, &message).await.unwrap(),
        ProcessingResult::Processed
    );
    let observation_count: i64 =
        sqlx::query("SELECT COUNT(*) AS count FROM spot_observations WHERE spot_id = $1")
            .bind(spot_id.into_uuid())
            .fetch_one(repository.pool())
            .await
            .unwrap()
            .try_get("count")
            .unwrap();
    let current_state: String =
        sqlx::query("SELECT state FROM spot_current_state WHERE spot_id = $1")
            .bind(spot_id.into_uuid())
            .fetch_one(repository.pool())
            .await
            .unwrap()
            .try_get("state")
            .unwrap();
    assert_eq!(observation_count, 1);
    assert_eq!(current_state, "occupied");
    fixture.cleanup().await;
}
