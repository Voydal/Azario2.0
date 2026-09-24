use std::{error::Error, fmt, sync::Arc};

use chrono::{DateTime, Utc};

use crate::BoundingBox;

#[derive(Clone, Debug)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Arc<[u8]>,
    pub captured_at: DateTime<Utc>,
}

impl Frame {
    #[must_use]
    pub fn rgb(width: u32, height: u32, pixels: Vec<u8>, captured_at: DateTime<Utc>) -> Self {
        Self {
            width,
            height,
            pixels: pixels.into(),
            captured_at,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Detection {
    pub class_id: u32,
    pub confidence: f32,
    pub bbox: BoundingBox,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DetectionError(String);

impl DetectionError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for DetectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for DetectionError {}

pub trait Detector {
    fn detect(&mut self, frame: &Frame) -> Result<Vec<Detection>, DetectionError>;
}
