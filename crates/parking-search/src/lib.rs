mod coordinate;
mod ports;
mod service;

pub use coordinate::{Coordinate, CoordinateError};
pub use ports::{
    ExternalServiceError, GeocodedLocation, Geocoder, MatrixElementStatus, ParkingCandidate,
    ParkingCandidateRepository, ParkingRepositoryError, RouteMatrixEntry, RouteMatrixProvider,
    RouteProvider, WalkingRouteMatrixEntry,
};
pub use service::{
    DEFAULT_OBSERVATION_TTL, DrivingRoute, FindParking, FindParkingConfig, FindParkingError,
    FindParkingOutcome, ParkingSearchResult, RankingDetails, SelectedParkingSpot, WalkingRoute,
};
