use std::env;

use chrono::{DateTime, Duration, Utc};
use parking_domain::{CameraId, EventId, ObservedState, ParkingSpotId, SpotObservation};
use parking_persistence::{ParkingRepository, StoreObservationResult};
use sqlx::Row;

const LATITUDE: f64 = 52.2297;
const LONGITUDE: f64 = 21.0122;
const TTL: Duration = Duration::seconds(15);

async fn repository() -> ParkingRepository {
    let database_url = env::var("DATABASE_URL")
        .expect("DATABASE_URL must point to the integration-test PostgreSQL/PostGIS database");
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
        .create_parking_spot(spot_id, LATITUDE, LONGITUDE)
        .await
        .expect("parking spot should be created");
    spot_id
}

fn observation(
    event_id: EventId,
    camera_id: CameraId,
    spot_id: ParkingSpotId,
    sequence: u64,
    observed_at: DateTime<Utc>,
    state: ObservedState,
) -> SpotObservation {
    SpotObservation::new(event_id, camera_id, spot_id, sequence, observed_at, state)
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
async fn observation_is_persisted() {
    let repository = repository().await;
    let spot_id = create_spot(&repository).await;
    let event_id = EventId::new();
    let observation = observation(
        event_id,
        CameraId::new(),
        spot_id,
        1,
        Utc::now(),
        ObservedState::Free,
    );

    repository
        .store_observation(&observation)
        .await
        .expect("observation should be stored");

    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM spot_observations WHERE event_id = $1")
            .bind(event_id.into_uuid())
            .fetch_one(repository.pool())
            .await
            .expect("history should be queryable");
    assert_eq!(count, 1);
}

#[tokio::test]
async fn first_observation_creates_current_state() {
    let repository = repository().await;
    let spot_id = create_spot(&repository).await;
    let observation = observation(
        EventId::new(),
        CameraId::new(),
        spot_id,
        7,
        Utc::now(),
        ObservedState::Free,
    );

    repository
        .store_observation(&observation)
        .await
        .expect("observation should be stored");

    assert_eq!(
        current_state(&repository, spot_id).await,
        ("free".into(), 7)
    );
}

#[tokio::test]
async fn newer_sequence_updates_current_state() {
    let repository = repository().await;
    let spot_id = create_spot(&repository).await;
    let camera_id = CameraId::new();
    let now = Utc::now();

    for event in [
        observation(
            EventId::new(),
            camera_id,
            spot_id,
            20,
            now,
            ObservedState::Free,
        ),
        observation(
            EventId::new(),
            camera_id,
            spot_id,
            21,
            now,
            ObservedState::Occupied,
        ),
    ] {
        repository
            .store_observation(&event)
            .await
            .expect("observation should be stored");
    }

    assert_eq!(
        current_state(&repository, spot_id).await,
        ("occupied".into(), 21)
    );
}

#[tokio::test]
async fn older_sequence_does_not_override_current_state() {
    let repository = repository().await;
    let spot_id = create_spot(&repository).await;
    let camera_id = CameraId::new();
    let now = Utc::now();

    for event in [
        observation(
            EventId::new(),
            camera_id,
            spot_id,
            21,
            now,
            ObservedState::Occupied,
        ),
        observation(
            EventId::new(),
            camera_id,
            spot_id,
            20,
            now,
            ObservedState::Free,
        ),
    ] {
        repository
            .store_observation(&event)
            .await
            .expect("observation should be stored");
    }

    assert_eq!(
        current_state(&repository, spot_id).await,
        ("occupied".into(), 21)
    );
}

#[tokio::test]
async fn duplicate_event_is_idempotent() {
    let repository = repository().await;
    let spot_id = create_spot(&repository).await;
    let event_id = EventId::new();
    let event = observation(
        event_id,
        CameraId::new(),
        spot_id,
        1,
        Utc::now(),
        ObservedState::Free,
    );

    assert!(matches!(
        repository.store_observation(&event).await,
        Ok(StoreObservationResult::Stored)
    ));
    assert!(matches!(
        repository.store_observation(&event).await,
        Ok(StoreObservationResult::Duplicate)
    ));

    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM spot_observations WHERE event_id = $1")
            .bind(event_id.into_uuid())
            .fetch_one(repository.pool())
            .await
            .expect("history should be queryable");
    assert_eq!(count, 1);
}

#[tokio::test]
async fn duplicate_sequence_is_idempotent() {
    let repository = repository().await;
    let spot_id = create_spot(&repository).await;
    let camera_id = CameraId::new();
    let now = Utc::now();
    let first = observation(
        EventId::new(),
        camera_id,
        spot_id,
        1,
        now,
        ObservedState::Free,
    );
    let duplicate = observation(
        EventId::new(),
        camera_id,
        spot_id,
        1,
        now,
        ObservedState::Occupied,
    );

    assert!(matches!(
        repository.store_observation(&first).await,
        Ok(StoreObservationResult::Stored)
    ));
    assert!(matches!(
        repository.store_observation(&duplicate).await,
        Ok(StoreObservationResult::Duplicate)
    ));
    assert_eq!(
        current_state(&repository, spot_id).await,
        ("free".into(), 1)
    );
}

#[tokio::test]
async fn free_fresh_spot_is_returned_by_geospatial_query() {
    let repository = repository().await;
    let spot_id = create_spot(&repository).await;
    let now = Utc::now();
    let event = observation(
        EventId::new(),
        CameraId::new(),
        spot_id,
        1,
        now,
        ObservedState::Free,
    );
    repository
        .store_observation(&event)
        .await
        .expect("observation should be stored");

    let spots = repository
        .find_free_parking_spots(LATITUDE, LONGITUDE, 500.0, now, TTL)
        .await
        .expect("search should succeed");

    assert!(spots.iter().any(|spot| spot.id == spot_id));
}

#[tokio::test]
async fn occupied_spot_is_not_returned() {
    let repository = repository().await;
    let spot_id = create_spot(&repository).await;
    let now = Utc::now();
    let event = observation(
        EventId::new(),
        CameraId::new(),
        spot_id,
        1,
        now,
        ObservedState::Occupied,
    );
    repository
        .store_observation(&event)
        .await
        .expect("observation should be stored");

    let spots = repository
        .find_free_parking_spots(LATITUDE, LONGITUDE, 500.0, now, TTL)
        .await
        .expect("search should succeed");

    assert!(!spots.iter().any(|spot| spot.id == spot_id));
}

#[tokio::test]
async fn stale_free_spot_is_not_returned() {
    let repository = repository().await;
    let spot_id = create_spot(&repository).await;
    let now = Utc::now();
    let event = observation(
        EventId::new(),
        CameraId::new(),
        spot_id,
        1,
        now - Duration::seconds(16),
        ObservedState::Free,
    );
    repository
        .store_observation(&event)
        .await
        .expect("observation should be stored");

    let spots = repository
        .find_free_parking_spots(LATITUDE, LONGITUDE, 500.0, now, TTL)
        .await
        .expect("search should succeed");

    assert!(!spots.iter().any(|spot| spot.id == spot_id));
}

#[tokio::test]
async fn spot_outside_radius_is_not_returned() {
    let repository = repository().await;
    let spot_id = ParkingSpotId::new();
    repository
        .create_parking_spot(spot_id, LATITUDE, LONGITUDE + 0.1)
        .await
        .expect("parking spot should be created");
    let now = Utc::now();
    let event = observation(
        EventId::new(),
        CameraId::new(),
        spot_id,
        1,
        now,
        ObservedState::Free,
    );
    repository
        .store_observation(&event)
        .await
        .expect("observation should be stored");

    let spots = repository
        .find_free_parking_spots(LATITUDE, LONGITUDE, 500.0, now, TTL)
        .await
        .expect("search should succeed");

    assert!(!spots.iter().any(|spot| spot.id == spot_id));
}

#[tokio::test]
async fn disabled_spot_is_not_returned() {
    let repository = repository().await;
    let spot_id = create_spot(&repository).await;
    let now = Utc::now();
    let event = observation(
        EventId::new(),
        CameraId::new(),
        spot_id,
        1,
        now,
        ObservedState::Free,
    );
    repository
        .store_observation(&event)
        .await
        .expect("observation should be stored");
    sqlx::query("UPDATE parking_spots SET enabled = FALSE WHERE id = $1")
        .bind(spot_id.into_uuid())
        .execute(repository.pool())
        .await
        .expect("spot should be disabled");

    let spots = repository
        .find_free_parking_spots(LATITUDE, LONGITUDE, 500.0, now, TTL)
        .await
        .expect("search should succeed");

    assert!(!spots.iter().any(|spot| spot.id == spot_id));
}

#[tokio::test]
async fn results_are_sorted_by_distance() {
    let repository = repository().await;
    let now = Utc::now();
    let camera_id = CameraId::new();
    let mut expected = Vec::new();

    for longitude_offset in [0.001, 0.0005, 0.0001] {
        let spot_id = ParkingSpotId::new();
        repository
            .create_parking_spot(spot_id, LATITUDE, LONGITUDE + longitude_offset)
            .await
            .expect("parking spot should be created");
        let event = observation(
            EventId::new(),
            camera_id,
            spot_id,
            1,
            now,
            ObservedState::Free,
        );
        repository
            .store_observation(&event)
            .await
            .expect("observation should be stored");
        expected.push(spot_id);
    }
    expected.reverse();

    let spots = repository
        .find_free_parking_spots(LATITUDE, LONGITUDE, 100.0, now, TTL)
        .await
        .expect("search should succeed");
    let actual: Vec<_> = spots
        .into_iter()
        .filter(|spot| expected.contains(&spot.id))
        .map(|spot| spot.id)
        .collect();

    assert_eq!(actual, expected);
}

#[tokio::test]
async fn concurrent_observations_leave_highest_sequence_as_current_state() {
    let repository = repository().await;
    let spot_id = create_spot(&repository).await;
    let camera_id = CameraId::new();
    let now = Utc::now();
    let sequence_101 = observation(
        EventId::new(),
        camera_id,
        spot_id,
        101,
        now,
        ObservedState::Free,
    );
    let sequence_102 = observation(
        EventId::new(),
        camera_id,
        spot_id,
        102,
        now,
        ObservedState::Occupied,
    );
    let first_repository = repository.clone();
    let second_repository = repository.clone();

    let (first, second) = tokio::join!(
        first_repository.store_observation(&sequence_101),
        second_repository.store_observation(&sequence_102),
    );
    first.expect("sequence 101 should be stored");
    second.expect("sequence 102 should be stored");

    assert_eq!(
        current_state(&repository, spot_id).await,
        ("occupied".into(), 102)
    );
}
