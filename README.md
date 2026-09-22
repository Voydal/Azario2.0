# parking-platform

Minimalny backend systemu parkingowego w Rust. Workspace zawiera czystą bibliotekę domenową,
wersjonowane kontrakty zdarzeń, wyszukiwanie i ranking miejsc, persistence w PostgreSQL/PostGIS,
API HTTP, worker ingestion oraz simulator kamery publikujący do NATS JetStream.

## Lokalne uruchomienie

Wymagane są Rust oraz Docker z Docker Compose. Lokalne środowisko używa przypiętych obrazów
`postgis/postgis:17-3.5` oraz `nats:2.15.0-alpine`.

```bash
docker compose up -d
docker compose ps
export DATABASE_URL=postgres://parking:parking@localhost:5432/parking
export NATS_URL=nats://127.0.0.1:4222
export FRONTEND_ORIGIN=http://localhost:5173
cargo run -p parking-api
```

Integracja Google jest opcjonalna przy starcie. Bez `GOOGLE_MAPS_API_KEY` healthcheck i pozostałe
endpointy działają, a `POST /v1/parking/search` zwraca kontrolowany błąd `503`. Konfiguracja:

```bash
export GOOGLE_MAPS_API_KEY='...'          # sekret; nie zapisuj go w repozytorium
export GOOGLE_MAPS_HTTP_TIMEOUT_MS=3000   # domyślnie 3000
export PARKING_SEARCH_RADIUS_M=800        # domyślnie 800
export PARKING_CANDIDATE_LIMIT=25         # domyślnie 25, maksymalnie 625
```

Przy starcie API automatycznie wykonuje migracje SQLx. W środowisku produkcyjnym migracje
powinny docelowo być wykonywane przez osobny proces lub job.

Healthcheck:

```bash
curl -i http://127.0.0.1:3000/health
```

Port `8222` udostępnia lokalny endpoint diagnostyczny NATS, np.
`http://127.0.0.1:8222/healthz`.

Zatrzymanie lokalnej infrastruktury:

```bash
docker compose down
```

Pełny reset lokalnych danych:

```bash
docker compose down -v
```

## Asynchroniczny przepływ ingestion

```text
camera-simulator
  -> NATS JetStream
  -> durable pull consumer
  -> parking-ingestion-worker
  -> PostgreSQL/PostGIS
```

Stream `PARKING_OBSERVATIONS` zapisuje `parking.observations.v1` na dysku z `LimitsPolicy`
i siedmiodniową retencją. Consumer `parking-ingestion-v1` używa `DeliverPolicy::All`, dzięki
czemu przy pierwszym utworzeniu rozpoczyna od najstarszej nadal dostępnej wiadomości.

Delivery jest typu at-least-once. Worker wykonuje ACK dopiero po commitcie PostgreSQL;
przejściowy błąd bazy powoduje NAK i redelivery, a błędny JSON lub nieobsługiwana wersja
kontraktu kończy się TERM. Ponowne dostarczenie po commitcie jest bezpieczne dzięki PK
`event_id`, unikalności `(camera_id, spot_id, sequence)` oraz warunkowemu UPSERT-owi.

## Manualny smoke test

Utworzenie miejsca parkingowego:

```bash
curl -i -X POST http://127.0.0.1:3000/v1/parking-spots \
  -H 'content-type: application/json' \
  -d '{
    "id": "019c1234-1234-7000-8000-123456789abe",
    "latitude": 52.2297,
    "longitude": 21.0122
  }'
```

W drugim terminalu uruchom worker:

```bash
export DATABASE_URL=postgres://parking:parking@localhost:5432/parking
export NATS_URL=nats://127.0.0.1:4222
cargo run -p parking-ingestion-worker
```

W trzecim terminalu opublikuj obserwację FREE:

```bash
export NATS_URL=nats://127.0.0.1:4222
cargo run -p camera-simulator -- \
  --spot-id 019c1234-1234-7000-8000-123456789abe \
  --camera-id 019c1234-1234-7000-8000-123456789abd \
  --sequence 101 \
  --state free
```

Wyszukiwanie powinno zwrócić miejsce:

```bash
curl -i 'http://127.0.0.1:3000/v1/parking-spots/free?lat=52.2297&lon=21.0122&radius_m=500'
```

Następnie opublikuj OCCUPIED:

```bash
cargo run -p camera-simulator -- \
  --spot-id 019c1234-1234-7000-8000-123456789abe \
  --camera-id 019c1234-1234-7000-8000-123456789abd \
  --sequence 102 \
  --state occupied
```

Ponowne wyszukiwanie nie powinno już zwrócić tego miejsca.

Endpoint HTTP pozostaje developerską/debugową, niezależną ścieżką ingestion. Nie publikuje
do NATS i nie wykonuje dual-write:

```bash
curl -i -X POST http://127.0.0.1:3000/v1/observations \
  -H 'content-type: application/json' \
  -d "{
    \"event_id\": \"019c1234-1234-7000-8000-123456789abc\",
    \"camera_id\": \"019c1234-1234-7000-8000-123456789abd\",
    \"spot_id\": \"019c1234-1234-7000-8000-123456789abe\",
    \"sequence\": 42,
    \"observed_at\": \"$(date -u +%Y-%m-%dT%H:%M:%SZ)\",
    \"state\": \"free\",
    \"model_score\": 0.97,
    \"model_version\": \"occupancy-v1\"
  }"
```

## Wyszukiwanie i ranking parkingu

```text
POST /v1/parking/search
        |
        v
Geocoding API v4
        |
        v
PostGIS: świeże i wolne miejsca w promieniu
        |
        v
Routes API v2: Compute Route Matrix
DRIVE: pozycja pojazdu -> miejsca
        |
        v
odrzucenie miejsc niedostępnych samochodem
        |
        v
Routes API v2: Compute Route Matrix
WALK: dostępne miejsca -> cel
        |
        v
odrzucenie miejsc niedostępnych pieszo
        |
        v
ranking DRIVE + WALK
        |
        v
Routes API v2: Compute Routes dla zwycięzcy
        |
        v
odpowiedź z ETA i encoded polyline
```

PostGIS ogranicza liczbę płatnych elementów macierzy przed wywołaniem Google. Dla jednego
wyszukiwania wykonywane są najwyżej dwa wywołania Route Matrix (jedno DRIVE i jedno WALK)
oraz jedno Compute Routes DRIVE dla zwycięzcy.
Wynik geokodowania z wieloma dopasowaniami używa pierwszego wyniku; interaktywne ujednoznacznianie
adresu nie należy jeszcze do tego etapu.

Walking proxy oparty na odległości w linii prostej został usunięty z rankingu. Czas marszu
pochodzi z rzeczywistej macierzy Google z `travelMode=WALK`. Ranking wynosi:

```text
score_s = driving_duration_s + walking_duration_s
```

Remisy rozstrzygają kolejno: krótszy rzeczywisty czas marszu, mniejsza odległość miejsca od celu
według PostGIS, krótszy czas dojazdu i na końcu stabilne sortowanie po identyfikatorze miejsca.

Przy `N` kandydatach macierz DRIVE zawiera maksymalnie `N` elementów, a macierz WALK maksymalnie
kolejne `N`. WALK jest wywoływany dopiero po odrzuceniu miejsc niedostępnych samochodem, więc
rzeczywista liczba elementów może być mniejsza. Domyślny limit 25 oznacza najwyżej 50 elementów
obu macierzy łącznie, bez wykonywania osobnego requestu dla każdego miejsca.

Google oznacza walking routes jako funkcję beta. Odpowiedź sukcesu zawiera ostrzeżenie
`walking_routes_beta`; przyszły frontend prezentujący trasę pieszą musi poinformować użytkownika,
że dane mogą nie obejmować wszystkich chodników i ścieżek pieszych.

Manualny test wymaga działającej infrastruktury, świeżej obserwacji `free` oraz prawdziwego klucza
z włączonymi Geocoding API v4 i Routes API:

```bash
curl -i -X POST http://127.0.0.1:3000/v1/parking/search \
  -H 'content-type: application/json' \
  -d '{
    "origin": {"latitude": 52.2297, "longitude": 21.0122},
    "destination_address": "Marszałkowska 10, Warszawa"
  }'
```

Serwer używa klucza tylko w nagłówku `X-Goog-Api-Key`. Nie przekazuj go w URL ani nie commituj
plików `.env`; są ignorowane przez Git.

## Frontend webowy

Frontend React/TypeScript znajduje się w `web/`. Routing pozostaje wyłącznie w backendzie:
przeglądarka renderuje otrzymane markery i encoded driving polyline, ale nie wywołuje Route Matrix,
Compute Routes ani Directions Service.

```text
SearchForm
  -> parkingApi
  -> POST /v1/parking/search
  -> SearchResult
  -> ParkingMap
```

Konfiguracja developerska:

```bash
cd web
npm install
cp .env.example .env.local

export VITE_GOOGLE_MAPS_API_KEY='browser-key'
export VITE_GOOGLE_MAP_ID=DEMO_MAP_ID
export VITE_PARKING_API_BASE_URL=http://127.0.0.1:3000
npm run dev
```

Aplikacja będzie dostępna pod `http://localhost:5173`. Backend powinien być uruchomiony z:

```bash
export FRONTEND_ORIGIN=http://localhost:5173
```

`GOOGLE_MAPS_API_KEY` jest sekretem serwerowym używanym przez Geocoding i Routes API — nigdy nie
jest przekazywany do przeglądarki. `VITE_GOOGLE_MAPS_API_KEY` jest publicznym kluczem przeglądarkowym
dla Maps JavaScript API. Należy ograniczyć go w Google Cloud wyłącznie do Maps JavaScript API oraz
ustawić HTTP referrer restrictions dla rzeczywistych domen frontendu. Nie commituj `.env.local` ani
żadnego prawdziwego klucza.

Advanced Markers wymagają map ID. `DEMO_MAP_ID` służy wyłącznie do developmentu; środowisko
produkcyjne powinno podawać własne `VITE_GOOGLE_MAP_ID`. Frontend ładuje tylko biblioteki `maps`,
`marker` i `geometry`. Brak klucza albo błąd Maps JavaScript API nie blokuje formularza ani tekstowego
wyniku — zamiast mapy pojawia się kontrolowany komunikat.

Manualny smoke test:

1. Uruchom PostGIS/NATS, backend z oboma zmiennymi `GOOGLE_MAPS_API_KEY` i `FRONTEND_ORIGIN`,
   a następnie frontend z powyższymi zmiennymi `VITE_*`.
2. Otwórz `http://localhost:5173`.
3. Kliknij `Use my location` albo wpisz współrzędne ręcznie, podaj adres i wyszukaj parking.
4. Sprawdź markery origin/destination/parking, trasę DRIVE, podsumowanie DRIVE/WALK i ostrzeżenie
   `walking_routes_beta`.
5. Wykonaj drugi search i sprawdź, że markery oraz polilinia zostały zastąpione.

Google walking routes pozostają beta. Każdy klient prezentujący ich wynik musi pokazać zwrócone
przez backend ostrzeżenie o możliwych brakach chodników i ścieżek pieszych.

## Testy i kontrola jakości

PostGIS i NATS z Compose muszą działać. Testy integracyjne wykonują prawdziwe zapytania do
obu usług.

```bash
export DATABASE_URL=postgres://parking:parking@localhost:5432/parking
export NATS_URL=nats://127.0.0.1:4222
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace

cd web
npm run lint
npm test
npm run build
```

## Założenie MVP

Jedno miejsce parkingowe ma jedno autorytatywne źródło obserwacji w danym czasie. System nie
implementuje sensor fusion ani rozbudowanego ownership kamer. `sequence` ustala kolejność
zdarzeń, a `observed_at` służy wyłącznie do oceny świeżości obserwacji.
