use std::{env, time::Duration};

use futures_util::StreamExt;
use parking_ingestion_worker::{BoxError, handle_message, provision_jetstream};
use parking_persistence::{ParkingRepository, PoolConfig};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const MESSAGE_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let log_format = init_logging()?;
    let database_url = env::var("DATABASE_URL")
        .map_err(|_| "DATABASE_URL must be set before starting the worker")?;
    let nats_url =
        env::var("NATS_URL").map_err(|_| "NATS_URL must be set before starting the worker")?;
    let max_connections = environment_u32("DATABASE_MAX_CONNECTIONS", 10)?;
    let acquire_timeout_ms = environment_u64("DATABASE_ACQUIRE_TIMEOUT_MS", 2000)?;
    if max_connections == 0 || acquire_timeout_ms == 0 {
        return Err("database pool size and acquire timeout must be positive".into());
    }
    info!(
        service = "parking-ingestion-worker",
        version = env!("CARGO_PKG_VERSION"),
        log_format,
        event = "startup",
        "worker starting"
    );
    let repository = tokio::time::timeout(
        STARTUP_TIMEOUT,
        ParkingRepository::connect_with_config(
            &database_url,
            PoolConfig {
                max_connections,
                acquire_timeout: Duration::from_millis(acquire_timeout_ms),
            },
        ),
    )
    .await
    .map_err(|_| "database startup timeout")??;
    info!(
        service = "parking-ingestion-worker",
        event = "database_connected",
        "connected to PostgreSQL"
    );

    let client = tokio::time::timeout(STARTUP_TIMEOUT, async_nats::connect(&nats_url))
        .await
        .map_err(|_| "NATS startup timeout")??;
    info!(
        service = "parking-ingestion-worker",
        event = "nats_connected",
        "connected to NATS"
    );
    let context = async_nats::jetstream::new(client);
    let consumer = tokio::time::timeout(STARTUP_TIMEOUT, provision_jetstream(&context))
        .await
        .map_err(|_| "JetStream provisioning timeout")??;
    let mut messages = tokio::time::timeout(STARTUP_TIMEOUT, consumer.messages())
        .await
        .map_err(|_| "JetStream consumer startup timeout")??;
    info!(
        service = "parking-ingestion-worker",
        event = "consumer_ready",
        "worker started"
    );
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            () = &mut shutdown => {
                info!(service = "parking-ingestion-worker", event = "shutdown_requested",
                    "stopping message intake");
                break;
            }
            maybe_message = messages.next() => {
                match maybe_message {
                    Some(Ok(message)) => {
                        match tokio::time::timeout(MESSAGE_TIMEOUT,
                            handle_message(&repository, &message)).await {
                            Ok(Ok(_)) => {},
                            Ok(Err(_)) => error!(service = "parking-ingestion-worker",
                                event = "message_ack_failure", "message acknowledgement failed"),
                            Err(_) => warn!(service = "parking-ingestion-worker",
                                event = "message_timeout", "message processing timed out without ACK"),
                        }
                    }
                    Some(Err(_)) => warn!(service = "parking-ingestion-worker",
                        event = "consumer_error", "JetStream consumer error"),
                    None => return Err("JetStream consumer ended unexpectedly".into()),
                }
            }
        }
    }

    info!(
        service = "parking-ingestion-worker",
        event = "shutdown_complete",
        "worker stopped"
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
        _ => return Err("LOG_FORMAT must be json or pretty".into()),
    }
    Ok(if format == "json" { "json" } else { "pretty" })
}

fn environment_u32(name: &str, default: u32) -> Result<u32, BoxError> {
    env::var(name).map_or(Ok(default), |value| {
        value
            .parse()
            .map_err(|_| format!("{name} must be an unsigned integer").into())
    })
}

fn environment_u64(name: &str, default: u64) -> Result<u64, BoxError> {
    env::var(name).map_or(Ok(default), |value| {
        value
            .parse()
            .map_err(|_| format!("{name} must be an unsigned integer").into())
    })
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
}
