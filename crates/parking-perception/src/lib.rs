mod detection;
mod engine;
mod geometry;
mod stabilizer;

pub use detection::{Detection, DetectionError, Detector, Frame};
pub use engine::{OccupancyConfig, OccupancyConfigError, OccupancyEngine, ScoredState};
pub use geometry::{BoundingBox, GeometryError, NormalizedPoint, ParkingSpotRoi};
pub use stabilizer::{Emission, EmissionReason, SpotStabilizer};
