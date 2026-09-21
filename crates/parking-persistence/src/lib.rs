use std::{error::Error, fmt};

use chrono::{DateTime, Duration, Utc};
use parking_domain::{AvailabilityState, ObservedState, ParkingSpotId, SpotObservation};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};

const SEARCH_RESULT_LIMIT: i64 = 100;

#[derive(Clone)]
pub struct ParkingRepository {
    pool: PgPool,
}

impl ParkingRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn connect(database_url: &str) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(database_url)
            .await?;
        Ok(Self::new(pool))
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn migrate(&self) -> Result<(), sqlx::migrate::MigrateError> {
        sqlx::migrate!("../../migrations").run(&self.pool).await
    }

    pub async fn create_parking_spot(
        &self,
        id: ParkingSpotId,
        latitude: f64,
        longitude: f64,
    ) -> Result<CreateParkingSpotResult, sqlx::Error> {
        let result = sqlx::query(
            r#"
            INSERT INTO parking_spots (id, location)
            VALUES ($1, ST_SetSRID(ST_MakePoint($3, $2), 4326)::geography)
            ON CONFLICT (id) DO NOTHING
            "#,
        )
        .bind(id.into_uuid())
        .bind(latitude)
        .bind(longitude)
        .execute(&self.pool)
        .await?;

        Ok(if result.rows_affected() == 1 {
            CreateParkingSpotResult::Created
        } else {
            CreateParkingSpotResult::AlreadyExists
        })
    }

    pub async fn store_observation(
        &self,
        observation: &SpotObservation,
    ) -> Result<StoreObservationResult, RepositoryError> {
        let sequence = i64::try_from(observation.sequence)
            .map_err(|_| RepositoryError::SequenceOutOfRange(observation.sequence))?;
        let mut transaction = self.pool.begin().await?;

        let inserted = sqlx::query(
            r#"
            INSERT INTO spot_observations (
                event_id,
                camera_id,
                spot_id,
                sequence,
                observed_at,
                received_at,
                observed_state,
                model_score,
                model_version
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(observation.event_id.into_uuid())
        .bind(observation.camera_id.into_uuid())
        .bind(observation.spot_id.into_uuid())
        .bind(sequence)
        .bind(observation.observed_at)
        .bind(Utc::now())
        .bind(observed_state_as_str(observation.state))
        .bind(observation.model_score)
        .bind(&observation.model_version)
        .execute(&mut *transaction)
        .await?;

        if inserted.rows_affected() == 0 {
            transaction.commit().await?;
            return Ok(StoreObservationResult::Duplicate);
        }

        sqlx::query(
            r#"
            INSERT INTO spot_current_state (
                spot_id,
                state,
                observed_at,
                last_sequence,
                updated_at
            )
            VALUES ($1, $2, $3, $4, NOW())
            ON CONFLICT (spot_id) DO UPDATE
            SET state = EXCLUDED.state,
                observed_at = EXCLUDED.observed_at,
                last_sequence = EXCLUDED.last_sequence,
                updated_at = NOW()
            WHERE EXCLUDED.last_sequence > spot_current_state.last_sequence
            "#,
        )
        .bind(observation.spot_id.into_uuid())
        .bind(availability_state_as_str(observation.state.into()))
        .bind(observation.observed_at)
        .bind(sequence)
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(StoreObservationResult::Stored)
    }

    pub async fn find_free_parking_spots(
        &self,
        latitude: f64,
        longitude: f64,
        radius_m: f64,
        now: DateTime<Utc>,
        ttl: Duration,
    ) -> Result<Vec<FreeParkingSpot>, sqlx::Error> {
        let freshness_cutoff = now - ttl;
        let rows = sqlx::query(
            r#"
            SELECT
                p.id,
                ST_Y(p.location::geometry) AS latitude,
                ST_X(p.location::geometry) AS longitude,
                current.observed_at,
                ST_Distance(
                    p.location,
                    ST_SetSRID(ST_MakePoint($2, $1), 4326)::geography
                ) AS distance_m
            FROM parking_spots AS p
            INNER JOIN spot_current_state AS current ON current.spot_id = p.id
            WHERE p.enabled = TRUE
              AND current.state = 'free'
              AND current.observed_at >= $4
              AND ST_DWithin(
                    p.location,
                    ST_SetSRID(ST_MakePoint($2, $1), 4326)::geography,
                    $3
              )
            ORDER BY distance_m ASC
            LIMIT $5
            "#,
        )
        .bind(latitude)
        .bind(longitude)
        .bind(radius_m)
        .bind(freshness_cutoff)
        .bind(SEARCH_RESULT_LIMIT)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(FreeParkingSpot {
                    id: ParkingSpotId::from_uuid(row.try_get("id")?),
                    latitude: row.try_get("latitude")?,
                    longitude: row.try_get("longitude")?,
                    observed_at: row.try_get("observed_at")?,
                    distance_m: row.try_get("distance_m")?,
                })
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CreateParkingSpotResult {
    Created,
    AlreadyExists,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreObservationResult {
    Stored,
    Duplicate,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FreeParkingSpot {
    pub id: ParkingSpotId,
    pub latitude: f64,
    pub longitude: f64,
    pub observed_at: DateTime<Utc>,
    pub distance_m: f64,
}

#[derive(Debug)]
pub enum RepositoryError {
    SequenceOutOfRange(u64),
    Database(sqlx::Error),
}

impl RepositoryError {
    #[must_use]
    pub fn is_foreign_key_violation(&self) -> bool {
        match self {
            Self::Database(sqlx::Error::Database(error)) => {
                error.code().as_deref() == Some("23503")
            }
            Self::SequenceOutOfRange(_) | Self::Database(_) => false,
        }
    }
}

impl fmt::Display for RepositoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SequenceOutOfRange(sequence) => {
                write!(
                    formatter,
                    "sequence {sequence} does not fit PostgreSQL BIGINT"
                )
            }
            Self::Database(error) => error.fmt(formatter),
        }
    }
}

impl Error for RepositoryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::SequenceOutOfRange(_) => None,
            Self::Database(error) => Some(error),
        }
    }
}

impl From<sqlx::Error> for RepositoryError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

const fn observed_state_as_str(state: ObservedState) -> &'static str {
    match state {
        ObservedState::Free => "free",
        ObservedState::Occupied => "occupied",
        ObservedState::Uncertain => "uncertain",
    }
}

const fn availability_state_as_str(state: AvailabilityState) -> &'static str {
    match state {
        AvailabilityState::Free => "free",
        AvailabilityState::Occupied => "occupied",
        AvailabilityState::Unknown => "unknown",
    }
}
