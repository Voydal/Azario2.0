# parking-platform

Minimalny backend systemu parkingowego w Rust. Workspace zawiera czystą bibliotekę domenową,
wersjonowane kontrakty zdarzeń, persistence w PostgreSQL/PostGIS, API HTTP, worker ingestion
oraz simulator kamery publikujący do NATS JetStream.

## Lokalne uruchomienie

Wymagane są Rust oraz Docker z Docker Compose. Lokalne środowisko używa przypiętych obrazów
`postgis/postgis:17-3.5` oraz `nats:2.15.0-alpine`.

```bash
docker compose up -d
docker compose ps
export DATABASE_URL=postgres://parking:parking@localhost:5432/parking
export NATS_URL=nats://127.0.0.1:4222
cargo run -p parking-api
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

## Testy i kontrola jakości

PostGIS i NATS z Compose muszą działać. Testy integracyjne wykonują prawdziwe zapytania do
obu usług.

```bash
export DATABASE_URL=postgres://parking:parking@localhost:5432/parking
export NATS_URL=nats://127.0.0.1:4222
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

## Założenie MVP

Jedno miejsce parkingowe ma jedno autorytatywne źródło obserwacji w danym czasie. System nie
implementuje sensor fusion ani rozbudowanego ownership kamer. `sequence` ustala kolejność
zdarzeń, a `observed_at` służy wyłącznie do oceny świeżości obserwacji.
