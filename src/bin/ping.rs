//! `just ping`: one raw SDK call to verify the key and print latency and usage.
use std::time::Instant;

use tavern_brawl::referee::{MoveState, questions};
use typesafe::Client;

#[tokio::main]
async fn main() -> typesafe::Result<()> {
    let client = Client::from_env()?;
    let players = vec!["Ana".to_string(), "Bob".to_string()];
    let state = MoveState {
        arena_rules: tavern_brawl::game::ARENA_RULES,
        round: 1,
        actor: "Ana".into(),
        move_text: "throw a chair at Bob".into(),
        live_players: vec![],
        recent_events: vec![],
    };
    let t0 = Instant::now();
    let res = client.system_one(&state, questions(&players)).await?;
    let ms = t0.elapsed().as_millis();
    println!(
        "model {}  latency {ms} ms  attempts {}  request_id {:?}",
        res.model,
        res.meta.attempts,
        res.request_id()
    );
    println!("usage {:?}", res.usage);
    for (name, answer) in &res.answers {
        println!("  {name}: {answer:?}");
    }
    Ok(())
}
