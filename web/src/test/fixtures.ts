import type { ParkingSearchSuccess } from "../types/api";

export const successResponse: ParkingSearchSuccess = {
  destination: {
    input_address: "Marszałkowska 10, Warszawa",
    formatted_address: "Marszałkowska 10, 00-590 Warszawa, Poland",
    latitude: 52.225,
    longitude: 21.015,
    place_id: "place-1",
  },
  parking_spot: {
    id: "019c1234-1234-7000-8000-123456789abe",
    latitude: 52.224,
    longitude: 21.014,
    observed_at: "2026-09-22T10:00:00Z",
    distance_to_destination_m: 180,
  },
  ranking: {
    driving_duration_s: 275,
    walking_duration_s: 145,
    score_s: 420,
  },
  walk: {
    distance_m: 180,
    duration_s: 145,
  },
  route: {
    distance_m: 1840,
    duration_s: 275,
    encoded_polyline: "encoded",
  },
  warnings: [
    {
      code: "walking_routes_beta",
      message: "Walking routes are beta and may not always include clear pedestrian paths.",
    },
  ],
};
