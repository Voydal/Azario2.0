mod availability;
mod ids;
mod observation;

pub use availability::{
    ApplyObservationError, ApplyObservationResult, AvailabilityState, ObservedState,
    SpotCurrentState, apply_observation,
};
pub use ids::{CameraId, EventId, ParkingSpotId};
pub use observation::SpotObservation;
