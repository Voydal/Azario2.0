import { importLibrary, setOptions } from "@googlemaps/js-api-loader";

export interface GoogleMapsLibraries {
  maps: google.maps.MapsLibrary;
  marker: google.maps.MarkerLibrary;
  geometry: google.maps.GeometryLibrary;
}

let loading: Promise<GoogleMapsLibraries> | undefined;

export function loadGoogleMaps(apiKey: string): Promise<GoogleMapsLibraries> {
  if (!loading) {
    setOptions({ key: apiKey, v: "weekly" });
    loading = Promise.all([
      importLibrary("maps") as Promise<google.maps.MapsLibrary>,
      importLibrary("marker") as Promise<google.maps.MarkerLibrary>,
      importLibrary("geometry") as Promise<google.maps.GeometryLibrary>,
    ]).then(([maps, marker, geometry]) => ({ maps, marker, geometry }));
  }
  return loading;
}
