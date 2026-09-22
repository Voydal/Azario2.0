import { useRef, useState } from "react";
import { searchParking } from "./api/parkingApi";
import { SearchForm } from "./components/SearchForm";
import { SearchResult } from "./components/SearchResult";
import { ParkingMap } from "./map/ParkingMap";
import { isNoResult, type Coordinates, type ParkingSearchRequest, type ParkingSearchResponse } from "./types/api";
import "./styles.css";

export default function App() {
  const [response, setResponse] = useState<ParkingSearchResponse>();
  const [origin, setOrigin] = useState<Coordinates>();
  const [error, setError] = useState<string>();
  const [isLoading, setIsLoading] = useState(false);
  const requestInFlight = useRef(false);

  async function handleSearch(request: ParkingSearchRequest) {
    if (requestInFlight.current) return;
    requestInFlight.current = true;
    setIsLoading(true);
    setError(undefined);
    setResponse(undefined);
    setOrigin(request.origin);
    try {
      setResponse(await searchParking(request));
    } catch (searchError) {
      setError(searchError instanceof Error ? searchError.message : "Parking search failed.");
    } finally {
      requestInFlight.current = false;
      setIsLoading(false);
    }
  }

  const mapResult = response && !isNoResult(response) ? response : undefined;

  return (
    <main>
      <header className="hero">
        <p className="eyebrow">Parking platform</p>
        <h1>Find parking near your destination</h1>
        <p>Use your location or enter coordinates, then compare the real drive and walk time.</p>
      </header>
      <div className="app-layout">
        <div className="sidebar">
          <SearchForm isLoading={isLoading} onSearch={(request) => void handleSearch(request)} />
          {error && <p className="api-error" role="alert">{error}</p>}
          {response && <SearchResult response={response} />}
        </div>
        <ParkingMap origin={origin} result={mapResult} />
      </div>
    </main>
  );
}
