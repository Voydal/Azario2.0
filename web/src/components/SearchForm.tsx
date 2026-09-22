import { useState, type FormEvent } from "react";
import type { ParkingSearchRequest } from "../types/api";

interface SearchFormProps {
  isLoading: boolean;
  onSearch: (request: ParkingSearchRequest) => void;
}

function geolocationMessage(error: GeolocationPositionError): string {
  switch (error.code) {
    case error.PERMISSION_DENIED:
      return "Location permission was denied. Enter coordinates manually.";
    case error.POSITION_UNAVAILABLE:
      return "Your location is unavailable. Enter coordinates manually.";
    case error.TIMEOUT:
      return "Finding your location timed out. Try again or enter coordinates manually.";
    default:
      return "Your location could not be determined.";
  }
}

export function SearchForm({ isLoading, onSearch }: SearchFormProps) {
  const [destination, setDestination] = useState("");
  const [latitude, setLatitude] = useState("");
  const [longitude, setLongitude] = useState("");
  const [error, setError] = useState<string>();
  const [isLocating, setIsLocating] = useState(false);

  function useMyLocation() {
    setError(undefined);
    if (!navigator.geolocation) {
      setError("This browser does not support geolocation. Enter coordinates manually.");
      return;
    }
    setIsLocating(true);
    navigator.geolocation.getCurrentPosition(
      (position) => {
        setLatitude(String(position.coords.latitude));
        setLongitude(String(position.coords.longitude));
        setIsLocating(false);
      },
      (geolocationError) => {
        setError(geolocationMessage(geolocationError));
        setIsLocating(false);
      },
      { enableHighAccuracy: true, timeout: 8_000, maximumAge: 30_000 },
    );
  }

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const parsedLatitude = Number(latitude);
    const parsedLongitude = Number(longitude);
    if (destination.trim() === "") {
      setError("Enter a destination address.");
      return;
    }
    if (latitude.trim() === "" || !Number.isFinite(parsedLatitude) || parsedLatitude < -90 || parsedLatitude > 90) {
      setError("Latitude must be a number between -90 and 90.");
      return;
    }
    if (longitude.trim() === "" || !Number.isFinite(parsedLongitude) || parsedLongitude < -180 || parsedLongitude > 180) {
      setError("Longitude must be a number between -180 and 180.");
      return;
    }
    setError(undefined);
    onSearch({
      origin: { latitude: parsedLatitude, longitude: parsedLongitude },
      destination_address: destination.trim(),
    });
  }

  return (
    <form className="search-form" onSubmit={submit} noValidate>
      <div className="field">
        <label htmlFor="destination">Destination address</label>
        <input
          id="destination"
          name="destination"
          type="text"
          value={destination}
          onChange={(event) => setDestination(event.target.value)}
          autoComplete="street-address"
          required
        />
      </div>
      <div className="coordinate-fields">
        <div className="field">
          <label htmlFor="latitude">Origin latitude</label>
          <input
            id="latitude"
            name="latitude"
            type="number"
            step="any"
            value={latitude}
            onChange={(event) => setLatitude(event.target.value)}
            required
          />
        </div>
        <div className="field">
          <label htmlFor="longitude">Origin longitude</label>
          <input
            id="longitude"
            name="longitude"
            type="number"
            step="any"
            value={longitude}
            onChange={(event) => setLongitude(event.target.value)}
            required
          />
        </div>
      </div>
      <div className="form-actions">
        <button type="button" className="secondary" onClick={useMyLocation} disabled={isLocating || isLoading}>
          {isLocating ? "Locating…" : "Use my location"}
        </button>
        <button type="submit" disabled={isLoading || isLocating}>
          {isLoading ? "Searching…" : "Search parking"}
        </button>
      </div>
      {error && <p className="form-error" role="alert">{error}</p>}
    </form>
  );
}
