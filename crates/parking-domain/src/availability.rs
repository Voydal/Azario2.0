use std::{error::Error, fmt};

use chrono::{DateTime, Duration, Utc};

use crate::{ParkingSpotId, SpotObservation};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservedState {
    Free,
    Occupied,
    Uncertain,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AvailabilityState {
    Free,
    Occupied,
    Unknown,
}

impl From<ObservedState> for AvailabilityState {
    fn from(state: ObservedState) -> Self {
        match state {
            ObservedState::Free => Self::Free,
            ObservedState::Occupied => Self::Occupied,
            ObservedState::Uncertain => Self::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpotCurrentState {
    pub spot_id: ParkingSpotId,
    pub state: AvailabilityState,
    pub observed_at: DateTime<Utc>,
    pub last_sequence: u64,
}

impl SpotCurrentState {
    #[must_use]
    pub fn effective_state(&self, now: DateTime<Utc>, ttl: Duration) -> AvailabilityState {
        if now.signed_duration_since(self.observed_at) <= ttl {
            self.state
        } else {
            AvailabilityState::Unknown
        }
    }

    fn from_observation(observation: SpotObservation) -> Self {
        Self {
            spot_id: observation.spot_id,
            state: observation.state.into(),
            observed_at: observation.observed_at,
            last_sequence: observation.sequence,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplyObservationResult {
    Applied(SpotCurrentState),
    Ignored(SpotCurrentState),
}

pub fn apply_observation(
    current: Option<SpotCurrentState>,
    observation: SpotObservation,
) -> Result<ApplyObservationResult, ApplyObservationError> {
    let Some(current) = current else {
        return Ok(ApplyObservationResult::Applied(
            SpotCurrentState::from_observation(observation),
        ));
    };

    if current.spot_id != observation.spot_id {
        return Err(ApplyObservationError::SpotMismatch {
            expected: current.spot_id,
            actual: observation.spot_id,
        });
    }

    if observation.sequence <= current.last_sequence {
        return Ok(ApplyObservationResult::Ignored(current));
    }

    Ok(ApplyObservationResult::Applied(
        SpotCurrentState::from_observation(observation),
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplyObservationError {
    SpotMismatch {
        expected: ParkingSpotId,
        actual: ParkingSpotId,
    },
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
    use crate::{CameraId, EventId};

    const TTL: Duration = Duration::seconds(15);

    fn at(hour: u32, minute: u32, second: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 2, hour, minute, second)
            .single()
            .expect("test timestamp should be valid")
    }

    fn observation(
        spot_id: ParkingSpotId,
        sequence: u64,
        observed_at: DateTime<Utc>,
        state: ObservedState,
    ) -> SpotObservation {
        SpotObservation::new(
            EventId::new(),
            CameraId::new(),
            spot_id,
            sequence,
            observed_at,
            state,
        )
    }

    fn current_state(observation: SpotObservation) -> SpotCurrentState {
        match apply_observation(None, observation).expect("first observation should be valid") {
            ApplyObservationResult::Applied(state) => state,
            ApplyObservationResult::Ignored(_) => {
                panic!("first observation should always be applied")
            }
        }
    }

    #[test]
    fn fresh_free_observation_is_free() {
        let observed_at = at(10, 0, 0);
        let state = current_state(observation(
            ParkingSpotId::new(),
            1,
            observed_at,
            ObservedState::Free,
        ));

        assert_eq!(
            state.effective_state(observed_at + Duration::seconds(10), TTL),
            AvailabilityState::Free
        );
    }

    #[test]
    fn stale_free_observation_becomes_unknown() {
        let observed_at = at(10, 0, 0);
        let state = current_state(observation(
            ParkingSpotId::new(),
            1,
            observed_at,
            ObservedState::Free,
        ));

        assert_eq!(
            state.effective_state(observed_at + Duration::seconds(16), TTL),
            AvailabilityState::Unknown
        );
    }

    #[test]
    fn fresh_occupied_observation_is_occupied() {
        let observed_at = at(10, 0, 0);
        let state = current_state(observation(
            ParkingSpotId::new(),
            1,
            observed_at,
            ObservedState::Occupied,
        ));

        assert_eq!(
            state.effective_state(observed_at + Duration::seconds(10), TTL),
            AvailabilityState::Occupied
        );
    }

    #[test]
    fn stale_occupied_observation_becomes_unknown() {
        let observed_at = at(10, 0, 0);
        let state = current_state(observation(
            ParkingSpotId::new(),
            1,
            observed_at,
            ObservedState::Occupied,
        ));

        assert_eq!(
            state.effective_state(observed_at + Duration::seconds(16), TTL),
            AvailabilityState::Unknown
        );
    }

    #[test]
    fn uncertain_observation_results_in_unknown() {
        let observed_at = at(10, 0, 0);
        let state = current_state(observation(
            ParkingSpotId::new(),
            1,
            observed_at,
            ObservedState::Uncertain,
        ));

        assert_eq!(
            state.effective_state(observed_at + Duration::seconds(1), TTL),
            AvailabilityState::Unknown
        );
    }

    #[test]
    fn older_sequence_is_ignored() {
        let spot_id = ParkingSpotId::new();
        let current = current_state(observation(spot_id, 20, at(10, 0, 0), ObservedState::Free));
        let incoming = observation(spot_id, 19, at(10, 0, 1), ObservedState::Occupied);

        assert_eq!(
            apply_observation(Some(current), incoming),
            Ok(ApplyObservationResult::Ignored(current))
        );
    }

    #[test]
    fn duplicate_sequence_is_ignored() {
        let spot_id = ParkingSpotId::new();
        let current = current_state(observation(spot_id, 20, at(10, 0, 0), ObservedState::Free));
        let incoming = observation(spot_id, 20, at(10, 0, 1), ObservedState::Occupied);

        assert_eq!(
            apply_observation(Some(current), incoming),
            Ok(ApplyObservationResult::Ignored(current))
        );
    }

    #[test]
    fn newer_sequence_changes_state() {
        let spot_id = ParkingSpotId::new();
        let current = current_state(observation(spot_id, 20, at(10, 0, 0), ObservedState::Free));
        let incoming = observation(spot_id, 21, at(10, 0, 1), ObservedState::Occupied);

        let result = apply_observation(Some(current), incoming);

        assert!(matches!(
            result,
            Ok(ApplyObservationResult::Applied(SpotCurrentState {
                state: AvailabilityState::Occupied,
                last_sequence: 21,
                ..
            }))
        ));
    }

    #[test]
    fn newer_sequence_is_applied_even_when_observed_at_is_earlier() {
        let spot_id = ParkingSpotId::new();
        let current = current_state(observation(spot_id, 20, at(10, 0, 5), ObservedState::Free));
        let earlier = at(9, 59, 59);
        let incoming = observation(spot_id, 21, earlier, ObservedState::Occupied);

        let result = apply_observation(Some(current), incoming);

        assert!(matches!(
            result,
            Ok(ApplyObservationResult::Applied(SpotCurrentState {
                state: AvailabilityState::Occupied,
                observed_at,
                last_sequence: 21,
                ..
            })) if observed_at == earlier
        ));
    }

    #[test]
    fn first_observation_creates_current_state() {
        let spot_id = ParkingSpotId::new();
        let observed_at = at(10, 0, 0);
        let first = observation(spot_id, 7, observed_at, ObservedState::Free);

        let result = apply_observation(None, first);

        assert_eq!(
            result,
            Ok(ApplyObservationResult::Applied(SpotCurrentState {
                spot_id,
                state: AvailabilityState::Free,
                observed_at,
                last_sequence: 7,
            }))
        );
    }

    #[test]
    fn observation_exactly_at_ttl_is_still_fresh() {
        let observed_at = at(10, 0, 0);
        let state = current_state(observation(
            ParkingSpotId::new(),
            1,
            observed_at,
            ObservedState::Occupied,
        ));

        assert_eq!(
            state.effective_state(observed_at + TTL, TTL),
            AvailabilityState::Occupied
        );
    }

    #[test]
    fn observation_for_different_spot_returns_spot_mismatch() {
        let spot_id = ParkingSpotId::new();
        let other_spot_id = ParkingSpotId::new();
        let current = current_state(observation(spot_id, 20, at(10, 0, 0), ObservedState::Free));
        let incoming = observation(other_spot_id, 21, at(10, 0, 1), ObservedState::Occupied);

        assert_eq!(
            apply_observation(Some(current), incoming),
            Err(ApplyObservationError::SpotMismatch {
                expected: spot_id,
                actual: other_spot_id,
            })
        );
    }
}
