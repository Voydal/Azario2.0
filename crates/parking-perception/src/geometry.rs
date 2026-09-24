use std::{error::Error, fmt};

use geo::{Area, BooleanOps, Coord, LineString, Polygon, polygon};
use parking_domain::ParkingSpotId;

const MIN_AREA: f64 = f64::EPSILON;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoundingBox {
    x_min: f64,
    y_min: f64,
    x_max: f64,
    y_max: f64,
}

impl BoundingBox {
    pub fn new(x_min: f64, y_min: f64, x_max: f64, y_max: f64) -> Result<Self, GeometryError> {
        if ![x_min, y_min, x_max, y_max]
            .into_iter()
            .all(|value| value.is_finite() && (0.0..=1.0).contains(&value))
        {
            return Err(GeometryError::CoordinateOutOfRange);
        }
        if x_min >= x_max || y_min >= y_max {
            return Err(GeometryError::InvalidBoundingBox);
        }
        Ok(Self {
            x_min,
            y_min,
            x_max,
            y_max,
        })
    }

    #[must_use]
    pub const fn coordinates(self) -> [f64; 4] {
        [self.x_min, self.y_min, self.x_max, self.y_max]
    }

    fn as_polygon(self) -> Polygon<f64> {
        polygon![
            (x: self.x_min, y: self.y_min),
            (x: self.x_max, y: self.y_min),
            (x: self.x_max, y: self.y_max),
            (x: self.x_min, y: self.y_max),
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NormalizedPoint {
    pub x: f64,
    pub y: f64,
}

impl NormalizedPoint {
    pub fn new(x: f64, y: f64) -> Result<Self, GeometryError> {
        if !x.is_finite()
            || !y.is_finite()
            || !(0.0..=1.0).contains(&x)
            || !(0.0..=1.0).contains(&y)
        {
            return Err(GeometryError::CoordinateOutOfRange);
        }
        Ok(Self { x, y })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParkingSpotRoi {
    pub spot_id: ParkingSpotId,
    polygon: Polygon<f64>,
    area: f64,
}

impl ParkingSpotRoi {
    pub fn new(
        spot_id: ParkingSpotId,
        points: Vec<NormalizedPoint>,
    ) -> Result<Self, GeometryError> {
        if points.len() < 3 {
            return Err(GeometryError::TooFewPolygonPoints);
        }
        let coordinates = points
            .into_iter()
            .map(|point| Coord {
                x: point.x,
                y: point.y,
            })
            .collect::<Vec<_>>();
        let polygon = Polygon::new(LineString::new(coordinates), vec![]);
        let area = polygon.unsigned_area();
        if !area.is_finite() || area <= MIN_AREA {
            return Err(GeometryError::ZeroAreaPolygon);
        }
        Ok(Self {
            spot_id,
            polygon,
            area,
        })
    }

    #[must_use]
    pub fn overlap_ratio(&self, bbox: BoundingBox) -> f64 {
        let intersection_area = self
            .polygon
            .intersection(&bbox.as_polygon())
            .unsigned_area();
        (intersection_area / self.area).clamp(0.0, 1.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GeometryError {
    CoordinateOutOfRange,
    InvalidBoundingBox,
    TooFewPolygonPoints,
    ZeroAreaPolygon,
}

impl fmt::Display for GeometryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::CoordinateOutOfRange => "coordinates must be finite and within 0..=1",
            Self::InvalidBoundingBox => "bounding box minima must be below maxima",
            Self::TooFewPolygonPoints => "parking spot polygon needs at least three points",
            Self::ZeroAreaPolygon => "parking spot polygon must have non-zero area",
        };
        formatter.write_str(message)
    }
}

impl Error for GeometryError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_bbox_validation() {
        assert!(BoundingBox::new(0.1, 0.2, 0.8, 0.9).is_ok());
        assert_eq!(
            BoundingBox::new(f64::NAN, 0.0, 1.0, 1.0),
            Err(GeometryError::CoordinateOutOfRange)
        );
        assert_eq!(
            BoundingBox::new(0.5, 0.0, 0.5, 1.0),
            Err(GeometryError::InvalidBoundingBox)
        );
        assert_eq!(
            BoundingBox::new(0.0, 0.0, 1.1, 1.0),
            Err(GeometryError::CoordinateOutOfRange)
        );
    }

    #[test]
    fn invalid_roi_is_rejected() {
        let points = vec![
            NormalizedPoint::new(0.1, 0.1).unwrap(),
            NormalizedPoint::new(0.2, 0.2).unwrap(),
            NormalizedPoint::new(0.3, 0.3).unwrap(),
        ];
        assert_eq!(
            ParkingSpotRoi::new(ParkingSpotId::new(), points),
            Err(GeometryError::ZeroAreaPolygon)
        );
        assert!(NormalizedPoint::new(f64::INFINITY, 0.5).is_err());
    }
}
