use std::{error::Error, fmt};

use chrono::{DateTime, Utc};

use crate::{SpotId, SpotObservation};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpotAvailability {
    Available,
    Occupied,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpotCurrentState {
    pub spot_id: SpotId,
    pub availability: SpotAvailability,
    pub observed_at: DateTime<Utc>,
}

impl SpotCurrentState {
    #[must_use]
    pub const fn from_observation(observation: SpotObservation) -> Self {
        Self {
            spot_id: observation.spot_id,
            availability: observation.availability,
            observed_at: observation.observed_at,
        }
    }

    /// Applies an observation when it is newer than the current state.
    ///
    /// Returns `Ok(true)` when the state changed and `Ok(false)` for an
    /// observation that is stale or has the same timestamp.
    pub fn apply_observation(
        &mut self,
        observation: SpotObservation,
    ) -> Result<bool, ApplyObservationError> {
        if self.spot_id != observation.spot_id {
            return Err(ApplyObservationError::SpotMismatch {
                expected: self.spot_id,
                actual: observation.spot_id,
            });
        }

        if observation.observed_at <= self.observed_at {
            return Ok(false);
        }

        self.availability = observation.availability;
        self.observed_at = observation.observed_at;
        Ok(true)
    }
}

impl From<SpotObservation> for SpotCurrentState {
    fn from(observation: SpotObservation) -> Self {
        Self::from_observation(observation)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplyObservationError {
    SpotMismatch { expected: SpotId, actual: SpotId },
}

impl fmt::Display for ApplyObservationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SpotMismatch { expected, actual } => {
                write!(
                    formatter,
                    "observation for spot {actual} cannot update spot {expected}"
                )
            }
        }
    }
}

impl Error for ApplyObservationError {}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    fn at(second: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, second)
            .single()
            .expect("test timestamp should be valid")
    }

    #[test]
    fn state_is_created_from_an_observation() {
        let spot_id = SpotId::new();
        let observation = SpotObservation::new(spot_id, SpotAvailability::Available, at(5));

        let state = SpotCurrentState::from_observation(observation);

        assert_eq!(state.spot_id, spot_id);
        assert_eq!(state.availability, SpotAvailability::Available);
        assert_eq!(state.observed_at, at(5));
    }

    #[test]
    fn newer_observation_updates_the_state() {
        let spot_id = SpotId::new();
        let mut state = SpotCurrentState::from_observation(SpotObservation::new(
            spot_id,
            SpotAvailability::Available,
            at(5),
        ));
        let newer = SpotObservation::new(spot_id, SpotAvailability::Occupied, at(6));

        assert_eq!(state.apply_observation(newer), Ok(true));
        assert_eq!(state.availability, SpotAvailability::Occupied);
        assert_eq!(state.observed_at, at(6));
    }

    #[test]
    fn stale_or_duplicate_observations_do_not_overwrite_the_state() {
        let spot_id = SpotId::new();
        let mut state = SpotCurrentState::from_observation(SpotObservation::new(
            spot_id,
            SpotAvailability::Occupied,
            at(6),
        ));

        for timestamp in [at(5), at(6)] {
            let stale = SpotObservation::new(spot_id, SpotAvailability::Available, timestamp);
            assert_eq!(state.apply_observation(stale), Ok(false));
        }

        assert_eq!(state.availability, SpotAvailability::Occupied);
        assert_eq!(state.observed_at, at(6));
    }

    #[test]
    fn observation_for_another_spot_is_rejected() {
        let spot_id = SpotId::new();
        let other_spot_id = SpotId::new();
        let mut state = SpotCurrentState::from_observation(SpotObservation::new(
            spot_id,
            SpotAvailability::Available,
            at(5),
        ));
        let observation = SpotObservation::new(other_spot_id, SpotAvailability::Occupied, at(6));

        assert_eq!(
            state.apply_observation(observation),
            Err(ApplyObservationError::SpotMismatch {
                expected: spot_id,
                actual: other_spot_id,
            })
        );
    }
}
