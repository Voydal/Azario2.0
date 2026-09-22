use std::{env, net::SocketAddr, sync::Arc, time::Duration as StdDuration};

use axum::{
    Json, Router,
    extract::{Query, State, rejection::JsonRejection},
    http::{HeaderValue, Method, StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use google_maps_adapter::GoogleMapsClient;
use parking_domain::{CameraId, EventId, ObservedState, ParkingSpotId, SpotObservation};
use parking_persistence::{
    CreateParkingSpotResult, ParkingRepository, RepositoryError, StoreObservationResult,
};
use parking_search::{
    Coordinate, DEFAULT_OBSERVATION_TTL, ExternalServiceError, FindParking, FindParkingConfig,
    FindParkingError, FindParkingOutcome, ParkingSearchResult,
};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
use uuid::Uuid;

const MAX_SEARCH_RADIUS_METERS: f64 = 5_000.0;
const DEFAULT_GOOGLE_MAPS_HTTP_TIMEOUT_MS: u64 = 3_000;
const DEFAULT_PARKING_SEARCH_RADIUS_M: u32 = 800;
const DEFAULT_PARKING_CANDIDATE_LIMIT: u32 = 25;
const MAX_PARKING_CANDIDATE_LIMIT: u32 = 625;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database_url = env::var("DATABASE_URL")
        .map_err(|_| "DATABASE_URL must be set before starting parking-api")?;
    let repository = ParkingRepository::connect(&database_url).await?;
    repository.migrate().await?;
    let parking_search = build_parking_search(&repository)?;
    let frontend_origin = env::var("FRONTEND_ORIGIN")
        .unwrap_or_else(|_| "http://localhost:5173".into())
        .parse::<HeaderValue>()?;

    let address = SocketAddr::from(([0, 0, 0, 0], 3000));
    let listener = TcpListener::bind(address).await?;

    println!("parking-api listening on http://{address}");
    axum::serve(
        listener,
        app(
            AppState {
                repository,
                parking_search,
            },
            frontend_origin,
        ),
    )
    .await?;
    Ok(())
}

#[derive(Clone)]
struct AppState {
    repository: ParkingRepository,
    parking_search: Option<Arc<FindParking>>,
}

fn app(state: AppState, frontend_origin: HeaderValue) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/parking-spots", post(create_parking_spot))
        .route("/v1/observations", post(create_observation))
        .route("/v1/parking-spots/free", get(find_free_parking_spots))
        .route("/v1/parking/search", post(find_parking))
        .with_state(state)
        .layer(
            CorsLayer::new()
                .allow_origin(frontend_origin)
                .allow_methods([Method::GET, Method::POST])
                .allow_headers([CONTENT_TYPE]),
        )
}

fn build_parking_search(
    repository: &ParkingRepository,
) -> Result<Option<Arc<FindParking>>, Box<dyn std::error::Error>> {
    let timeout_ms = environment_u64(
        "GOOGLE_MAPS_HTTP_TIMEOUT_MS",
        DEFAULT_GOOGLE_MAPS_HTTP_TIMEOUT_MS,
    )?;
    let search_radius_m =
        environment_u32("PARKING_SEARCH_RADIUS_M", DEFAULT_PARKING_SEARCH_RADIUS_M)?;
    let candidate_limit =
        environment_u32("PARKING_CANDIDATE_LIMIT", DEFAULT_PARKING_CANDIDATE_LIMIT)?;
    if timeout_ms == 0 || search_radius_m == 0 || candidate_limit == 0 {
        return Err("Google timeout, search radius and candidate limit must be positive".into());
    }
    if candidate_limit > MAX_PARKING_CANDIDATE_LIMIT {
        return Err("PARKING_CANDIDATE_LIMIT must not exceed 625".into());
    }

    let Ok(api_key) = env::var("GOOGLE_MAPS_API_KEY") else {
        println!("parking search disabled: GOOGLE_MAPS_API_KEY is not configured");
        return Ok(None);
    };
    if api_key.trim().is_empty() {
        println!("parking search disabled: GOOGLE_MAPS_API_KEY is empty");
        return Ok(None);
    }

    let google = Arc::new(GoogleMapsClient::new(
        api_key,
        StdDuration::from_millis(timeout_ms),
    )?);
    let search = FindParking::new(
        google.clone(),
        Arc::new(repository.clone()),
        google.clone(),
        google,
        FindParkingConfig {
            search_radius_m,
            candidate_limit,
        },
    )?;
    Ok(Some(Arc::new(search)))
}

fn environment_u64(name: &str, default: u64) -> Result<u64, Box<dyn std::error::Error>> {
    env::var(name).map_or(Ok(default), |value| {
        value
            .parse()
            .map_err(|_| format!("{name} must be an unsigned integer").into())
    })
}

fn environment_u32(name: &str, default: u32) -> Result<u32, Box<dyn std::error::Error>> {
    env::var(name).map_or(Ok(default), |value| {
        value
            .parse()
            .map_err(|_| format!("{name} must be an unsigned integer").into())
    })
}

async fn health() -> StatusCode {
    StatusCode::OK
}

async fn create_parking_spot(
    State(state): State<AppState>,
    payload: Result<Json<CreateParkingSpotRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(payload) = payload.map_err(|_| ApiError::bad_request("invalid JSON body"))?;
    validate_coordinates(payload.latitude, payload.longitude)?;

    let id = ParkingSpotId::from_uuid(payload.id);
    let result = state
        .repository
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
    State(state): State<AppState>,
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

    let result = state
        .repository
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
    State(state): State<AppState>,
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

    let spots = state
        .repository
        .find_free_parking_spots(
            query.lat,
            query.lon,
            query.radius_m,
            Utc::now(),
            DEFAULT_OBSERVATION_TTL,
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

async fn find_parking(
    State(state): State<AppState>,
    payload: Result<Json<FindParkingRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(payload) = payload.map_err(|_| ApiError::bad_request("invalid JSON body"))?;
    let origin = Coordinate::new(payload.origin.latitude, payload.origin.longitude)
        .map_err(|_| ApiError::bad_request("invalid origin coordinates"))?;
    if payload.destination_address.trim().is_empty() {
        return Err(ApiError::bad_request(
            "destination_address must not be empty",
        ));
    }
    let search = state.parking_search.ok_or_else(ApiError::not_configured)?;
    let outcome = search
        .execute(origin, &payload.destination_address)
        .await
        .map_err(map_search_error)?;

    match outcome {
        FindParkingOutcome::Found(result) => Ok((
            StatusCode::OK,
            Json(FindParkingResponse::from_result(
                payload.destination_address,
                *result,
            )),
        )
            .into_response()),
        FindParkingOutcome::NoCandidates => Ok((
            StatusCode::OK,
            Json(EmptyParkingSearchResponse {
                result: None,
                reason: "no_available_parking",
            }),
        )
            .into_response()),
        FindParkingOutcome::NoReachableParking => Ok((
            StatusCode::OK,
            Json(EmptyParkingSearchResponse {
                result: None,
                reason: "no_reachable_parking",
            }),
        )
            .into_response()),
    }
}

fn validate_coordinates(latitude: f64, longitude: f64) -> Result<(), ApiError> {
    Coordinate::new(latitude, longitude)
        .map(|_| ())
        .map_err(|_| ApiError::bad_request("invalid coordinates"))
}

fn map_search_error(error: FindParkingError) -> ApiError {
    match error {
        FindParkingError::EmptyDestinationAddress => {
            ApiError::bad_request("destination_address must not be empty")
        }
        FindParkingError::AddressNotFound => ApiError::unprocessable_entity("address_not_found"),
        FindParkingError::External(ExternalServiceError::Timeout) => ApiError::gateway_timeout(),
        FindParkingError::External(
            ExternalServiceError::QuotaExceeded
            | ExternalServiceError::Upstream
            | ExternalServiceError::MalformedResponse,
        ) => ApiError::bad_gateway(),
        FindParkingError::Repository(_) | FindParkingError::InvalidConfiguration => {
            ApiError::internal()
        }
    }
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

#[derive(Deserialize)]
struct FindParkingRequest {
    origin: CoordinateDto,
    destination_address: String,
}

#[derive(Deserialize)]
struct CoordinateDto {
    latitude: f64,
    longitude: f64,
}

#[derive(Serialize)]
struct FindParkingResponse {
    destination: DestinationResponse,
    parking_spot: SelectedParkingSpotResponse,
    ranking: RankingResponse,
    walk: WalkResponse,
    route: RouteResponse,
    warnings: Vec<WarningResponse>,
}

impl FindParkingResponse {
    fn from_result(input_address: String, result: ParkingSearchResult) -> Self {
        Self {
            destination: DestinationResponse {
                input_address,
                formatted_address: result.destination.formatted_address,
                latitude: result.destination.coordinates.latitude(),
                longitude: result.destination.coordinates.longitude(),
                place_id: result.destination.place_id,
            },
            parking_spot: SelectedParkingSpotResponse {
                id: result.spot.spot_id.into_uuid(),
                latitude: result.spot.coordinates.latitude(),
                longitude: result.spot.coordinates.longitude(),
                observed_at: result.spot.observed_at,
                distance_to_destination_m: result.spot.distance_to_destination_m,
            },
            ranking: RankingResponse {
                driving_duration_s: result.ranking.drive_duration_s,
                walking_duration_s: result.ranking.walking_duration_s,
                score_s: result.ranking.score_s,
            },
            walk: WalkResponse {
                distance_m: result.walk.distance_m,
                duration_s: result.walk.duration_s,
            },
            route: RouteResponse {
                distance_m: result.route.distance_m,
                duration_s: result.route.duration_s,
                encoded_polyline: result.route.encoded_polyline,
            },
            warnings: vec![WarningResponse {
                code: "walking_routes_beta",
                message: "Walking routes are beta and may not always include clear pedestrian paths.",
            }],
        }
    }
}

#[derive(Serialize)]
struct DestinationResponse {
    input_address: String,
    formatted_address: String,
    latitude: f64,
    longitude: f64,
    place_id: Option<String>,
}

#[derive(Serialize)]
struct SelectedParkingSpotResponse {
    id: Uuid,
    latitude: f64,
    longitude: f64,
    observed_at: DateTime<Utc>,
    distance_to_destination_m: f64,
}

#[derive(Serialize)]
struct RankingResponse {
    driving_duration_s: u64,
    walking_duration_s: u64,
    score_s: u64,
}

#[derive(Serialize)]
struct WalkResponse {
    distance_m: u64,
    duration_s: u64,
}

#[derive(Serialize)]
struct RouteResponse {
    distance_m: u64,
    duration_s: u64,
    encoded_polyline: String,
}

#[derive(Serialize)]
struct WarningResponse {
    code: &'static str,
    message: &'static str,
}

#[derive(Serialize)]
struct EmptyParkingSearchResponse {
    result: Option<()>,
    reason: &'static str,
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

    const fn unprocessable_entity(message: &'static str) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            message,
        }
    }

    const fn not_configured() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: "parking_search_not_configured",
        }
    }

    const fn bad_gateway() -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            message: "google_maps_upstream_failure",
        }
    }

    const fn gateway_timeout() -> Self {
        Self {
            status: StatusCode::GATEWAY_TIMEOUT,
            message: "google_maps_timeout",
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
