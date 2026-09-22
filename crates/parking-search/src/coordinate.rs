use std::{error::Error, fmt};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coordinate {
    latitude: f64,
    longitude: f64,
}

impl Coordinate {
    pub fn new(latitude: f64, longitude: f64) -> Result<Self, CoordinateError> {
        if !latitude.is_finite()
            || !longitude.is_finite()
            || !(-90.0..=90.0).contains(&latitude)
            || !(-180.0..=180.0).contains(&longitude)
        {
            return Err(CoordinateError);
        }

        Ok(Self {
            latitude,
            longitude,
        })
    }

    #[must_use]
    pub const fn latitude(self) -> f64 {
        self.latitude
    }

    #[must_use]
    pub const fn longitude(self) -> f64 {
        self.longitude
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoordinateError;

impl fmt::Display for CoordinateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("latitude must be in [-90, 90] and longitude in [-180, 180]")
    }
}

impl Error for CoordinateError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_boundary_coordinates() {
        assert!(Coordinate::new(-90.0, 180.0).is_ok());
        assert!(Coordinate::new(90.0, -180.0).is_ok());
    }

    #[test]
    fn rejects_invalid_coordinates() {
        assert!(Coordinate::new(90.1, 0.0).is_err());
        assert!(Coordinate::new(0.0, f64::NAN).is_err());
    }
}
