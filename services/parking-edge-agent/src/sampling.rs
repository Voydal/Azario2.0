use std::time::Duration;

use chrono::{DateTime, Utc};

/// Identical cadence gate for production edge and offline generation.
pub struct FrameSampler {
    sample_interval: Duration,
    last_sampled_at: Option<DateTime<Utc>>,
}

impl FrameSampler {
    #[must_use]
    pub const fn new(sample_interval: Duration) -> Self {
        Self {
            sample_interval,
            last_sampled_at: None,
        }
    }

    #[must_use]
    pub fn should_sample(&mut self, timestamp: DateTime<Utc>) -> bool {
        if self.last_sampled_at.is_some_and(|last| {
            timestamp
                .signed_duration_since(last)
                .to_std()
                .is_ok_and(|elapsed| elapsed < self.sample_interval)
        }) {
            return false;
        }
        self.last_sampled_at = Some(timestamp);
        true
    }
}
