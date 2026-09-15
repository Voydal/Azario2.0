mod availability;
mod ids;
mod observation;

pub use availability::{ApplyObservationError, SpotAvailability, SpotCurrentState};
pub use ids::SpotId;
pub use observation::SpotObservation;
