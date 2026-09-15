use std::net::SocketAddr;

use axum::{Router, http::StatusCode, routing::get};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let address = SocketAddr::from(([0, 0, 0, 0], 3000));
    let listener = TcpListener::bind(address).await?;

    println!("parking-api listening on http://{address}");
    axum::serve(listener, app()).await
}

fn app() -> Router {
    Router::new().route("/health", get(health))
}

async fn health() -> StatusCode {
    StatusCode::OK
}
