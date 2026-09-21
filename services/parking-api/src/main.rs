use std::{env, net::SocketAddr};

use axum::{
    Json, Router,
    extract::{Query, State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Duration, Utc};
use parking_domain::{CameraId, EventId, ObservedState, ParkingSpotId, SpotObservation};
use parking_persistence::{
    CreateParkingSpotResult, ParkingRepository, RepositoryError, StoreObservationResult,
};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use uuid::Uuid;

const OBSERVATION_TTL: Duration = Duration::seconds(15);
const MAX_SEARCH_RADIUS_METERS: f64 = 5_000.0;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database_url = env::var("DATABASE_URL")
        .map_err(|_| "DATABASE_URL must be set before starting parking-api")?;
    let repository = ParkingRepository::connect(&database_url).await?;
    repository.migrate().await?;

    let address = SocketAddr::from(([0, 0, 0, 0], 3000));
    let listener = TcpListener::bind(address).await?;

    println!("parking-api listening on http://{address}");
    axum::serve(listener, app(repository)).await?;
    Ok(())
}

fn app(repository: ParkingRepository) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/parking-spots", post(create_parking_spot))
        .route("/v1/observations", post(create_observation))
        .route("/v1/parking-spots/free", get(find_free_parking_spots))
        .with_state(repository)
}

async fn health() -> StatusCode {
    StatusCode::OK
}

async fn create_parking_spot(
    State(repository): State<ParkingRepository>,
    payload: Result<Json<CreateParkingSpotRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(payload) = payload.map_err(|_| ApiError::bad_request("invalid JSON body"))?;
    validate_coordinates(payload.latitude, payload.longitude)?;

    let id = ParkingSpotId::from_uuid(payload.id);
    let result = repository
        .create_parking_spot(id, payload.latitude, payload.longitude)
        .await
        .map_err(|_| ApiError::internal())?;
    let status = match result {
        CreateParkingSpotResult::Created => StatusCode::CREATED,
        CreateParkingSpotResult::AlreadyExists => StatusCode::OK,
    };

    Ok((status, Json(CreateParkingSpotResponse { id: payload.id })))
}

async fn create_observation(
    State(repository): State<ParkingRepository>,
    payload: Result<Json<CreateObservationRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(payload) = payload.map_err(|_| ApiError::bad_request("invalid JSON body"))?;
    let observation = SpotObservation {
        event_id: EventId::from_uuid(payload.event_id),
        camera_id: CameraId::from_uuid(payload.camera_id),
        spot_id: ParkingSpotId::from_uuid(payload.spot_id),
        sequence: payload.sequence,
        observed_at: payload.observed_at,
        state: payload.state.into(),
        model_score: payload.model_score,
        model_version: payload.model_version,
    };

    let result = repository
        .store_observation(&observation)
        .await
        .map_err(map_repository_error)?;
    let (status, outcome) = match result {
        StoreObservationResult::Stored => (StatusCode::ACCEPTED, "stored"),
        StoreObservationResult::Duplicate => (StatusCode::OK, "duplicate"),
    };

    Ok((status, Json(CreateObservationResponse { outcome })))
}

async fn find_free_parking_spots(
    State(repository): State<ParkingRepository>,
    Query(query): Query<FindFreeParkingSpotsQuery>,
) -> Result<Json<FindFreeParkingSpotsResponse>, ApiError> {
    validate_coordinates(query.lat, query.lon)?;
    if !query.radius_m.is_finite()
        || query.radius_m <= 0.0
        || query.radius_m > MAX_SEARCH_RADIUS_METERS
    {
        return Err(ApiError::bad_request(
            "radius_m must be greater than 0 and no greater than 5000",
        ));
    }

    let spots = repository
        .find_free_parking_spots(
            query.lat,
            query.lon,
            query.radius_m,
            Utc::now(),
            OBSERVATION_TTL,
        )
        .await
        .map_err(|_| ApiError::internal())?
        .into_iter()
        .map(|spot| FreeParkingSpotResponse {
            id: spot.id.into_uuid(),
            latitude: spot.latitude,
            longitude: spot.longitude,
            observed_at: spot.observed_at,
            distance_m: spot.distance_m,
        })
        .collect();

    Ok(Json(FindFreeParkingSpotsResponse { spots }))
}

fn validate_coordinates(latitude: f64, longitude: f64) -> Result<(), ApiError> {
    if !latitude.is_finite()
        || !longitude.is_finite()
        || !(-90.0..=90.0).contains(&latitude)
        || !(-180.0..=180.0).contains(&longitude)
    {
        return Err(ApiError::bad_request(
            "latitude must be in [-90, 90] and longitude in [-180, 180]",
        ));
    }
    Ok(())
}

fn map_repository_error(error: RepositoryError) -> ApiError {
    if matches!(error, RepositoryError::SequenceOutOfRange(_)) {
        ApiError::bad_request("sequence must fit PostgreSQL BIGINT")
    } else if error.is_foreign_key_violation() {
        ApiError::not_found("parking spot does not exist")
    } else {
        ApiError::internal()
    }
}

#[derive(Deserialize)]
struct CreateParkingSpotRequest {
    id: Uuid,
    latitude: f64,
    longitude: f64,
}

#[derive(Serialize)]
struct CreateParkingSpotResponse {
    id: Uuid,
}

#[derive(Deserialize)]
struct CreateObservationRequest {
    event_id: Uuid,
    camera_id: Uuid,
    spot_id: Uuid,
    sequence: u64,
    observed_at: DateTime<Utc>,
    state: ObservedStateDto,
    model_score: Option<f32>,
    model_version: Option<String>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ObservedStateDto {
    Free,
    Occupied,
    Uncertain,
}

impl From<ObservedStateDto> for ObservedState {
    fn from(state: ObservedStateDto) -> Self {
        match state {
            ObservedStateDto::Free => Self::Free,
            ObservedStateDto::Occupied => Self::Occupied,
            ObservedStateDto::Uncertain => Self::Uncertain,
        }
    }
}

#[derive(Serialize)]
struct CreateObservationResponse {
    outcome: &'static str,
}

#[derive(Deserialize)]
struct FindFreeParkingSpotsQuery {
    lat: f64,
    lon: f64,
    radius_m: f64,
}

#[derive(Serialize)]
struct FindFreeParkingSpotsResponse {
    spots: Vec<FreeParkingSpotResponse>,
}

#[derive(Serialize)]
struct FreeParkingSpotResponse {
    id: Uuid,
    latitude: f64,
    longitude: f64,
    observed_at: DateTime<Utc>,
    distance_m: f64,
}

struct ApiError {
    status: StatusCode,
    message: &'static str,
}

impl ApiError {
    const fn bad_request(message: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message,
        }
    }

    const fn not_found(message: &'static str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message,
        }
    }

    const fn internal() -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "internal server error",
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorResponse {
                error: self.message,
            }),
        )
            .into_response()
    }
}

#[derive(Serialize)]
struct ErrorResponse {
    error: &'static str,
}
