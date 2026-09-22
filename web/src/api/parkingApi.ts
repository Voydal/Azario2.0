import type {
  ParkingSearchNoResultReason,
  ParkingSearchRequest,
  ParkingSearchResponse,
} from "../types/api";

const configuredBaseUrl = import.meta.env.VITE_PARKING_API_BASE_URL?.trim();
const defaultBaseUrl = configuredBaseUrl || "http://127.0.0.1:3000";

const statusMessages: Readonly<Record<number, string>> = {
  400: "Check the destination and origin coordinates.",
  422: "The destination address could not be found.",
  502: "The route provider is temporarily unavailable.",
  503: "Parking search is currently unavailable.",
  504: "The route provider timed out. Please try again.",
};

export class ParkingApiError extends Error {
  constructor(
    message: string,
    public readonly status: number,
  ) {
    super(message);
    this.name = "ParkingApiError";
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isNoResultReason(value: unknown): value is ParkingSearchNoResultReason {
  return value === "no_available_parking" || value === "no_reachable_parking";
}

function parseResponse(payload: unknown): ParkingSearchResponse {
  if (!isRecord(payload)) {
    throw new ParkingApiError("The server returned an invalid response.", 502);
  }
  if (payload.result === null && isNoResultReason(payload.reason)) {
    return { result: null, reason: payload.reason };
  }
  if (
    isRecord(payload.destination) &&
    isRecord(payload.parking_spot) &&
    isRecord(payload.ranking) &&
    isRecord(payload.walk) &&
    isRecord(payload.route) &&
    Array.isArray(payload.warnings)
  ) {
    return payload as unknown as ParkingSearchResponse;
  }
  throw new ParkingApiError("The server returned an invalid response.", 502);
}

export async function searchParking(
  request: ParkingSearchRequest,
  options: { baseUrl?: string; fetchImpl?: typeof fetch } = {},
): Promise<ParkingSearchResponse> {
  const baseUrl = (options.baseUrl ?? defaultBaseUrl).replace(/\/$/, "");
  const fetchImpl = options.fetchImpl ?? fetch;
  let response: Response;
  try {
    response = await fetchImpl(`${baseUrl}/v1/parking/search`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(request),
    });
  } catch {
    throw new ParkingApiError("Could not connect to the parking service.", 0);
  }

  if (!response.ok) {
    throw new ParkingApiError(
      statusMessages[response.status] ?? "Parking search failed. Please try again.",
      response.status,
    );
  }

  let payload: unknown;
  try {
    payload = await response.json();
  } catch {
    throw new ParkingApiError("The server returned an invalid response.", 502);
  }
  return parseResponse(payload);
}
