import { isNoResult, type ParkingSearchResponse } from "../types/api";
import { WarningBanner } from "./WarningBanner";

interface SearchResultProps {
  response: ParkingSearchResponse;
}

export function formatDuration(seconds: number): string {
  const wholeSeconds = Math.max(0, Math.round(seconds));
  const minutes = Math.floor(wholeSeconds / 60);
  const remainder = wholeSeconds % 60;
  if (minutes === 0) return `${remainder} s`;
  if (remainder === 0) return `${minutes} min`;
  return `${minutes} min ${remainder} s`;
}

export function formatDistance(meters: number): string {
  if (meters < 1_000) return `${Math.round(meters)} m`;
  return `${(meters / 1_000).toFixed(1)} km`;
}

export function SearchResult({ response }: SearchResultProps) {
  if (isNoResult(response)) {
    return (
      <section className="result-card" aria-live="polite">
        <h2>No parking result</h2>
        <p>
          {response.reason === "no_available_parking"
            ? "No fresh, available parking spots were found near the destination."
            : "Parking spots were found, but no complete driving and walking route is available."}
        </p>
      </section>
    );
  }

  return (
    <section className="result-card" aria-live="polite">
      <h2>Selected parking</h2>
      <p className="destination">Near {response.destination.formatted_address}</p>
      <dl className="metrics">
        <div>
          <dt>Parking coordinates</dt>
          <dd>
            {response.parking_spot.latitude.toFixed(5)}, {response.parking_spot.longitude.toFixed(5)}
          </dd>
        </div>
        <div>
          <dt>Drive</dt>
          <dd>
            {formatDistance(response.route.distance_m)} · {formatDuration(response.route.duration_s)}
          </dd>
        </div>
        <div>
          <dt>Walk</dt>
          <dd>
            {formatDistance(response.walk.distance_m)} · {formatDuration(response.walk.duration_s)}
          </dd>
        </div>
        <div>
          <dt>Estimated total time</dt>
          <dd>{formatDuration(response.ranking.score_s)}</dd>
        </div>
      </dl>
      <WarningBanner warnings={response.warnings} />
    </section>
  );
}
