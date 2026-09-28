use std::{
    env,
    net::SocketAddr,
    sync::Arc,
    time::{Duration as StdDuration, Instant},
};

use axum::{
    Json, Router,
    extract::{Query, Request, State, rejection::JsonRejection},
    http::{HeaderName, HeaderValue, Method, StatusCode, header::CONTENT_TYPE},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use google_maps_adapter::GoogleMapsClient;
use parking_domain::{CameraId, EventId, ObservedState, ParkingSpotId, SpotObservation};
use parking_persistence::{
    CreateParkingSpotResult, ParkingRepository, PoolConfig, RepositoryError, StoreObservationResult,
};
use parking_search::{
    Coordinate, DEFAULT_OBSERVATION_TTL, ExternalServiceError, FindParking, FindParkingConfig,
    FindParkingError, FindParkingOutcome, ParkingSearchResult,
};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

const MAX_SEARCH_RADIUS_METERS: f64 = 5_000.0;
const DEFAULT_GOOGLE_MAPS_HTTP_TIMEOUT_MS: u64 = 3_000;
const DEFAULT_PARKING_SEARCH_RADIUS_M: u32 = 800;
const DEFAULT_PARKING_CANDIDATE_LIMIT: u32 = 25;
const MAX_PARKING_CANDIDATE_LIMIT: u32 = 625;
const DEFAULT_DATABASE_CHECK_TIMEOUT_MS: u64 = 1500;
const DEFAULT_DATABASE_OPERATION_TIMEOUT_MS: u64 = 5000;
const DEFAULT_SEARCH_TIMEOUT_MS: u64 = 15000;
const DEFAULT_DATABASE_ACQUIRE_TIMEOUT_MS: u64 = 2000;
const DEFAULT_DATABASE_MAX_CONNECTIONS: u32 = 10;
const SHUTDOWN_TIMEOUT: StdDuration = StdDuration::from_secs(10);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let log_format = init_logging()?;
    let mut args = env::args().skip(1);
    let migrate = match (args.next().as_deref(), args.next()) {
        (None, None) => false,
        (Some("migrate"), None) => true,
        _ => return Err("usage: parking-api [migrate]".into()),
    };
    let database_url = env::var("DATABASE_URL")
        .map_err(|_| "DATABASE_URL must be set before starting parking-api")?;
    let max_connections =
        environment_u32("DATABASE_MAX_CONNECTIONS", DEFAULT_DATABASE_MAX_CONNECTIONS)?;
    let acquire_timeout_ms = environment_u64(
        "DATABASE_ACQUIRE_TIMEOUT_MS",
        DEFAULT_DATABASE_ACQUIRE_TIMEOUT_MS,
    )?;
    let check_timeout_ms = environment_u64(
        "DATABASE_CHECK_TIMEOUT_MS",
        DEFAULT_DATABASE_CHECK_TIMEOUT_MS,
    )?;
    let operation_timeout_ms = environment_u64(
        "DATABASE_OPERATION_TIMEOUT_MS",
        DEFAULT_DATABASE_OPERATION_TIMEOUT_MS,
    )?;
    let search_timeout_ms =
        environment_u64("PARKING_SEARCH_TIMEOUT_MS", DEFAULT_SEARCH_TIMEOUT_MS)?;
    if max_connections == 0
        || acquire_timeout_ms == 0
        || check_timeout_ms == 0
        || operation_timeout_ms == 0
        || search_timeout_ms == 0
    {
        return Err("database pool size and timeouts must be positive".into());
    }
    let repository = ParkingRepository::connect_with_config(
        &database_url,
        PoolConfig {
            max_connections,
            acquire_timeout: StdDuration::from_millis(acquire_timeout_ms),
        },
    )
    .await
    .map_err(|_| "database connection failed")?;
    if migrate {
        repository.migrate().await?;
        info!(
            service = "parking-api",
            event = "migration_complete",
            "database migrations applied"
        );
        return Ok(());
    }
    let parking_search = build_parking_search(&repository)?;
    let frontend_origin =
        env::var("FRONTEND_ORIGIN").unwrap_or_else(|_| "http://localhost:5173".into());
    let parsed_origin = reqwest::Url::parse(&frontend_origin)
        .map_err(|_| "FRONTEND_ORIGIN must be an absolute HTTP(S) URL")?;
    if !matches!(parsed_origin.scheme(), "http" | "https") || parsed_origin.host().is_none() {
        return Err("FRONTEND_ORIGIN must be an absolute HTTP(S) URL".into());
    }
    let frontend_origin = frontend_origin.parse::<HeaderValue>()?;
    let address: SocketAddr = env::var("API_BIND_ADDRESS")
        .unwrap_or_else(|_| "0.0.0.0:3000".into())
        .parse()
        .map_err(|_| "API_BIND_ADDRESS must be a valid IP:port socket address")?;
    let listener = TcpListener::bind(address).await?;
    info!(service = "parking-api", version = env!("CARGO_PKG_VERSION"), log_format,
        bind_address = %address, event = "startup", "API listening");
    axum::serve(
        listener,
        app(
            AppState {
                repository,
                parking_search,
                database_check_timeout: StdDuration::from_millis(check_timeout_ms),
                database_operation_timeout: StdDuration::from_millis(operation_timeout_ms),
                search_timeout: StdDuration::from_millis(search_timeout_ms),
            },
            frontend_origin,
        ),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    info!(
        service = "parking-api",
        event = "shutdown_complete",
        "API stopped"
    );
    Ok(())
}

fn init_logging() -> Result<&'static str, Box<dyn std::error::Error>> {
    let format = env::var("LOG_FORMAT").unwrap_or_else(|_| "pretty".into());
    let filter = match env::var("RUST_LOG") {
        Ok(value) => EnvFilter::try_new(value)?,
        Err(env::VarError::NotPresent) => EnvFilter::new("info"),
        Err(error) => return Err(error.into()),
    };
    match format.as_str() {
        "json" => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .json()
            .init(),
        "pretty" => tracing_subscriber::fmt().with_env_filter(filter).init(),
        _ => return Err("LOG_FORMAT must be json or pretty".into()),
    }
    Ok(if format == "json" { "json" } else { "pretty" })
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
        match terminate {
            Ok(mut terminate) => tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = terminate.recv() => {},
            },
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    info!(
        service = "parking-api",
        event = "shutdown_requested",
        "stopping new requests"
    );
    tokio::spawn(async {
        tokio::time::sleep(SHUTDOWN_TIMEOUT).await;
        tracing::error!(
            service = "parking-api",
            event = "shutdown_timeout",
            "graceful shutdown timed out"
        );
        std::process::exit(1);
    });
}

#[derive(Clone)]
struct AppState {
    repository: ParkingRepository,
    parking_search: Option<Arc<FindParking>>,
    database_check_timeout: StdDuration,
    database_operation_timeout: StdDuration,
    search_timeout: StdDuration,
}

fn app(state: AppState, frontend_origin: HeaderValue) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/health/live", get(health_live))
        .route("/health/ready", get(health_ready))
        .route("/version", get(version))
        .route("/v1/parking-spots", post(create_parking_spot))
        .route("/v1/observations", post(create_observation))
        .route("/v1/parking-spots/free", get(find_free_parking_spots))
        .route("/v1/parking/search", post(find_parking))
        .with_state(state)
        .layer(
            CorsLayer::new()
                .allow_origin(frontend_origin)
                .allow_methods([Method::GET, Method::POST])
                .allow_headers([CONTENT_TYPE, HeaderName::from_static("x-request-id")])
                .expose_headers([HeaderName::from_static("x-request-id")]),
        )
        .layer(middleware::from_fn(request_id_middleware))
}

async fn request_id_middleware(request: Request, next: Next) -> Response {
    let request_id = request
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        .unwrap_or_else(Uuid::new_v4);
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let started = Instant::now();
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        "x-request-id",
        HeaderValue::from_str(&request_id.to_string())
            .unwrap_or_else(|_| HeaderValue::from_static("invalid")),
    );
    info!(service = "parking-api", event = "http_request", %request_id, %method, %path,
        status = response.status().as_u16(), duration_ms = started.elapsed().as_millis() as u64,
        "request completed");
    response
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

async fn health_live() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

#[derive(Serialize)]
struct ReadinessResponse {
    status: &'static str,
    checks: ReadinessChecks,
}

#[derive(Serialize)]
struct ReadinessChecks {
    database: &'static str,
}

async fn health_ready(State(state): State<AppState>) -> (StatusCode, Json<ReadinessResponse>) {
    let check = tokio::time::timeout(state.database_check_timeout, async {
        sqlx::query("SELECT 1")
            .execute(state.repository.pool())
            .await
    })
    .await;
    let ready = matches!(check, Ok(Ok(_)));
    if !ready {
        warn!(
            service = "parking-api",
            event = "readiness_failure",
            dependency = "database",
            "database unavailable"
        );
    }
    (
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(ReadinessResponse {
            status: if ready { "ok" } else { "not_ready" },
            checks: ReadinessChecks {
                database: if ready { "ok" } else { "unavailable" },
            },
        }),
    )
}

#[derive(Serialize)]
struct VersionResponse {
    service: &'static str,
    version: &'static str,
    git_sha: Option<&'static str>,
}

async fn version() -> Json<VersionResponse> {
    Json(VersionResponse {
        service: "parking-api",
        version: env!("CARGO_PKG_VERSION"),
        git_sha: option_env!("PARKING_GIT_SHA"),
    })
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
        info!(
            service = "parking-api",
            event = "search_disabled",
            "Google key not configured"
        );
        return Ok(None);
    };
    if api_key.trim().is_empty() {
        info!(
            service = "parking-api",
            event = "search_disabled",
            "Google key is empty"
        );
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
    let result = tokio::time::timeout(
        state.database_operation_timeout,
        state
            .repository
            .create_parking_spot(id, payload.latitude, payload.longitude),
    )
    .await
    .map_err(|_| ApiError::internal())?
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

    let result = tokio::time::timeout(
        state.database_operation_timeout,
        state.repository.store_observation(&observation),
    )
    .await
    .map_err(|_| ApiError::internal())?
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

    let spots = tokio::time::timeout(
        state.database_operation_timeout,
        state.repository.find_free_parking_spots(
            query.lat,
            query.lon,
            query.radius_m,
            Utc::now(),
            DEFAULT_OBSERVATION_TTL,
        ),
    )
    .await
    .map_err(|_| ApiError::internal())?
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
    let outcome = tokio::time::timeout(
        state.search_timeout,
        search.execute(origin, &payload.destination_address),
    )
    .await
    .map_err(|_| ApiError::search_timeout())?
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

    const fn search_timeout() -> Self {
        Self {
            status: StatusCode::GATEWAY_TIMEOUT,
            message: "search_timeout",
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

#[cfg(test)]
mod operability_tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;

    fn unavailable_app() -> Router {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(StdDuration::from_millis(100))
            .connect_lazy("postgres://parking:parking@127.0.0.1:1/parking")
            .expect("valid test database URL");
        app(
            AppState {
                repository: ParkingRepository::new(pool),
                parking_search: None,
                database_check_timeout: StdDuration::from_millis(100),
                database_operation_timeout: StdDuration::from_secs(5),
                search_timeout: StdDuration::from_secs(15),
            },
            HeaderValue::from_static("http://localhost:5173"),
        )
    }

    async fn get(router: Router, path: &str) -> Response {
        router
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("valid test request"),
            )
            .await
            .expect("router response")
    }

    #[tokio::test]
    async fn liveness_does_not_depend_on_database() {
        let response = get(unavailable_app(), "/health/live").await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn readiness_fails_when_database_is_unavailable() {
        let response = get(unavailable_app(), "/health/ready").await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = to_bytes(response.into_body(), 1024)
            .await
            .expect("response body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
        assert_eq!(json["checks"]["database"], "unavailable");
    }

    #[tokio::test]
    async fn readiness_succeeds_with_local_postgres() {
        let Ok(url) = env::var("DATABASE_URL") else {
            return;
        };
        let repository = ParkingRepository::connect(&url)
            .await
            .expect("local PostgreSQL");
        let response = get(
            app(
                AppState {
                    repository,
                    parking_search: None,
                    database_check_timeout: StdDuration::from_millis(1500),
                    database_operation_timeout: StdDuration::from_secs(5),
                    search_timeout: StdDuration::from_secs(15),
                },
                HeaderValue::from_static("http://localhost:5173"),
            ),
            "/health/ready",
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn version_endpoint_returns_service_and_version() {
        let response = get(unavailable_app(), "/version").await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 1024)
            .await
            .expect("response body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
        assert_eq!(json["service"], "parking-api");
        assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn response_contains_request_id() {
        let response = get(unavailable_app(), "/health/live").await;
        let id = response
            .headers()
            .get("x-request-id")
            .expect("request ID")
            .to_str()
            .expect("ASCII request ID");
        assert!(Uuid::parse_str(id).is_ok());
    }

    #[tokio::test]
    async fn valid_incoming_request_id_is_propagated() {
        let id = Uuid::new_v4();
        let response = unavailable_app()
            .oneshot(
                Request::builder()
                    .uri("/health/live")
                    .header("x-request-id", id.to_string())
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("router response");
        assert_eq!(
            response
                .headers()
                .get("x-request-id")
                .expect("request ID")
                .to_str()
                .expect("ASCII"),
            id.to_string()
        );
    }
}
