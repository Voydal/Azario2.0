import { describe, expect, it, vi } from "vitest";
import { successResponse } from "../test/fixtures";
import type { ParkingSearchRequest } from "../types/api";
import { searchParking } from "./parkingApi";

const request: ParkingSearchRequest = {
  origin: { latitude: 52.2297, longitude: 21.0122 },
  destination_address: "Marszałkowska 10, Warszawa",
};

function jsonResponse(payload: unknown, status = 200): Response {
  return new Response(JSON.stringify(payload), {
    status,
    headers: { "content-type": "application/json" },
  });
}

describe("searchParking", () => {
  it("maps a 200 success response", async () => {
    const fetchImpl = vi.fn<typeof fetch>().mockResolvedValue(jsonResponse(successResponse));
    await expect(searchParking(request, { baseUrl: "http://api.test/", fetchImpl })).resolves.toEqual(successResponse);
    expect(fetchImpl).toHaveBeenCalledWith(
      "http://api.test/v1/parking/search",
      expect.objectContaining({ method: "POST", body: JSON.stringify(request) }),
    );
  });

  it("maps a 200 no-result response", async () => {
    const payload = { result: null, reason: "no_available_parking" };
    const fetchImpl = vi.fn<typeof fetch>().mockResolvedValue(jsonResponse(payload));
    await expect(searchParking(request, { fetchImpl })).resolves.toEqual(payload);
  });

  it("maps a 422 response to address not found", async () => {
    const fetchImpl = vi.fn<typeof fetch>().mockResolvedValue(jsonResponse({ error: "address_not_found" }, 422));
    await expect(searchParking(request, { fetchImpl })).rejects.toMatchObject({
      status: 422,
      message: "The destination address could not be found.",
    });
  });

  it("maps a 503 response to unavailable", async () => {
    const fetchImpl = vi.fn<typeof fetch>().mockResolvedValue(jsonResponse({ error: "not_configured" }, 503));
    await expect(searchParking(request, { fetchImpl })).rejects.toMatchObject({
      status: 503,
      message: "Parking search is currently unavailable.",
    });
  });
});
