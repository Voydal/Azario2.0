use std::{
    env,
    error::Error,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use async_nats::jetstream;
use parking_edge_agent::{
    BoxError,
    config::EdgeConfig,
    frame_source::{FrameSource, GstreamerFileSource},
    onnx::OnnxDetector,
    pipeline::PerceptionPipeline,
    publisher::{JetStreamPublisher, publish_with_retry},
    sequence::SequenceStore,
};
use parking_perception::{Detector, OccupancyEngine};
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;

const PUBLISH_ATTEMPTS: u32 = 5;
const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(100);

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let config_path = config_path()?;
    let config = EdgeConfig::load(&config_path)?;
    config.validate_artifacts()?;
    info!(camera_id = %config.camera_id, config = %config_path.display(), "camera config loaded");

    let nats_url = env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let client = async_nats::connect(&nats_url).await?;
    let publisher = JetStreamPublisher::new(jetstream::new(client));
    let mut sequences = SequenceStore::open(&config.state_database)?;
    let mut source = GstreamerFileSource::open(&config.video_file)?;
    info!(video = %config.video_file.display(), format = "RGB", "video opened");
    let mut detector =
        OnnxDetector::load(&config.model_path, config.input_width, config.input_height)?;
    info!(model = %config.model_path.display(), provider = "CPU", "model loaded");

    let rois = config
        .spots
        .iter()
        .map(|spot| spot.roi())
        .collect::<Result<Vec<_>, _>>()?;
    let mut pipeline = PerceptionPipeline::new(
        OccupancyEngine::new(config.occupancy_config()),
        rois,
        || config.stabilizer(),
        config.sample_interval(),
    );

    let shutdown = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&shutdown);
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal.store(true, Ordering::Relaxed);
        }
    });
    info!(camera_id = %config.camera_id, "edge started");

    while !shutdown.load(Ordering::Relaxed) {
        let next_frame = source.next_frame();
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        let Some(frame) = next_frame? else {
            info!("video EOF");
            break;
        };
        if !pipeline.should_sample(frame.captured_at) {
            continue;
        }
        debug!(width = frame.width, height = frame.height, "frame sampled");
        let detections = match detector.detect(&frame) {
            Ok(detections) => detections,
            Err(error) => {
                warn!(error = %error, "inference error");
                continue;
            }
        };
        for pending in pipeline.process_detections(&detections, frame.captured_at) {
            info!(spot_id = %pending.spot_id, state = ?pending.state, reason = ?pending.reason, "stable state changed or observation refresh");
            if let Err(publish_error) = publish_with_retry(
                &publisher,
                &mut sequences,
                config.camera_id(),
                pending,
                &config.model_version,
                PUBLISH_ATTEMPTS,
                INITIAL_RETRY_DELAY,
            )
            .await
            {
                error!(error = %publish_error, spot_id = %pending.spot_id, "observation publish failed after bounded retry");
            }
        }
    }
    source.close()?;
    info!("shutdown");
    Ok(())
}

fn config_path() -> Result<PathBuf, BoxError> {
    let mut arguments = env::args_os().skip(1);
    match (
        arguments.next().as_deref(),
        arguments.next(),
        arguments.next(),
    ) {
        (Some(flag), Some(path), None) if flag == "--config" => Ok(path.into()),
        _ => Err(SimpleError("usage: parking-edge-agent --config <path>".into()).into()),
    }
}

#[derive(Debug)]
struct SimpleError(String);

impl std::fmt::Display for SimpleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for SimpleError {}
