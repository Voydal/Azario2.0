use std::{error::Error, fmt};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_domain::ParkingSpotId;

use crate::{Coordinate, DrivingRoute};

#[derive(Clone, Debug, PartialEq)]
pub struct GeocodedLocation {
    pub coordinates: Coordinate,
    pub formatted_address: String,
    pub place_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParkingCandidate {
    pub spot_id: ParkingSpotId,
    pub coordinates: Coordinate,
    pub observed_at: DateTime<Utc>,
    pub distance_to_destination_m: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MatrixElementStatus {
    Reachable,
    Unreachable,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RouteMatrixEntry {
    pub destination_index: usize,
    pub distance_m: u64,
    pub duration_s: u64,
    pub status: MatrixElementStatus,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WalkingRouteMatrixEntry {
    pub origin_index: usize,
    pub distance_m: u64,
    pub duration_s: u64,
    pub status: MatrixElementStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalServiceError {
    Timeout,
    QuotaExceeded,
    Upstream,
    MalformedResponse,
}

impl fmt::Display for ExternalServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => formatter.write_str("external service timed out"),
            Self::QuotaExceeded => formatter.write_str("external service quota exceeded"),
            Self::Upstream => formatter.write_str("external service failed"),
            Self::MalformedResponse => {
                formatter.write_str("external service returned invalid data")
            }
        }
    }
}

impl Error for ExternalServiceError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParkingRepositoryError;

impl fmt::Display for ParkingRepositoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("parking candidate repository failed")
    }
}

impl Error for ParkingRepositoryError {}

#[async_trait]
pub trait Geocoder: Send + Sync {
    async fn geocode(
        &self,
        address: &str,
    ) -> Result<Option<GeocodedLocation>, ExternalServiceError>;
}

#[async_trait]
pub trait RouteMatrixProvider: Send + Sync {
    async fn driving_costs(
        &self,
        origin: Coordinate,
        destinations: &[Coordinate],
    ) -> Result<Vec<RouteMatrixEntry>, ExternalServiceError>;

    async fn walking_costs(
        &self,
        origins: &[Coordinate],
        destination: Coordinate,
    ) -> Result<Vec<WalkingRouteMatrixEntry>, ExternalServiceError>;
}

#[async_trait]
pub trait RouteProvider: Send + Sync {
    async fn driving_route(
        &self,
        origin: Coordinate,
        destination: Coordinate,
    ) -> Result<DrivingRoute, ExternalServiceError>;
}

#[async_trait]
pub trait ParkingCandidateRepository: Send + Sync {
    async fn find_candidates(
        &self,
        center: Coordinate,
        radius_m: u32,
        limit: u32,
        now: DateTime<Utc>,
    ) -> Result<Vec<ParkingCandidate>, ParkingRepositoryError>;
}
