use chrono::{DateTime, Utc};

use crate::{SpotAvailability, SpotId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpotObservation {
    pub spot_id: SpotId,
    pub availability: SpotAvailability,
    pub observed_at: DateTime<Utc>,
}

impl SpotObservation {
    #[must_use]
    pub const fn new(
        spot_id: SpotId,
        availability: SpotAvailability,
        observed_at: DateTime<Utc>,
    ) -> Self {
        Self {
            spot_id,
            availability,
            observed_at,
        }
    }
}
