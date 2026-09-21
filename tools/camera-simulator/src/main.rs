use std::{collections::HashMap, env, error::Error};

use async_nats::jetstream::{context::traits::Publisher, message::PublishMessage};
use chrono::Utc;
use parking_events::{
    ObservedStateV1, PARKING_OBSERVATIONS_V1_SUBJECT, SPOT_OBSERVATION_V1_SCHEMA_VERSION,
    SpotObservationV1,
};
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let arguments = parse_arguments(env::args().skip(1))?;
    let nats_url =
        env::var("NATS_URL").map_err(|_| "NATS_URL must be set before running the simulator")?;
    let event_id = Uuid::new_v4();
    let event = SpotObservationV1 {
        schema_version: SPOT_OBSERVATION_V1_SCHEMA_VERSION,
        event_id,
        camera_id: parse_uuid(&arguments, "--camera-id")?,
        spot_id: parse_uuid(&arguments, "--spot-id")?,
        sequence: required(&arguments, "--sequence")?
            .parse()
            .map_err(|_| "--sequence must be an unsigned integer")?,
        observed_at: Utc::now(),
        state: parse_state(required(&arguments, "--state")?)?,
        model_score: None,
        model_version: Some("simulator-v1".into()),
    };

    let client = async_nats::connect(&nats_url)
        .await
        .map_err(|error| format!("could not connect to NATS at configured NATS_URL: {error}"))?;
    let context = async_nats::jetstream::new(client);
    let payload = serde_json::to_vec(&event)?;
    let message = PublishMessage::build()
        .payload(payload.into())
        .message_id(event_id.to_string())
        .outbound_message(PARKING_OBSERVATIONS_V1_SUBJECT);
    let acknowledgement = context
        .publish_message(message)
        .await
        .map_err(|error| {
            format!("JetStream publish failed; ensure the worker provisioned the stream: {error}")
        })?
        .await
        .map_err(|error| format!("JetStream did not acknowledge the publish: {error}"))?;

    println!(
        "published event {event_id}; stream sequence {}",
        acknowledgement.sequence
    );
    Ok(())
}

fn parse_arguments(
    arguments: impl Iterator<Item = String>,
) -> Result<HashMap<String, String>, Box<dyn Error>> {
    let mut parsed = HashMap::new();
    let mut arguments = arguments;
    while let Some(name) = arguments.next() {
        if !name.starts_with("--") {
            return Err(format!("unexpected argument: {name}").into());
        }
        let value = arguments
            .next()
            .ok_or_else(|| format!("missing value for {name}"))?;
        parsed.insert(name, value);
    }
    Ok(parsed)
}

fn required<'a>(
    arguments: &'a HashMap<String, String>,
    name: &str,
) -> Result<&'a str, Box<dyn Error>> {
    arguments
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| format!("required argument {name} is missing").into())
}

fn parse_uuid(arguments: &HashMap<String, String>, name: &str) -> Result<Uuid, Box<dyn Error>> {
    required(arguments, name)?
        .parse()
        .map_err(|_| format!("{name} must be a valid UUID").into())
}

fn parse_state(value: &str) -> Result<ObservedStateV1, Box<dyn Error>> {
    match value {
        "free" => Ok(ObservedStateV1::Free),
        "occupied" => Ok(ObservedStateV1::Occupied),
        "uncertain" => Ok(ObservedStateV1::Uncertain),
        _ => Err("--state must be one of: free, occupied, uncertain".into()),
    }
}
