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
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let log_format = init_logging()?;
    info!(
        service = "parking-edge-agent",
        version = env!("CARGO_PKG_VERSION"),
        log_format,
        event = "startup",
        "edge starting"
    );
    let config_path = config_path()?;
    let config = EdgeConfig::load(&config_path)?;
    config.validate_artifacts()?;
    info!(camera_id = %config.camera_id, config = %config_path.display(), "camera config loaded");

    let nats_url = env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let client = tokio::time::timeout(Duration::from_secs(5), async_nats::connect(&nats_url))
        .await
        .map_err(|_| SimpleError("NATS startup timeout".into()))??;
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
        shutdown_signal().await;
        signal.store(true, Ordering::Relaxed);
        tokio::spawn(async {
            tokio::time::sleep(SHUTDOWN_TIMEOUT).await;
            error!(
                service = "parking-edge-agent",
                event = "shutdown_timeout",
                "edge shutdown timed out"
            );
            std::process::exit(1);
        });
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
            if shutdown.load(Ordering::Relaxed) {
                break;
            }
            info!(spot_id = %pending.spot_id, state = ?pending.state, reason = ?pending.reason, "stable state changed or observation refresh");
            if let Err(publish_error) = tokio::time::timeout(
                Duration::from_secs(10),
                publish_with_retry(
                    &publisher,
                    &mut sequences,
                    config.camera_id(),
                    pending,
                    &config.model_version,
                    PUBLISH_ATTEMPTS,
                    INITIAL_RETRY_DELAY,
                ),
            )
            .await
            .map_err(|_| SimpleError("publish timeout".into()))
            .and_then(|result| result.map_err(|error| SimpleError(error.to_string())))
            {
                error!(error = %publish_error, spot_id = %pending.spot_id, "observation publish failed after bounded retry");
            }
        }
    }
    source.close()?;
    info!(
        service = "parking-edge-agent",
        event = "shutdown_complete",
        "edge stopped"
    );
    Ok(())
}

fn init_logging() -> Result<&'static str, BoxError> {
    let format = env::var("LOG_FORMAT").unwrap_or_else(|_| "pretty".into());
    let filter = match env::var("RUST_LOG") {
        Ok(value) => EnvFilter::try_new(value)?,
        Err(env::VarError::NotPresent) => EnvFilter::new("info"),
        Err(error) => return Err(error.into()),
    };
    match format.as_str() {
        "json" => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .json()
            .init(),
        "pretty" => tracing_subscriber::fmt().with_env_filter(filter).init(),
        _ => return Err(SimpleError("LOG_FORMAT must be json or pretty".into()).into()),
    }
    Ok(if format == "json" { "json" } else { "pretty" })
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
        match terminate {
            Ok(mut terminate) => tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = terminate.recv() => {},
            },
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    info!(
        service = "parking-edge-agent",
        event = "shutdown_requested",
        "stopping edge"
    );
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
