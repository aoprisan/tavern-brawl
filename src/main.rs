use std::net::SocketAddr;
use std::sync::Arc;

use tavern_brawl::game::Policy;
use tavern_brawl::referee::{JevReferee, MockReferee, Referee};
use tavern_brawl::server::{AppState, serve};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("tavern_brawl=info,typesafe=info")),
        )
        .init();

    let referee: Arc<dyn Referee> = if std::env::var("MOCK").is_ok_and(|v| v == "1") {
        tracing::warn!("MOCK=1: keyword mock referee — this is not Jev");
        Arc::new(MockReferee::new())
    } else {
        match JevReferee::from_env() {
            Ok(r) => {
                tracing::info!(model = r.model(), "referee: Jev via typesafe-ai-sdk");
                Arc::new(r)
            }
            Err(e) => {
                eprintln!("cannot start the Jev referee: {e}");
                eprintln!("set TYPESAFE_API_KEY in .env (see .env.example) or run with MOCK=1");
                std::process::exit(2);
            }
        }
    };

    let host = std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3000);
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .unwrap_or_else(|e| panic!("bad HOST/PORT {host}:{port}: {e}"));

    let state = AppState::new(referee, Policy::default());
    if let Err(e) = serve(state, addr).await {
        eprintln!("server error: {e}");
        std::process::exit(1);
    }
}
