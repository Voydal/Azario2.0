use std::{error::Error, fmt, sync::Arc};

use chrono::Duration;
use parking_domain::ParkingSpotId;

use crate::{
    Coordinate, ExternalServiceError, GeocodedLocation, Geocoder, MatrixElementStatus,
    ParkingCandidate, ParkingCandidateRepository, ParkingRepositoryError, RouteMatrixEntry,
    RouteMatrixProvider, RouteProvider,
};

const WALKING_SPEED_METERS_PER_SECOND: f64 = 1.4;
pub const DEFAULT_OBSERVATION_TTL: Duration = Duration::seconds(15);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FindParkingConfig {
    pub search_radius_m: u32,
    pub candidate_limit: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DrivingRoute {
    pub distance_m: u64,
    pub duration_s: u64,
    pub encoded_polyline: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RankingDetails {
    pub drive_duration_s: u64,
    pub drive_distance_m: u64,
    pub walking_proxy_seconds: f64,
    pub score: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectedParkingSpot {
    pub spot_id: ParkingSpotId,
    pub coordinates: Coordinate,
    pub observed_at: chrono::DateTime<chrono::Utc>,
    pub distance_to_destination_m: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParkingSearchResult {
    pub destination: GeocodedLocation,
    pub spot: SelectedParkingSpot,
    pub ranking: RankingDetails,
    pub route: DrivingRoute,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FindParkingOutcome {
    Found(ParkingSearchResult),
    NoCandidates,
    NoReachableParking,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FindParkingError {
    InvalidConfiguration,
    EmptyDestinationAddress,
    AddressNotFound,
    Repository(ParkingRepositoryError),
    External(ExternalServiceError),
}

impl fmt::Display for FindParkingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration => {
                formatter.write_str("invalid parking search configuration")
            }
            Self::EmptyDestinationAddress => formatter.write_str("destination address is empty"),
            Self::AddressNotFound => formatter.write_str("destination address was not found"),
            Self::Repository(error) => error.fmt(formatter),
            Self::External(error) => error.fmt(formatter),
        }
    }
}

impl Error for FindParkingError {}

pub struct FindParking {
    geocoder: Arc<dyn Geocoder>,
    repository: Arc<dyn ParkingCandidateRepository>,
    matrix_provider: Arc<dyn RouteMatrixProvider>,
    route_provider: Arc<dyn RouteProvider>,
    config: FindParkingConfig,
}

impl FindParking {
    pub fn new(
        geocoder: Arc<dyn Geocoder>,
        repository: Arc<dyn ParkingCandidateRepository>,
        matrix_provider: Arc<dyn RouteMatrixProvider>,
        route_provider: Arc<dyn RouteProvider>,
        config: FindParkingConfig,
    ) -> Result<Self, FindParkingError> {
        if config.search_radius_m == 0 || config.candidate_limit == 0 {
            return Err(FindParkingError::InvalidConfiguration);
        }
        Ok(Self {
            geocoder,
            repository,
            matrix_provider,
            route_provider,
            config,
        })
    }

    pub async fn execute(
        &self,
        origin: Coordinate,
        destination_address: &str,
    ) -> Result<FindParkingOutcome, FindParkingError> {
        let destination_address = destination_address.trim();
        if destination_address.is_empty() {
            return Err(FindParkingError::EmptyDestinationAddress);
        }

        let destination = self
            .geocoder
            .geocode(destination_address)
            .await
            .map_err(FindParkingError::External)?
            .ok_or(FindParkingError::AddressNotFound)?;
        let mut candidates = self
            .repository
            .find_candidates(
                destination.coordinates,
                self.config.search_radius_m,
                self.config.candidate_limit,
                chrono::Utc::now(),
            )
            .await
            .map_err(FindParkingError::Repository)?;
        candidates.truncate(self.config.candidate_limit as usize);

        if candidates.is_empty() {
            return Ok(FindParkingOutcome::NoCandidates);
        }

        let candidate_coordinates: Vec<_> = candidates
            .iter()
            .map(|candidate| candidate.coordinates)
            .collect();
        let matrix = self
            .matrix_provider
            .driving_costs(origin, &candidate_coordinates)
            .await
            .map_err(FindParkingError::External)?;

        let Some((candidate, ranking)) = select_best_candidate(&candidates, &matrix) else {
            return Ok(FindParkingOutcome::NoReachableParking);
        };
        let route = self
            .route_provider
            .driving_route(origin, candidate.coordinates)
            .await
            .map_err(FindParkingError::External)?;

        Ok(FindParkingOutcome::Found(ParkingSearchResult {
            destination,
            spot: SelectedParkingSpot {
                spot_id: candidate.spot_id,
                coordinates: candidate.coordinates,
                observed_at: candidate.observed_at,
                distance_to_destination_m: candidate.distance_to_destination_m,
            },
            ranking,
            route,
        }))
    }
}

fn select_best_candidate<'a>(
    candidates: &'a [ParkingCandidate],
    matrix: &[RouteMatrixEntry],
) -> Option<(&'a ParkingCandidate, RankingDetails)> {
    matrix
        .iter()
        .filter(|entry| entry.status == MatrixElementStatus::Reachable)
        .filter_map(|entry| {
            let candidate = candidates.get(entry.destination_index)?;
            let walking_proxy_seconds =
                candidate.distance_to_destination_m / WALKING_SPEED_METERS_PER_SECOND;
            let ranking = RankingDetails {
                drive_duration_s: entry.duration_s,
                drive_distance_m: entry.distance_m,
                walking_proxy_seconds,
                score: entry.duration_s as f64 + walking_proxy_seconds,
            };
            Some((candidate, ranking))
        })
        .min_by(|(left_candidate, left), (right_candidate, right)| {
            left.score
                .total_cmp(&right.score)
                .then_with(|| {
                    left_candidate
                        .distance_to_destination_m
                        .total_cmp(&right_candidate.distance_to_destination_m)
                })
                .then_with(|| left.drive_duration_s.cmp(&right.drive_duration_s))
                .then_with(|| {
                    left_candidate
                        .spot_id
                        .into_uuid()
                        .as_u128()
                        .cmp(&right_candidate.spot_id.into_uuid().as_u128())
                })
        })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    use super::*;

    #[derive(Default)]
    struct Calls {
        geocoded: Mutex<Vec<String>>,
        repository: Mutex<Vec<(Coordinate, u32, u32)>>,
        matrix_destinations: Mutex<Vec<Vec<Coordinate>>>,
        routes: Mutex<Vec<Coordinate>>,
    }

    struct FakeGeocoder {
        calls: Arc<Calls>,
        result: Option<GeocodedLocation>,
    }

    #[async_trait]
    impl Geocoder for FakeGeocoder {
        async fn geocode(
            &self,
            address: &str,
        ) -> Result<Option<GeocodedLocation>, ExternalServiceError> {
            self.calls.geocoded.lock().unwrap().push(address.into());
            Ok(self.result.clone())
        }
    }

    struct FakeRepository {
        calls: Arc<Calls>,
        candidates: Vec<ParkingCandidate>,
    }

    #[async_trait]
    impl ParkingCandidateRepository for FakeRepository {
        async fn find_candidates(
            &self,
            center: Coordinate,
            radius_m: u32,
            limit: u32,
            _now: chrono::DateTime<Utc>,
        ) -> Result<Vec<ParkingCandidate>, ParkingRepositoryError> {
            self.calls
                .repository
                .lock()
                .unwrap()
                .push((center, radius_m, limit));
            Ok(self.candidates.clone())
        }
    }

    struct FakeMatrix {
        calls: Arc<Calls>,
        entries: Vec<RouteMatrixEntry>,
    }

    #[async_trait]
    impl RouteMatrixProvider for FakeMatrix {
        async fn driving_costs(
            &self,
            _origin: Coordinate,
            destinations: &[Coordinate],
        ) -> Result<Vec<RouteMatrixEntry>, ExternalServiceError> {
            self.calls
                .matrix_destinations
                .lock()
                .unwrap()
                .push(destinations.to_vec());
            Ok(self.entries.clone())
        }
    }

    struct FakeRoutes {
        calls: Arc<Calls>,
    }

    #[async_trait]
    impl RouteProvider for FakeRoutes {
        async fn driving_route(
            &self,
            _origin: Coordinate,
            destination: Coordinate,
        ) -> Result<DrivingRoute, ExternalServiceError> {
            self.calls.routes.lock().unwrap().push(destination);
            Ok(DrivingRoute {
                distance_m: 123,
                duration_s: 45,
                encoded_polyline: "polyline".into(),
            })
        }
    }

    fn coordinate(latitude: f64, longitude: f64) -> Coordinate {
        Coordinate::new(latitude, longitude).unwrap()
    }

    fn candidate(id: u128, longitude: f64, destination_distance: f64) -> ParkingCandidate {
        ParkingCandidate {
            spot_id: ParkingSpotId::from_uuid(Uuid::from_u128(id)),
            coordinates: coordinate(52.0, longitude),
            observed_at: Utc.with_ymd_and_hms(2026, 9, 22, 12, 0, 0).unwrap(),
            distance_to_destination_m: destination_distance,
        }
    }

    fn matrix_entry(index: usize, duration_s: u64) -> RouteMatrixEntry {
        RouteMatrixEntry {
            destination_index: index,
            distance_m: 1_000,
            duration_s,
            status: MatrixElementStatus::Reachable,
        }
    }

    fn service(
        candidates: Vec<ParkingCandidate>,
        entries: Vec<RouteMatrixEntry>,
    ) -> (FindParking, Arc<Calls>) {
        let calls = Arc::new(Calls::default());
        let destination = GeocodedLocation {
            coordinates: coordinate(52.2, 21.0),
            formatted_address: "Destination".into(),
            place_id: Some("place-1".into()),
        };
        let service = FindParking::new(
            Arc::new(FakeGeocoder {
                calls: Arc::clone(&calls),
                result: Some(destination),
            }),
            Arc::new(FakeRepository {
                calls: Arc::clone(&calls),
                candidates,
            }),
            Arc::new(FakeMatrix {
                calls: Arc::clone(&calls),
                entries,
            }),
            Arc::new(FakeRoutes {
                calls: Arc::clone(&calls),
            }),
            FindParkingConfig {
                search_radius_m: 800,
                candidate_limit: 25,
            },
        )
        .unwrap();
        (service, calls)
    }

    #[tokio::test]
    async fn search_geocodes_destination() {
        let (service, calls) = service(vec![], vec![]);
        let _ = service
            .execute(coordinate(52.1, 21.1), "  Marszałkowska 1  ")
            .await;
        assert_eq!(*calls.geocoded.lock().unwrap(), ["Marszałkowska 1"]);
    }

    #[tokio::test]
    async fn search_queries_candidates_around_destination() {
        let (service, calls) = service(vec![], vec![]);
        let _ = service.execute(coordinate(52.1, 21.1), "address").await;
        assert_eq!(
            *calls.repository.lock().unwrap(),
            [(coordinate(52.2, 21.0), 800, 25)]
        );
    }

    #[tokio::test]
    async fn search_returns_no_available_parking_when_repository_is_empty() {
        let (service, calls) = service(vec![], vec![]);
        assert_eq!(
            service.execute(coordinate(52.1, 21.1), "address").await,
            Ok(FindParkingOutcome::NoCandidates)
        );
        assert!(calls.matrix_destinations.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn search_excludes_unreachable_matrix_elements() {
        let candidates = vec![candidate(1, 21.1, 10.0), candidate(2, 21.2, 20.0)];
        let entries = vec![
            RouteMatrixEntry {
                status: MatrixElementStatus::Unreachable,
                ..matrix_entry(0, 1)
            },
            matrix_entry(1, 100),
        ];
        let (service, _) = service(candidates, entries);
        let outcome = service
            .execute(coordinate(52.1, 21.1), "address")
            .await
            .unwrap();
        let FindParkingOutcome::Found(result) = outcome else {
            panic!("expected a selected parking spot");
        };
        assert_eq!(result.spot.spot_id.into_uuid(), Uuid::from_u128(2));
    }

    #[tokio::test]
    async fn search_returns_no_reachable_parking_when_all_routes_fail() {
        let candidates = vec![candidate(1, 21.1, 10.0)];
        let entries = vec![RouteMatrixEntry {
            status: MatrixElementStatus::Unreachable,
            ..matrix_entry(0, 1)
        }];
        let (service, calls) = service(candidates, entries);
        assert_eq!(
            service.execute(coordinate(52.1, 21.1), "address").await,
            Ok(FindParkingOutcome::NoReachableParking)
        );
        assert!(calls.routes.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn search_selects_candidate_with_lowest_combined_score() {
        let candidates = vec![
            candidate(1, 21.1, 700.0),
            candidate(2, 21.2, 50.0),
            candidate(3, 21.3, 20.0),
        ];
        let entries = vec![
            matrix_entry(0, 120),
            matrix_entry(1, 180),
            matrix_entry(2, 240),
        ];
        let (service, _) = service(candidates, entries);
        let FindParkingOutcome::Found(result) = service
            .execute(coordinate(52.1, 21.1), "address")
            .await
            .unwrap()
        else {
            panic!("expected a selected parking spot");
        };
        assert_eq!(result.spot.spot_id.into_uuid(), Uuid::from_u128(2));
    }

    #[tokio::test]
    async fn tie_breaking_is_deterministic() {
        let candidates = vec![candidate(2, 21.2, 14.0), candidate(1, 21.1, 14.0)];
        let entries = vec![matrix_entry(0, 100), matrix_entry(1, 100)];
        let (service, _) = service(candidates, entries);
        let FindParkingOutcome::Found(result) = service
            .execute(coordinate(52.1, 21.1), "address")
            .await
            .unwrap()
        else {
            panic!("expected a selected parking spot");
        };
        assert_eq!(result.spot.spot_id.into_uuid(), Uuid::from_u128(1));
    }

    #[tokio::test]
    async fn final_route_is_requested_only_for_selected_candidate() {
        let candidates = vec![candidate(1, 21.1, 700.0), candidate(2, 21.2, 50.0)];
        let entries = vec![matrix_entry(0, 120), matrix_entry(1, 180)];
        let selected = candidates[1].coordinates;
        let (service, calls) = service(candidates, entries);
        let _ = service.execute(coordinate(52.1, 21.1), "address").await;
        assert_eq!(*calls.routes.lock().unwrap(), [selected]);
    }

    #[tokio::test]
    async fn search_defensively_limits_matrix_destinations() {
        let candidates = (1..=30)
            .map(|id| candidate(id, 21.0 + id as f64 / 1_000.0, 10.0))
            .collect();
        let (service, calls) = service(candidates, vec![]);
        let _ = service.execute(coordinate(52.1, 21.1), "address").await;
        assert_eq!(calls.matrix_destinations.lock().unwrap()[0].len(), 25);
    }
}
