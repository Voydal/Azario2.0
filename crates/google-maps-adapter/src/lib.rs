use std::time::Duration;

use async_trait::async_trait;
use parking_search::{
    Coordinate, DrivingRoute, ExternalServiceError, GeocodedLocation, Geocoder,
    MatrixElementStatus, RouteMatrixEntry, RouteMatrixProvider, RouteProvider,
    WalkingRouteMatrixEntry,
};
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};

pub const GEOCODING_ENDPOINT: &str = "https://geocode.googleapis.com/v4/geocode/address/";
pub const ROUTE_MATRIX_ENDPOINT: &str =
    "https://routes.googleapis.com/distanceMatrix/v2:computeRouteMatrix";
pub const COMPUTE_ROUTES_ENDPOINT: &str =
    "https://routes.googleapis.com/directions/v2:computeRoutes";
pub const GEOCODING_FIELD_MASK: &str = "results.location,results.formattedAddress,results.placeId";
pub const ROUTE_MATRIX_FIELD_MASK: &str =
    "originIndex,destinationIndex,status,condition,distanceMeters,duration";
pub const COMPUTE_ROUTES_FIELD_MASK: &str =
    "routes.duration,routes.distanceMeters,routes.polyline.encodedPolyline";

#[derive(Clone)]
pub struct GoogleMapsClient {
    client: Client,
    api_key: String,
    geocoding_endpoint: String,
    route_matrix_endpoint: String,
    compute_routes_endpoint: String,
}

impl GoogleMapsClient {
    pub fn new(api_key: String, timeout: Duration) -> Result<Self, reqwest::Error> {
        Self::with_endpoints(
            api_key,
            timeout,
            GEOCODING_ENDPOINT.into(),
            ROUTE_MATRIX_ENDPOINT.into(),
            COMPUTE_ROUTES_ENDPOINT.into(),
        )
    }

    pub fn with_endpoints(
        api_key: String,
        timeout: Duration,
        geocoding_endpoint: String,
        route_matrix_endpoint: String,
        compute_routes_endpoint: String,
    ) -> Result<Self, reqwest::Error> {
        let client = Client::builder().timeout(timeout).build()?;
        Ok(Self {
            client,
            api_key,
            geocoding_endpoint,
            route_matrix_endpoint,
            compute_routes_endpoint,
        })
    }

    fn request(
        &self,
        method: reqwest::Method,
        url: Url,
        field_mask: &'static str,
    ) -> reqwest::RequestBuilder {
        self.client
            .request(method, url)
            .header("X-Goog-Api-Key", &self.api_key)
            .header("X-Goog-FieldMask", field_mask)
    }

    async fn matrix_elements(
        &self,
        origins: &[Coordinate],
        destinations: &[Coordinate],
        travel_mode: &'static str,
        routing_preference: Option<&'static str>,
    ) -> Result<Vec<MatrixResponseElement>, ExternalServiceError> {
        let request = MatrixRequest {
            origins: origins.iter().copied().map(MatrixWaypoint::new).collect(),
            destinations: destinations
                .iter()
                .copied()
                .map(MatrixWaypoint::new)
                .collect(),
            travel_mode,
            routing_preference,
        };
        let url = Url::parse(&self.route_matrix_endpoint)
            .map_err(|_| ExternalServiceError::MalformedResponse)?;
        let response = self
            .request(reqwest::Method::POST, url, ROUTE_MATRIX_FIELD_MASK)
            .json(&request)
            .send()
            .await
            .map_err(map_reqwest_error)?;
        let response = checked_response(response)?;
        response
            .json()
            .await
            .map_err(|_| ExternalServiceError::MalformedResponse)
    }
}

#[async_trait]
impl Geocoder for GoogleMapsClient {
    async fn geocode(
        &self,
        address: &str,
    ) -> Result<Option<GeocodedLocation>, ExternalServiceError> {
        let mut url = Url::parse(&self.geocoding_endpoint)
            .map_err(|_| ExternalServiceError::MalformedResponse)?;
        url.path_segments_mut()
            .map_err(|_| ExternalServiceError::MalformedResponse)?
            .push(address);
        let response = self
            .request(reqwest::Method::GET, url, GEOCODING_FIELD_MASK)
            .send()
            .await
            .map_err(map_reqwest_error)?;
        let response = checked_response(response)?;
        let payload: GeocodeResponse = response
            .json()
            .await
            .map_err(|_| ExternalServiceError::MalformedResponse)?;
        payload
            .results
            .into_iter()
            .next()
            .map(|result| {
                Ok(GeocodedLocation {
                    coordinates: Coordinate::new(
                        result.location.latitude,
                        result.location.longitude,
                    )
                    .map_err(|_| ExternalServiceError::MalformedResponse)?,
                    formatted_address: result.formatted_address,
                    place_id: result.place_id,
                })
            })
            .transpose()
    }
}

#[async_trait]
impl RouteMatrixProvider for GoogleMapsClient {
    async fn driving_costs(
        &self,
        origin: Coordinate,
        destinations: &[Coordinate],
    ) -> Result<Vec<RouteMatrixEntry>, ExternalServiceError> {
        let elements = self
            .matrix_elements(&[origin], destinations, "DRIVE", Some("TRAFFIC_AWARE"))
            .await?;

        let mut seen_destinations = vec![false; destinations.len()];
        elements
            .into_iter()
            .map(|element| {
                if element.origin_index != 0 || element.destination_index >= destinations.len() {
                    return Err(ExternalServiceError::MalformedResponse);
                }
                if seen_destinations[element.destination_index] {
                    return Err(ExternalServiceError::MalformedResponse);
                }
                seen_destinations[element.destination_index] = true;
                let reachable = element.status.code.unwrap_or(0) == 0
                    && element.condition.as_deref() == Some("ROUTE_EXISTS");
                if !reachable {
                    return Ok(RouteMatrixEntry {
                        destination_index: element.destination_index,
                        distance_m: 0,
                        duration_s: 0,
                        status: MatrixElementStatus::Unreachable,
                    });
                }
                Ok(RouteMatrixEntry {
                    destination_index: element.destination_index,
                    distance_m: element
                        .distance_meters
                        .ok_or(ExternalServiceError::MalformedResponse)?,
                    duration_s: parse_duration(
                        element
                            .duration
                            .as_deref()
                            .ok_or(ExternalServiceError::MalformedResponse)?,
                    )?,
                    status: MatrixElementStatus::Reachable,
                })
            })
            .collect()
    }

    async fn walking_costs(
        &self,
        origins: &[Coordinate],
        destination: Coordinate,
    ) -> Result<Vec<WalkingRouteMatrixEntry>, ExternalServiceError> {
        let elements = self
            .matrix_elements(origins, &[destination], "WALK", None)
            .await?;
        let mut seen_origins = vec![false; origins.len()];
        elements
            .into_iter()
            .map(|element| {
                if element.destination_index != 0 || element.origin_index >= origins.len() {
                    return Err(ExternalServiceError::MalformedResponse);
                }
                if seen_origins[element.origin_index] {
                    return Err(ExternalServiceError::MalformedResponse);
                }
                seen_origins[element.origin_index] = true;
                let reachable = element.status.code.unwrap_or(0) == 0
                    && element.condition.as_deref() == Some("ROUTE_EXISTS");
                if !reachable {
                    return Ok(WalkingRouteMatrixEntry {
                        origin_index: element.origin_index,
                        distance_m: 0,
                        duration_s: 0,
                        status: MatrixElementStatus::Unreachable,
                    });
                }
                Ok(WalkingRouteMatrixEntry {
                    origin_index: element.origin_index,
                    distance_m: element
                        .distance_meters
                        .ok_or(ExternalServiceError::MalformedResponse)?,
                    duration_s: parse_duration(
                        element
                            .duration
                            .as_deref()
                            .ok_or(ExternalServiceError::MalformedResponse)?,
                    )?,
                    status: MatrixElementStatus::Reachable,
                })
            })
            .collect()
    }
}

#[async_trait]
impl RouteProvider for GoogleMapsClient {
    async fn driving_route(
        &self,
        origin: Coordinate,
        destination: Coordinate,
    ) -> Result<DrivingRoute, ExternalServiceError> {
        let request = RoutesRequest {
            origin: Waypoint::new(origin),
            destination: Waypoint::new(destination),
            travel_mode: "DRIVE",
            routing_preference: "TRAFFIC_AWARE",
            compute_alternative_routes: false,
        };
        let url = Url::parse(&self.compute_routes_endpoint)
            .map_err(|_| ExternalServiceError::MalformedResponse)?;
        let response = self
            .request(reqwest::Method::POST, url, COMPUTE_ROUTES_FIELD_MASK)
            .json(&request)
            .send()
            .await
            .map_err(map_reqwest_error)?;
        let response = checked_response(response)?;
        let payload: RoutesResponse = response
            .json()
            .await
            .map_err(|_| ExternalServiceError::MalformedResponse)?;
        let route = payload
            .routes
            .into_iter()
            .next()
            .ok_or(ExternalServiceError::MalformedResponse)?;
        Ok(DrivingRoute {
            distance_m: route.distance_meters,
            duration_s: parse_duration(&route.duration)?,
            encoded_polyline: route.polyline.encoded_polyline,
        })
    }
}

fn checked_response(
    response: reqwest::Response,
) -> Result<reqwest::Response, ExternalServiceError> {
    if response.status() == StatusCode::TOO_MANY_REQUESTS {
        Err(ExternalServiceError::QuotaExceeded)
    } else if response.status().is_success() {
        Ok(response)
    } else {
        Err(ExternalServiceError::Upstream)
    }
}

fn map_reqwest_error(error: reqwest::Error) -> ExternalServiceError {
    if error.is_timeout() {
        ExternalServiceError::Timeout
    } else {
        ExternalServiceError::Upstream
    }
}

fn parse_duration(value: &str) -> Result<u64, ExternalServiceError> {
    let seconds = value
        .strip_suffix('s')
        .ok_or(ExternalServiceError::MalformedResponse)?
        .parse::<f64>()
        .map_err(|_| ExternalServiceError::MalformedResponse)?;
    if !seconds.is_finite() || seconds < 0.0 || seconds > u64::MAX as f64 {
        return Err(ExternalServiceError::MalformedResponse);
    }
    Ok(seconds.ceil() as u64)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeocodeResponse {
    #[serde(default)]
    results: Vec<GeocodeResult>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeocodeResult {
    location: LatLng,
    formatted_address: String,
    #[serde(default)]
    place_id: Option<String>,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
struct LatLng {
    latitude: f64,
    longitude: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MatrixRequest {
    origins: Vec<MatrixWaypoint>,
    destinations: Vec<MatrixWaypoint>,
    travel_mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    routing_preference: Option<&'static str>,
}

#[derive(Serialize)]
struct MatrixWaypoint {
    waypoint: Waypoint,
}

impl MatrixWaypoint {
    fn new(coordinates: Coordinate) -> Self {
        Self {
            waypoint: Waypoint::new(coordinates),
        }
    }
}

#[derive(Serialize)]
struct Waypoint {
    location: Location,
}

impl Waypoint {
    fn new(coordinates: Coordinate) -> Self {
        Self {
            location: Location {
                lat_lng: LatLng {
                    latitude: coordinates.latitude(),
                    longitude: coordinates.longitude(),
                },
            },
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Location {
    lat_lng: LatLng,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MatrixResponseElement {
    #[serde(default)]
    status: GoogleStatus,
    condition: Option<String>,
    distance_meters: Option<u64>,
    duration: Option<String>,
    #[serde(default)]
    origin_index: usize,
    #[serde(default)]
    destination_index: usize,
}

#[derive(Default, Deserialize)]
struct GoogleStatus {
    code: Option<i32>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RoutesRequest {
    origin: Waypoint,
    destination: Waypoint,
    travel_mode: &'static str,
    routing_preference: &'static str,
    compute_alternative_routes: bool,
}

#[derive(Deserialize)]
struct RoutesResponse {
    #[serde(default)]
    routes: Vec<RouteResponse>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RouteResponse {
    distance_meters: u64,
    duration: String,
    polyline: Polyline,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Polyline {
    encoded_polyline: String,
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use parking_search::{Geocoder, MatrixElementStatus, RouteMatrixProvider, RouteProvider};
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    use super::*;

    async fn client(server: &MockServer, timeout: Duration) -> GoogleMapsClient {
        GoogleMapsClient::with_endpoints(
            "secret-key".into(),
            timeout,
            format!("{}/geocode/address/", server.uri()),
            format!("{}/distanceMatrix/v2:computeRouteMatrix", server.uri()),
            format!("{}/routes", server.uri()),
        )
        .unwrap()
    }

    fn coordinate(latitude: f64, longitude: f64) -> Coordinate {
        Coordinate::new(latitude, longitude).unwrap()
    }

    #[tokio::test]
    async fn geocode_uses_v4_shape_headers_and_first_result() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "results": [{
                    "location": {"latitude": 52.23, "longitude": 21.01},
                    "formattedAddress": "Warszawa, Polska",
                    "placeId": "abc"
                }]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let result = client(&server, Duration::from_secs(1))
            .await
            .geocode("Marszałkowska 1/2")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.coordinates, coordinate(52.23, 21.01));
        assert_eq!(result.formatted_address, "Warszawa, Polska");
        let requests = server.received_requests().await.unwrap();
        assert!(
            requests[0]
                .url
                .path()
                .contains("Marsza%C5%82kowska%201%2F2")
        );
        assert!(!requests[0].url.as_str().contains("secret-key"));
        assert_eq!(
            requests[0].headers.get("x-goog-api-key").unwrap(),
            "secret-key"
        );
        assert_eq!(
            requests[0].headers.get("x-goog-fieldmask").unwrap(),
            GEOCODING_FIELD_MASK
        );
    }

    #[tokio::test]
    async fn geocode_empty_results_returns_none() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": []})))
            .mount(&server)
            .await;
        assert!(
            client(&server, Duration::from_secs(1))
                .await
                .geocode("unknown")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn matrix_maps_success_and_per_element_failure() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/distanceMatrix/v2:computeRouteMatrix"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"status": {}, "condition": "ROUTE_EXISTS", "distanceMeters": 1200, "duration": "275s"},
                {"destinationIndex": 1, "status": {"code": 5}, "condition": "ROUTE_NOT_FOUND"}
            ])))
            .mount(&server)
            .await;
        let entries = client(&server, Duration::from_secs(1))
            .await
            .driving_costs(
                coordinate(52.0, 21.0),
                &[coordinate(52.1, 21.1), coordinate(52.2, 21.2)],
            )
            .await
            .unwrap();
        assert_eq!(entries[0].duration_s, 275);
        assert_eq!(entries[0].status, MatrixElementStatus::Reachable);
        assert_eq!(entries[1].status, MatrixElementStatus::Unreachable);
        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests[0].headers.get("x-goog-fieldmask").unwrap(),
            ROUTE_MATRIX_FIELD_MASK
        );
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["travelMode"], "DRIVE");
        assert_eq!(body["routingPreference"], "TRAFFIC_AWARE");
    }

    #[tokio::test]
    async fn walking_route_matrix_success() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/distanceMatrix/v2:computeRouteMatrix"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"originIndex": 1, "status": {}, "condition": "ROUTE_EXISTS", "distanceMeters": 400, "duration": "300s"},
                {"status": {}, "condition": "ROUTE_EXISTS", "distanceMeters": 200, "duration": "150s"}
            ])))
            .mount(&server)
            .await;
        let entries = client(&server, Duration::from_secs(1))
            .await
            .walking_costs(
                &[coordinate(52.1, 21.1), coordinate(52.2, 21.2)],
                coordinate(52.3, 21.3),
            )
            .await
            .unwrap();
        assert_eq!(entries[0].origin_index, 1);
        assert_eq!(entries[0].distance_m, 400);
        assert_eq!(entries[0].duration_s, 300);
        assert_eq!(entries[0].status, MatrixElementStatus::Reachable);
        assert_eq!(entries[1].origin_index, 0);

        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests[0].headers.get("x-goog-fieldmask").unwrap(),
            ROUTE_MATRIX_FIELD_MASK
        );
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["travelMode"], "WALK");
        assert!(body.get("routingPreference").is_none());
        assert_eq!(body["origins"].as_array().unwrap().len(), 2);
        assert_eq!(body["destinations"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn walking_route_matrix_handles_per_element_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/distanceMatrix/v2:computeRouteMatrix"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"originIndex": 0, "destinationIndex": 0, "status": {}, "condition": "ROUTE_EXISTS", "distanceMeters": 200, "duration": "150s"},
                {"originIndex": 1, "destinationIndex": 0, "status": {"code": 5}, "condition": "ROUTE_NOT_FOUND"}
            ])))
            .mount(&server)
            .await;
        let entries = client(&server, Duration::from_secs(1))
            .await
            .walking_costs(
                &[coordinate(52.1, 21.1), coordinate(52.2, 21.2)],
                coordinate(52.3, 21.3),
            )
            .await
            .unwrap();
        assert_eq!(entries[0].status, MatrixElementStatus::Reachable);
        assert_eq!(entries[1].status, MatrixElementStatus::Unreachable);
    }

    #[tokio::test]
    async fn malformed_walking_duration_is_reported() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/distanceMatrix/v2:computeRouteMatrix"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
                "originIndex": 0,
                "destinationIndex": 0,
                "status": {},
                "condition": "ROUTE_EXISTS",
                "distanceMeters": 200,
                "duration": "banana"
            }])))
            .mount(&server)
            .await;
        assert_eq!(
            client(&server, Duration::from_secs(1))
                .await
                .walking_costs(&[coordinate(52.1, 21.1)], coordinate(52.3, 21.3))
                .await,
            Err(ExternalServiceError::MalformedResponse)
        );
    }

    #[tokio::test]
    async fn compute_routes_maps_success() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/routes"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "routes": [{"distanceMeters": 772, "duration": "165s", "polyline": {"encodedPolyline": "abc"}}]
            })))
            .mount(&server)
            .await;
        let route = client(&server, Duration::from_secs(1))
            .await
            .driving_route(coordinate(52.0, 21.0), coordinate(52.1, 21.1))
            .await
            .unwrap();
        assert_eq!(route.distance_m, 772);
        assert_eq!(route.duration_s, 165);
        assert_eq!(route.encoded_polyline, "abc");
        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests[0].headers.get("x-goog-fieldmask").unwrap(),
            COMPUTE_ROUTES_FIELD_MASK
        );
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["travelMode"], "DRIVE");
        assert_eq!(body["routingPreference"], "TRAFFIC_AWARE");
        assert_eq!(body["computeAlternativeRoutes"], false);
    }

    #[tokio::test]
    async fn malformed_response_is_reported() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/routes"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not-json"))
            .mount(&server)
            .await;
        assert_eq!(
            client(&server, Duration::from_secs(1))
                .await
                .driving_route(coordinate(52.0, 21.0), coordinate(52.1, 21.1))
                .await,
            Err(ExternalServiceError::MalformedResponse)
        );
    }

    #[tokio::test]
    async fn http_500_is_reported_without_exposing_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500).set_body_string("secret upstream detail"))
            .mount(&server)
            .await;
        assert_eq!(
            client(&server, Duration::from_secs(1))
                .await
                .geocode("address")
                .await,
            Err(ExternalServiceError::Upstream)
        );
    }

    #[tokio::test]
    async fn http_429_is_reported_as_quota_failure() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;
        assert_eq!(
            client(&server, Duration::from_secs(1))
                .await
                .geocode("address")
                .await,
            Err(ExternalServiceError::QuotaExceeded)
        );
    }

    #[tokio::test]
    async fn timeout_is_reported() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(100)))
            .mount(&server)
            .await;
        assert_eq!(
            client(&server, Duration::from_millis(10))
                .await
                .geocode("address")
                .await,
            Err(ExternalServiceError::Timeout)
        );
    }

    #[test]
    fn malformed_duration_is_reported() {
        assert_eq!(
            parse_duration("not-a-duration"),
            Err(ExternalServiceError::MalformedResponse)
        );
        assert_eq!(parse_duration("3.5s"), Ok(4));
    }

    #[tokio::test]
    async fn matrix_rejects_out_of_range_destination_index() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/distanceMatrix/v2:computeRouteMatrix"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
                "originIndex": 0,
                "destinationIndex": 1,
                "status": {},
                "condition": "ROUTE_EXISTS",
                "distanceMeters": 1200,
                "duration": "275s"
            }])))
            .mount(&server)
            .await;
        assert_eq!(
            client(&server, Duration::from_secs(1))
                .await
                .driving_costs(coordinate(52.0, 21.0), &[coordinate(52.1, 21.1)])
                .await,
            Err(ExternalServiceError::MalformedResponse)
        );
    }
}
