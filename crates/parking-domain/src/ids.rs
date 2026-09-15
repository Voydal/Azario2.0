use std::fmt;

use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SpotId(Uuid);

impl SpotId {
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

impl Default for SpotId {
    fn default() -> Self {
        Self::new()
    }
}

impl From<Uuid> for SpotId {
    fn from(value: Uuid) -> Self {
        Self::from_uuid(value)
    }
}

impl From<SpotId> for Uuid {
    fn from(value: SpotId) -> Self {
        value.into_uuid()
    }
}

impl fmt::Display for SpotId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}
