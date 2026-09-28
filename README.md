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

## Edge perception dla nagranego wideo

Etap 7 dodaje alternatywnego producenta tego samego kontraktu zdarzeń; `camera-simulator` pozostaje
dostępny do testowania backendu bez Computer Vision.

```text
recorded video
      |
GStreamer (RGB)
      |
frame sampling
      |
ONNX ParkingDetectorV1 (CPU)
      |
vehicle detections
      |
parking-perception: normalized ROI occupancy
      |
temporal stabilizer
      |
SpotObservationV1
      |
NATS JetStream
      |
parking-ingestion-worker
      |
PostgreSQL
```

`parking-perception` jest czystą biblioteką domenowej logiki CV. Nie zależy od NATS, GStreamer,
ONNX Runtime, SQLx ani Axum. `parking-edge-agent` zawiera źródło klatek, adapter modelu,
konfigurację, trwałą sekwencję i publikację.

### Zależności systemowe i uruchomienie

Bindingi GStreamer 0.25.x wymagają Rust 1.92 oraz developerskich bibliotek GStreamer. Nazwy
pakietów zależą od dystrybucji. Przykładowo na Debianie/Ubuntu są to między innymi:

```bash
sudo apt install pkg-config libglib2.0-dev libgstreamer1.0-dev \
  libgstreamer-plugins-base1.0-dev gstreamer1.0-tools \
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-libav
```

Na Fedorze odpowiednikami są zwykle `pkgconf-pkg-config`, `gstreamer1-devel`,
`gstreamer1-plugins-base-devel` oraz potrzebne zestawy pluginów. Należy korzystać z pakietów
systemowych danej dystrybucji.

Skopiuj i dostosuj przykładową konfigurację. Lokalnego pliku, modelu, wideo i bazy stanu nie należy
commitować:

```bash
cp config/edge-camera.example.toml config/edge-camera.toml
export NATS_URL=nats://127.0.0.1:4222
export PARKING_MODEL_PATH=/absolute/path/to/parking-detector-v1.onnx # opcjonalny override
export PARKING_VIDEO_PATH=/absolute/path/to/parking-camera.mp4 # opcjonalny override
cargo run -p parking-edge-agent -- --config config/edge-camera.toml
```

Przed edge agentem uruchom NATS i `parking-ingestion-worker`, który provisionuje istniejący stream
`PARKING_OBSERVATIONS`. Agent publikuje na istniejący subject `parking.observations.v1`, czeka na
JetStream publish ACK i ustawia `Nats-Msg-Id` na nowe `event_id`. Model nie jest pobierany przy
starcie; brak wskazanego pliku jest czytelnym błędem startupu. Domyślnym i jedynym wspieranym
providerem Etapu 7 jest CPU. ORT jest ładowany dynamicznie, aby build i testy nie pobierały zależnej
od platformy biblioteki binarnej. Przy uruchamianiu ustaw `ORT_DYLIB_PATH` na zgodną bibliotekę
`libonnxruntime.so` (ONNX Runtime dla CPU); brak biblioteki powoduje kontrolowany błąd startupu.

### Algorytm occupancy i stabilizacja

ROI miejsc oraz bounding boxy używają współrzędnych znormalizowanych `0..=1`. Dla każdej detekcji,
której `class_id` znajduje się w konfigurowanym `vehicle_class_ids` i której confidence przekracza
próg, liczony jest:

```text
spot_overlap_ratio = intersection_area(spot_polygon, vehicle_bbox) / spot_polygon_area
occupancy_score = max(detection.confidence * spot_overlap_ratio)
```

To celowo nie jest klasyczne IoU. Wynik `<= free_threshold` oznacza `Free`, wynik
`>= occupied_threshold` oznacza `Occupied`, a martwa strefa między progami oznacza `Uncertain`.
Brak pasującej detekcji daje score `0`, ale wyłącznie dla poprawnie przetworzonej klatki.

Inferencja wykonywana jest najwyżej co `sample_interval_ms`. Nowa klasyfikacja staje się stabilna
dopiero po `stable_samples_required` kolejnych zgodnych próbkach. Event powstaje natychmiast po
zmianie stabilnego stanu albo, bez zmiany, gdy od poprzedniej emisji upłynął
`observation_refresh_interval_ms`. Walidacja wymaga, aby refresh był krótszy od backendowego TTL
15 s; przykład używa 5 s.

Wartości przykładowe: confidence `0.50`, free `0.10`, occupied `0.30`, sampling `500 ms` i trzy
stabilne próbki są wyłącznie początkowymi parametrami eksperymentalnymi. Nie są naukowo optymalne;
docelowo należy je skalibrować na oznaczonym zbiorze walidacyjnym i zgodnie z label map konkretnego
modelu. Przykładowe ID klas `[2, 3, 5, 7]` również muszą zostać dopasowane do label map modelu.

### Kontrakt modelu ParkingDetectorV1

Adapter nie deklaruje zgodności z dowolnym surowym eksportem YOLO. Obsługuje dokładnie model po NMS:

```text
input:  float32 [1, 3, H, W], RGB, CHW, wartości [0,1]
output: float32 [N, 6]
row:    [x_min, y_min, x_max, y_max, confidence, class_id]
bbox:   współrzędne znormalizowane względem oryginalnej klatki
```

Preprocessing przyjmuje spakowany RGB z GStreamer, wykonuje jawny nearest-neighbor stretch resize
do `input_width` × `input_height`, normalizację `[0,255] -> [0,1]`, HWC -> CHW i dodaje batch.
Stretch jest świadomym ograniczeniem MVP: może zniekształcić proporcje. Ponieważ wejście i output
są normalizowane w obu osiach niezależnie, bbox nie wymaga transformacji letterbox. Surowy YOLO
wymagający dekodowania anchors, mapowania tensorów lub NMS potrzebuje osobnego postprocessora.
Niepoprawne bboxy, confidence lub class ID z modelu kończą inferencję tej klatki kontrolowanym
błędem bez panic. Brak detekcji w poprawnym output `[0,6]` jest odrębnym przypadkiem.

### Failure i restart semantics

- Brak/zatrzymanie wideo nie tworzy `Free`: nie ma eventu, refresh ustaje, a backend po TTL przechodzi
  do `UNKNOWN`.
- Błąd pojedynczej inferencji jest logowany i nie aktualizuje stabilizatora. Długotrwały błąd także
  kończy się brakiem refreshu i backendowym `UNKNOWN`.
- Stabilne `Uncertain` jest jawnie publikowane; backend mapuje je na `UNKNOWN` zamiast utrzymywać
  poprzedni stan w nieskończoność.
- Publish jest ponawiany maksymalnie pięć razy z opóźnieniami 100/200/400/800 ms przed piątą próbą.
  Trwały outage jest logowany, a analiza może być kontynuowana; Etap 7 nie ma offline spoolera.
- EOF kończy pipeline bez zapętlania i zamyka GStreamer. Ctrl+C jest obsługiwane między operacjami;
  oczekiwanie na klatkę ma limit 5 s, więc zastoju źródła nie traktujemy jako końca pliku.
- Brak/niezgodność pliku modelu przerywa startup zamiast pozorować działanie.

Camera-wide `sequence` jest zapisywany w lokalnym SQLite wskazanym przez `state_database`. Numer jest
alokowany i commitowany przed pierwszą próbą publikacji. Po restarcie następny numer jest większy od
każdego wcześniej zaalokowanego. Nieudana publikacja może pozostawić lukę, ale numer nigdy nie jest
używany ponownie — backend wymaga monotoniczności, nie ciągłości.

Edge nie publikuje klatek, cropów, twarzy ani tablic rejestracyjnych. Do NATS trafia wyłącznie
`SpotObservationV1` z metadanymi occupancy.

Test GStreamer oparty na `videotestsrc` jest domyślnie ignorowany, ponieważ wymaga bibliotek i
pluginów systemowych. Po ich instalacji uruchom:

```bash
cargo test -p parking-edge-agent gstreamer_pipeline_produces_rgb_frames -- --ignored
```

Opcjonalny live smoke wymaga kompatybilnego modelu `ParkingDetectorV1`, lokalnego pliku wideo,
działającego workera i poprawnie skonfigurowanych ROI. Duże pliki `.onnx`, `.mp4`, katalog
`artifacts/`, lokalna konfiguracja oraz `edge-state.db*` są ignorowane przez Git.

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

## Perception evaluation

Etap 7.1 używa osobnego `perception-evaluator`: czyta wcześniej zapisane detekcje i niezależny
ground truth, a scoring ROI i stabilizację wykonuje przez ten sam crate `parking-perception` co
edge. Nie uruchamia ONNX ani nie zmienia konfiguracji produkcyjnej. Najpierw wygeneruj
`detections.jsonl` raz przy ustalonej częstości próbkowania, przygotuj niezależne
`ground_truth.jsonl`, a następnie uruchamiaj wiele porównań progów na tych samych danych.
Generowanie dumpu detekcji z wideo opisuje sekcja „Real-data evaluation workflow” poniżej.

Manifest `dataset.toml` ma `schema_version = 1`, nazwę, wersję, opis i listę `[[videos]]`
z `video_id`, `camera_id`, `detections_path`, `ground_truth_path` i
`roi_config_path`. Ścieżki są względne wobec manifestu. ROI wskazuje zwykłą konfigurację
edge (te same `[[spots]]`, `polygon`, `vehicle_class_ids` i refresh interval).
Każdy wiersz JSONL ma `schema_version: 1`. Detekcja zawiera `video_id`, `camera_id`,
`frame_index`, `timestamp_ms` i listę detekcji z `class_id`, `confidence` oraz
znormalizowanym bbox (`x_min`, `y_min`, `x_max`, `y_max`). Ground truth zawiera
`video_id`, `spot_id`, `timestamp_ms` i `state`: `free`, `occupied` lub `unknown`.
Mały syntetyczny przykład jest w `fixtures/perception-eval/`; nie jest reprezentatywnym
zbiorem parkingowym. Dużych filmów, klatek i dumpów nie commitujemy
(`datasets/raw/`, `datasets/generated/`, `evaluation-output/` są ignorowane).

```bash
cargo run -p perception-evaluator -- evaluate \
  --dataset fixtures/perception-eval/dataset.toml \
  --output evaluation-output/fixture-evaluate \
  --confidence 0.5 --free 0.1 --occupied 0.3 --stable-samples 3

cargo run -p perception-evaluator -- sweep \
  --dataset fixtures/perception-eval/dataset.toml \
  --output evaluation-output/fixture-sweep \
  --confidence-values 0.4,0.5,0.6 \
  --free-values 0.05,0.10,0.15 \
  --occupied-values 0.25,0.30,0.35 \
  --stable-samples-values 2,3,4
```

Dostępne są `--ground-truth-tolerance-ms` (domyślnie 250) i
`--transition-timeout-ms` (domyślnie 10000). Etykieta jest dopasowana po
`video_id + spot_id + najbliższy timestamp`; remis rozstrzyga wcześniejsza adnotacja.
Nieoznaczone próbki i `unknown` są raportowane, ale wyłączone z klasyfikacji.
`Unknown` przerywa ciąg znanych etykiet przy wykrywaniu transitions — zmiana przez lukę
unknown nie ma przypisanej dokładnej latencji. Pierwsze próbki bez ustalonego stanu stabilnego
są oceniane jako prediction `UNCERTAIN`.

`results.csv` ma jeden wiersz na poprawną konfigurację z oddzielnymi kolumnami `frame_*`
i metrykami stanu stabilnego. `per_camera.csv` ma przekrój dla każdej kamery. `summary.json` zawiera
wersję schematu, balans klas, metryki frame/stabilized dla pojedynczej ewaluacji lub wybranej
konfiguracji, metryki per-camera, SHA-256 wejść i liczbę pominiętych kombinacji.
`evaluation-config.json` zapisuje wszystkie parametry. Dla pełnej powtarzalności
`evaluation_timestamp` jest domyślnie `null`; można podać jawny znacznik RFC3339 przez
`--evaluation-timestamp 2026-01-01T00:00:00Z`.

Analizuj przede wszystkim false-free (zajęte miejsce błędnie pokazane jako wolne), coverage,
F1 obu klas oraz latencję zmian. `UNCERTAIN` zmniejsza coverage, ale szerszy obszar
niepewności może ograniczyć kosztowne błędne decyzje. Strict accuracy jest pomocnicza i
może mylić przy niezbalansowanych klasach. Bez limitów narzędzie nie wskazuje zwycięzcy;
`--max-false-free-rate` i/lub `--min-coverage` włączają jawny wybór najwyższego macro F1
spośród konfiguracji spełniających ograniczenia. Limitów nie narzucamy z góry.

Wynik sweep dotyczy tylko użytego datasetu i cadence detekcji, nie jest globalnie
najlepszym zestawem progów. Strojenie i końcowy raport na tych samych danych zawyżają ocenę:
docelowo należy mieć oddzielny zbiór kalibracyjny/walidacyjny oraz held-out test.

## Real-data evaluation workflow

Etap 7.2A łączy istniejące elementy:

```text
recorded video -> GStreamer -> frame sampling -> existing OnnxDetector
               -> raw Detection[] -> detections.jsonl -> perception-evaluator
```

1. Przygotuj model zgodny **dokładnie** z kontraktem `ParkingDetectorV1` opisanym wyżej
   (wejście float32 `[1,3,H,W]`, RGB/CHW/stretch, output float32 `[N,6]` po NMS).
   Nie wystarczy dowolny eksport YOLO ONNX.
2. Przygotuj lokalny plik nagrania. Model i film mogą znajdować się poza repozytorium;
   do uruchomienia potrzebna jest także lokalna biblioteka CPU ONNX Runtime wskazana przez
   `ORT_DYLIB_PATH`.
3. Wygeneruj detekcje jednokrotnie:

```bash
export ORT_DYLIB_PATH=/path/to/libonnxruntime.so
cargo run -p perception-evaluator -- generate-detections \
  --video /path/to/recording.mp4 \
  --model /path/to/parking-detector-v1.onnx \
  --camera-id 11111111-1111-1111-1111-111111111111 \
  --video-id real-run-001 \
  --sample-interval-ms 500 \
  --input-width 640 --input-height 640 \
  --output datasets/real-run-001/detections.jsonl
```

Można zamiast ścieżek i parametrów użyć `--config config/edge-camera.toml` z istniejącym
formatem edge; `--video-id` i `--output` nadal są wymagane. Precedence:
jawne argumenty CLI > `PARKING_MODEL_PATH`/`PARKING_VIDEO_PATH` > edge config;
bez configu domyślny sampling to 500 ms i rozmiar wejścia 640×640.
`--detector-output-floor` ma domyślnie 0.0 i nie jest produkcyjnym
`detection_confidence_threshold`. Generator nie filtruje `vehicle_class_ids`:
zachowuje wszystkie klasy i niskie confidence, żeby późniejszy sweep nie wymagał ponownej
inferencji. `--force` jawnie zezwala na zastąpienie istniejącego outputu; domyślnie
nadpisanie jest zabronione.

4. Przygotuj niezależne, ręcznie lub zewnętrznie oznaczone `ground_truth.jsonl` na poziomie
   spot + timestamp. Generator nie tworzy ground truth.
5. Utwórz/zaktualizuj `dataset.toml` w formacie Etapu 7.1, wskazując wygenerowany JSONL,
   ground truth i tę samą konfigurację ROI.
6. Uruchom `evaluate`, następnie osobno `sweep` z przykładu powyżej.
7. Analizuj false-free, coverage, F1 obu klas i latencję zmian; nie wybieraj progów na
   podstawie samego accuracy.

Sugerowany układ:

```text
datasets/real-run-001/
├── dataset.toml
├── detections.jsonl
├── detections-metadata.json
└── ground_truth.jsonl
```

Każdy rekord JSONL ma `schema_version: 1`; `frame_index` oznacza kolejny **zapisany
sample** od 0, nie indeks klatki źródłowej. `timestamp_ms` pochodzi z GStreamer PTS
(czas prezentacji wideo), nie z zegara systemowego. Brak PTS lub cofnięcie PTS jest
błędem — nie ma fallbacku do wall clock. Ten sam film i sample interval dają te same
rekordy przy deterministycznym modelu. `detections-metadata.json` zapisuje nazwę
i SHA-256 modelu oraz wideo, kontrakt, rozmiar wejścia, cadence, output floor,
czas uruchomienia i liczbę rekordów. Pole `generated_at` oraz czas wykonania są
naturalnie różne między uruchomieniami; sam `detections.jsonl` jest artefaktem
do porównywania i ponownej ewaluacji.

Zapis jest strumieniowy do `detections.jsonl.tmp`. Dopiero poprawny EOF, flush,
zapis metadata i rename publikują finalny plik. Brak modelu/wideo/ORT, niezgodny
kontrakt modelu, błąd inferencji, nieprawidłowy output modelu lub błąd zapisu
kończą generowanie błędem bez udawania kompletnego datasetu. Polityka badawcza
jest tu **fail-fast**, inaczej niż runtime edge, który może pominąć błędną klatkę.
Generator nie łączy się z NATS ani PostgreSQL, nie używa SQLite sequence i nie
publikuje `SpotObservationV1`; nie uruchamia też `evaluate` automatycznie.

`detections.jsonl` opisuje obecność i położenie obiektów, ale nie zawiera surowych
klatek, cropów, twarzy ani tablic. Artefakty nie są automatycznie przesyłane do
chmury; duże dane i modele pozostają poza Git. Testy fake source/fake detector
oraz syntetyczny GStreamer pozwalają sprawdzić pipeline bez realnego modelu
i nagrania. Etap 7.2A przygotowuje real-data pipeline, lecz nie potwierdza jeszcze
jakości ani poprawności konkretnego modelu na rzeczywistym nagraniu.

Test syntetycznej ścieżki GStreamer → fake detector → JSONL (wymaga bibliotek i pluginów
GStreamer, domyślnie ignorowany):

```bash
cargo test -p perception-evaluator gstreamer_synthetic_source_generates_jsonl -- --ignored
```
