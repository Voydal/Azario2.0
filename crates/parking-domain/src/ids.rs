use std::fmt;

use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ParkingSpotId(Uuid);

impl ParkingSpotId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn into_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for ParkingSpotId {
    fn default() -> Self {
        Self::new()
    }
}

impl From<Uuid> for ParkingSpotId {
    fn from(value: Uuid) -> Self {
        Self::from_uuid(value)
    }
}

impl From<ParkingSpotId> for Uuid {
    fn from(value: ParkingSpotId) -> Self {
        value.into_uuid()
    }
}

impl fmt::Display for ParkingSpotId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CameraId(Uuid);

impl CameraId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn into_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for CameraId {
    fn default() -> Self {
        Self::new()
    }
}

impl From<Uuid> for CameraId {
    fn from(value: Uuid) -> Self {
        Self::from_uuid(value)
    }
}

impl From<CameraId> for Uuid {
    fn from(value: CameraId) -> Self {
        value.into_uuid()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EventId(Uuid);

impl EventId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn into_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for EventId {
    fn default() -> Self {
        Self::new()
    }
}

impl From<Uuid> for EventId {
    fn from(value: Uuid) -> Self {
        Self::from_uuid(value)
    }
}

impl From<EventId> for Uuid {
    fn from(value: EventId) -> Self {
        value.into_uuid()
    }
}
