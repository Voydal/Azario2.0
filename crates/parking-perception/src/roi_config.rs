use parking_domain::ParkingSpotId;
use serde::Deserialize;
use uuid::Uuid;

use crate::{GeometryError, NormalizedPoint, ParkingSpotRoi};

/// Shared TOML representation of an edge parking-spot ROI.
#[derive(Clone, Debug, Deserialize)]
pub struct SpotConfig {
    pub spot_id: Uuid,
    pub polygon: Vec<PointConfig>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct PointConfig {
    pub x: f64,
    pub y: f64,
}

impl SpotConfig {
    pub fn roi(&self) -> Result<ParkingSpotRoi, GeometryError> {
        let points = self
            .polygon
            .iter()
            .map(|point| NormalizedPoint::new(point.x, point.y))
            .collect::<Result<Vec<_>, _>>()?;
        ParkingSpotRoi::new(ParkingSpotId::from_uuid(self.spot_id), points)
    }
}
