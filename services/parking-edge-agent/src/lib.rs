pub mod config;
pub mod frame_source;
pub mod onnx;
pub mod pipeline;
pub mod publisher;
pub mod sequence;

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;
