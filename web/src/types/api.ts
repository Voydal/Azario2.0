export interface Coordinates {
  latitude: number;
  longitude: number;
}

export interface ParkingSearchRequest {
  origin: Coordinates;
  destination_address: string;
}

export interface ParkingSearchWarning {
  code: string;
  message: string;
}

export interface ParkingSearchSuccess {
  destination: {
    input_address: string;
    formatted_address: string;
    latitude: number;
    longitude: number;
    place_id: string | null;
  };
  parking_spot: {
    id: string;
    latitude: number;
    longitude: number;
    observed_at: string;
    distance_to_destination_m: number;
  };
  ranking: {
    driving_duration_s: number;
    walking_duration_s: number;
    score_s: number;
  };
  walk: {
    distance_m: number;
    duration_s: number;
  };
  route: {
    distance_m: number;
    duration_s: number;
    encoded_polyline: string;
  };
  warnings: ParkingSearchWarning[];
}

export type ParkingSearchNoResultReason =
  | "no_available_parking"
  | "no_reachable_parking";

export interface ParkingSearchNoResult {
  result: null;
  reason: ParkingSearchNoResultReason;
}

export type ParkingSearchResponse = ParkingSearchSuccess | ParkingSearchNoResult;

export function isNoResult(response: ParkingSearchResponse): response is ParkingSearchNoResult {
  return "result" in response && response.result === null;
}
