mod detection;
mod engine;
mod geometry;
mod roi_config;
mod stabilizer;

pub use detection::{Detection, DetectionError, Detector, Frame};
pub use engine::{OccupancyConfig, OccupancyConfigError, OccupancyEngine, ScoredState};
pub use geometry::{BoundingBox, GeometryError, NormalizedPoint, ParkingSpotRoi};
pub use roi_config::{PointConfig, SpotConfig};
pub use stabilizer::{Emission, EmissionReason, SpotStabilizer};
