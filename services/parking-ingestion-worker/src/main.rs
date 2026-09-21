use std::env;

use futures_util::StreamExt;
use parking_ingestion_worker::{BoxError, handle_message, provision_jetstream};
use parking_persistence::ParkingRepository;

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    println!("parking ingestion worker starting");
    let database_url = env::var("DATABASE_URL")
        .map_err(|_| "DATABASE_URL must be set before starting the worker")?;
    let nats_url =
        env::var("NATS_URL").map_err(|_| "NATS_URL must be set before starting the worker")?;

    let repository = ParkingRepository::connect(&database_url).await?;
    repository.migrate().await?;
    println!("connected to PostgreSQL");

    let client = async_nats::connect(&nats_url).await?;
    println!("connected to NATS");
    let context = async_nats::jetstream::new(client);
    let consumer = provision_jetstream(&context).await?;
    println!("JetStream stream and durable consumer ready; worker started");
    let mut messages = consumer.messages().await?;

    loop {
        tokio::select! {
            signal = tokio::signal::ctrl_c() => {
                signal?;
                println!("shutdown requested");
                break;
            }
            maybe_message = messages.next() => {
                match maybe_message {
                    Some(Ok(message)) => {
                        if let Err(error) = handle_message(&repository, &message).await {
                            eprintln!("could not acknowledge message: {error}");
                        }
                    }
                    Some(Err(error)) => eprintln!("consumer error: {error}"),
                    None => return Err("JetStream consumer ended unexpectedly".into()),
                }
            }
        }
    }

    println!("worker shutdown complete");
    Ok(())
}
