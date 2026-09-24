use chrono::{DateTime, Duration, Utc};
use parking_domain::ObservedState;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmissionReason {
    StableChange,
    Refresh,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Emission {
    pub state: ObservedState,
    pub reason: EmissionReason,
}

#[derive(Clone, Debug)]
pub struct SpotStabilizer {
    required_samples: u32,
    refresh_interval: Duration,
    candidate: Option<ObservedState>,
    candidate_count: u32,
    stable: Option<ObservedState>,
    last_emitted_at: Option<DateTime<Utc>>,
}

impl SpotStabilizer {
    #[must_use]
    pub fn new(required_samples: u32, refresh_interval: Duration) -> Self {
        assert!(required_samples > 0, "required samples must be non-zero");
        assert!(
            refresh_interval > Duration::zero(),
            "refresh must be positive"
        );
        Self {
            required_samples,
            refresh_interval,
            candidate: None,
            candidate_count: 0,
            stable: None,
            last_emitted_at: None,
        }
    }

    pub fn observe(&mut self, state: ObservedState, now: DateTime<Utc>) -> Option<Emission> {
        if self.candidate == Some(state) {
            self.candidate_count = self.candidate_count.saturating_add(1);
        } else {
            self.candidate = Some(state);
            self.candidate_count = 1;
        }

        if self.candidate_count >= self.required_samples && self.stable != Some(state) {
            self.stable = Some(state);
            self.last_emitted_at = Some(now);
            return Some(Emission {
                state,
                reason: EmissionReason::StableChange,
            });
        }

        if self.stable == Some(state)
            && self
                .last_emitted_at
                .is_some_and(|last| now.signed_duration_since(last) >= self.refresh_interval)
        {
            self.last_emitted_at = Some(now);
            return Some(Emission {
                state,
                reason: EmissionReason::Refresh,
            });
        }
        None
    }

    #[must_use]
    pub const fn stable_state(&self) -> Option<ObservedState> {
        self.stable
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    fn at(second: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(second, 0).single().unwrap()
    }

    fn stabilizer() -> SpotStabilizer {
        SpotStabilizer::new(3, Duration::seconds(5))
    }

    #[test]
    fn single_frame_does_not_flip_stable_state() {
        let mut subject = stabilizer();
        assert_eq!(subject.observe(ObservedState::Free, at(0)), None);
        assert_eq!(subject.stable_state(), None);
    }

    #[test]
    fn three_consistent_samples_establish_state() {
        let mut subject = stabilizer();
        assert_eq!(subject.observe(ObservedState::Free, at(0)), None);
        assert_eq!(subject.observe(ObservedState::Free, at(1)), None);
        assert_eq!(
            subject.observe(ObservedState::Free, at(2)),
            Some(Emission {
                state: ObservedState::Free,
                reason: EmissionReason::StableChange,
            })
        );
    }

    #[test]
    fn short_fluctuation_does_not_change_state() {
        let mut subject = stabilizer();
        for second in 0..3 {
            subject.observe(ObservedState::Free, at(second));
        }
        assert_eq!(subject.observe(ObservedState::Occupied, at(3)), None);
        assert_eq!(subject.observe(ObservedState::Free, at(4)), None);
        assert_eq!(subject.observe(ObservedState::Free, at(5)), None);
        assert_eq!(subject.stable_state(), Some(ObservedState::Free));
    }

    #[test]
    fn stable_change_is_emitted() {
        let mut subject = stabilizer();
        for second in 0..3 {
            subject.observe(ObservedState::Free, at(second));
        }
        assert_eq!(subject.observe(ObservedState::Occupied, at(3)), None);
        assert_eq!(subject.observe(ObservedState::Occupied, at(4)), None);
        assert_eq!(
            subject.observe(ObservedState::Occupied, at(5)),
            Some(Emission {
                state: ObservedState::Occupied,
                reason: EmissionReason::StableChange,
            })
        );
    }

    #[test]
    fn stable_uncertain_is_emitted() {
        let mut subject = stabilizer();
        subject.observe(ObservedState::Uncertain, at(0));
        subject.observe(ObservedState::Uncertain, at(1));
        let emission = subject.observe(ObservedState::Uncertain, at(2)).unwrap();
        assert_eq!(emission.state, ObservedState::Uncertain);
    }

    #[test]
    fn unchanged_state_is_refreshed_before_backend_ttl() {
        let mut subject = SpotStabilizer::new(1, Duration::seconds(5));
        assert!(subject.observe(ObservedState::Free, at(0)).is_some());
        assert_eq!(subject.observe(ObservedState::Free, at(1)), None);
        assert_eq!(subject.observe(ObservedState::Free, at(4)), None);
        assert_eq!(
            subject.observe(ObservedState::Free, at(5)),
            Some(Emission {
                state: ObservedState::Free,
                reason: EmissionReason::Refresh,
            })
        );
    }
}
