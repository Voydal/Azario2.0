import { useEffect, useRef, useState } from "react";
import type { Coordinates, ParkingSearchSuccess } from "../types/api";
import { loadGoogleMaps, type GoogleMapsLibraries } from "./googleMaps";

interface ParkingMapProps {
  origin?: Coordinates;
  result?: ParkingSearchSuccess;
}

const apiKey = import.meta.env.VITE_GOOGLE_MAPS_API_KEY?.trim();
const mapId = import.meta.env.VITE_GOOGLE_MAP_ID?.trim() || "DEMO_MAP_ID";

export function ParkingMap({ origin, result }: ParkingMapProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const mapRef = useRef<google.maps.Map | undefined>(undefined);
  const markersRef = useRef<google.maps.marker.AdvancedMarkerElement[]>([]);
  const polylineRef = useRef<google.maps.Polyline | undefined>(undefined);
  const [libraries, setLibraries] = useState<GoogleMapsLibraries>();
  const [mapError, setMapError] = useState<string>();

  function clearResult() {
    for (const marker of markersRef.current) marker.map = null;
    markersRef.current = [];
    polylineRef.current?.setMap(null);
    polylineRef.current = undefined;
  }

  useEffect(() => {
    if (!apiKey) {
      setMapError("Map is not configured");
      return;
    }
    let active = true;
    loadGoogleMaps(apiKey)
      .then((loaded) => {
        if (!active || !containerRef.current) return;
        mapRef.current = new loaded.maps.Map(containerRef.current, {
          center: { lat: 52.2297, lng: 21.0122 },
          zoom: 12,
          mapId,
          mapTypeControl: false,
          streetViewControl: false,
        });
        setLibraries(loaded);
      })
      .catch(() => {
        if (active) setMapError("The map could not be loaded. Search results remain available below.");
      });
    return () => {
      active = false;
      clearResult();
      mapRef.current = undefined;
    };
  }, []);

  useEffect(() => {
    clearResult();
    const map = mapRef.current;
    if (!map || !libraries || !origin || !result) return;

    const originPosition = { lat: origin.latitude, lng: origin.longitude };
    const destinationPosition = {
      lat: result.destination.latitude,
      lng: result.destination.longitude,
    };
    const parkingPosition = {
      lat: result.parking_spot.latitude,
      lng: result.parking_spot.longitude,
    };
    markersRef.current = [
      new libraries.marker.AdvancedMarkerElement({ map, position: originPosition, title: "Current location" }),
      new libraries.marker.AdvancedMarkerElement({ map, position: destinationPosition, title: "Destination" }),
      new libraries.marker.AdvancedMarkerElement({ map, position: parkingPosition, title: "Selected parking spot" }),
    ];

    const bounds = new google.maps.LatLngBounds();
    bounds.extend(originPosition);
    bounds.extend(destinationPosition);
    bounds.extend(parkingPosition);
    try {
      const path = libraries.geometry.encoding.decodePath(result.route.encoded_polyline);
      if (path.length > 0) {
        polylineRef.current = new libraries.maps.Polyline({
          map,
          path,
          strokeColor: "#2563eb",
          strokeOpacity: 0.9,
          strokeWeight: 5,
        });
        for (const point of path) bounds.extend(point);
      }
    } catch {
      // Markers and the textual result remain useful when a polyline is malformed.
    }
    map.fitBounds(bounds, 48);
  }, [libraries, origin, result]);

  return (
    <section className="map-panel" aria-label="Parking route map">
      {mapError && <p className="map-message" role="status">{mapError}</p>}
      <div ref={containerRef} className="map" aria-label="Map showing the parking route" />
    </section>
  );
}
