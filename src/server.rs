//! axum routes, WebSocket fan-out and the move pipeline.
//!
//! Locking discipline: `Game` sits behind a `std::sync::Mutex` and nothing awaits while holding
//! it. A move takes the lock twice — once to log it as pending and build the referee's state,
//! once to apply the verdict — with the referee call (the slow part) in between, so moves from
//! different players are judged concurrently.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use tokio::sync::{broadcast, mpsc};

use crate::game::{Game, Outcome, Policy};
use crate::protocol::{ClientMsg, ServerMsg, Snapshot};
use crate::referee::Referee;

/// Both views live in this one file; the path picks the view client-side.
pub const INDEX_HTML: &str = include_str!("../public/index.html");

/// What the phone sees when the referee hesitates.
pub const HESITATION_MESSAGE: &str = "The referee squints at you… say that again?";

#[derive(Clone)]
pub struct AppState {
    pub game: Arc<Mutex<Game>>,
    pub referee: Arc<dyn Referee>,
    /// Fan-out to every connected browser.
    pub tx: broadcast::Sender<ServerMsg>,
}

impl AppState {
    pub fn new(referee: Arc<dyn Referee>, policy: Policy) -> Self {
        let (tx, _) = broadcast::channel(512);
        Self {
            game: Arc::new(Mutex::new(Game::new(policy))),
            referee,
            tx,
        }
    }

    /// Lock the game. A poisoned lock means a panic while resolving; the state is still usable
    /// for a demo, so recover rather than take the whole room down.
    pub fn lock(&self) -> std::sync::MutexGuard<'_, Game> {
        self.game.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn broadcast(&self, msg: ServerMsg) {
        // Err means no receivers — nothing to do.
        let _ = self.tx.send(msg);
    }

    pub fn snapshot(&self, with_events: bool) -> Snapshot {
        let game = self.lock();
        snapshot_of(&game, with_events)
    }

    /// Full state for a (re)connecting browser. One lock, held for the whole build: a guard
    /// living in a temporary while another `lock()` runs would deadlock.
    pub fn welcome(&self) -> ServerMsg {
        let game = self.lock();
        ServerMsg::Welcome {
            referee: self.referee.name().to_owned(),
            mock: self.referee.is_mock(),
            policy: game.policy.clone(),
            snapshot: snapshot_of(&game, true),
        }
    }

    /// The whole life of one move. Direct replies go to `direct` (the phone that sent it);
    /// everything else is broadcast.
    pub async fn submit_move(&self, actor: &str, text: &str, direct: &mpsc::Sender<ServerMsg>) {
        let submission = {
            let mut game = self.lock();
            game.submit(actor, text)
        };
        let submission = match submission {
            Ok(s) => s,
            Err(e) => {
                let _ = direct
                    .send(ServerMsg::Rejected {
                        reason: e.to_string(),
                    })
                    .await;
                return;
            }
        };
        let seq = submission.event.seq;
        let text = submission.event.text.clone();
        self.broadcast(ServerMsg::Event(submission.event));

        // Outside the lock, concurrent with other players' moves.
        let judged = self
            .referee
            .judge(&submission.state, &submission.players)
            .await;

        let (event, reply) = {
            let mut game = self.lock();
            match judged {
                Ok(verdict) => {
                    tracing::info!(
                        seq,
                        actor,
                        text,
                        kind = %verdict.kind,
                        target = %verdict.target,
                        force = verdict.force,
                        breaks_rules = verdict.breaks_rules,
                        latency_ms = verdict.latency.as_millis() as u64,
                        input_tokens = verdict.input_tokens,
                        "verdict"
                    );
                    let event = game.settle(seq, &verdict);
                    (event, None)
                }
                Err(err) => {
                    tracing::warn!(seq, actor, code = err.code(), %err, "referee down");
                    let event = game.fail(seq, &err.to_string());
                    (
                        event,
                        Some(ServerMsg::RefereeDown {
                            code: err.code().to_owned(),
                            reason: err.to_string(),
                        }),
                    )
                }
            }
        };
        if let Some(down) = reply {
            self.broadcast(down);
        }
        let Some(event) = event else {
            // The round was restarted while the referee was thinking; the move is void.
            let _ = direct
                .send(ServerMsg::MoveResult {
                    seq,
                    text,
                    message: "The round ended before the referee could rule.".into(),
                    hesitated: false,
                })
                .await;
            return;
        };
        let (message, hesitated) = phone_line(&event.outcome, &event.summary());
        self.broadcast(ServerMsg::Event(event));
        self.broadcast(ServerMsg::Snapshot(self.snapshot(false)));
        let _ = direct
            .send(ServerMsg::MoveResult {
                seq,
                text,
                message,
                hesitated,
            })
            .await;
    }

    pub fn start_round(&self) {
        let (round, seconds) = {
            let mut game = self.lock();
            game.start_round(Instant::now());
            (game.round, game.policy.round_secs)
        };
        tracing::info!(round, "round started");
        self.broadcast(ServerMsg::RoundStarted { round, seconds });
        self.broadcast(ServerMsg::Snapshot(self.snapshot(false)));
    }

    /// One tick of the round clock: broadcast the countdown or end the round.
    pub fn tick(&self, now: Instant) {
        let ended = {
            let mut game = self.lock();
            if game.round_expired(now) {
                game.end_round();
                Some(ServerMsg::RoundEnded {
                    round: game.round,
                    leaderboard: game.leaderboard(),
                })
            } else if game.phase == crate::game::Phase::Running {
                self.broadcast(ServerMsg::Tick {
                    seconds_left: game.seconds_left(now),
                });
                None
            } else {
                None
            }
        };
        if let Some(msg) = ended {
            tracing::info!("round ended");
            self.broadcast(msg);
            self.broadcast(ServerMsg::Snapshot(self.snapshot(false)));
        }
    }

    /// Ticks once a second until the sender is dropped.
    pub async fn run_clock(self) {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            interval.tick().await;
            self.tick(Instant::now());
        }
    }
}

fn snapshot_of(game: &Game, with_events: bool) -> Snapshot {
    Snapshot {
        round: game.round,
        phase: game.phase,
        seconds_left: game.seconds_left(Instant::now()),
        players: game.players.clone(),
        leaderboard: game.leaderboard(),
        stats: game.stats.clone(),
        referee_gold: game.referee_gold,
        events: if with_events {
            game.events.clone()
        } else {
            Vec::new()
        },
    }
}

/// The one-liner the phone shows for a settled move.
fn phone_line(outcome: &Option<Outcome>, summary: &str) -> (String, bool) {
    match outcome {
        Some(Outcome::Hesitated) => (HESITATION_MESSAGE.to_owned(), true),
        Some(Outcome::Cancelled { gold_lost }) => (
            format!("The referee cancels that — cheating costs you {gold_lost} gold."),
            false,
        ),
        Some(Outcome::Applied { .. }) => (
            summary
                .split_once(" -> ")
                .map(|(_, rest)| rest.to_owned())
                .unwrap_or_else(|| summary.to_owned()),
            false,
        ),
        Some(Outcome::Fizzled { reason }) => (format!("Nothing happens: {reason}."), false),
        Some(Outcome::Failed { reason }) => {
            (format!("The referee is unavailable: {reason}"), false)
        }
        None => ("The referee is still thinking.".to_owned(), false),
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/screen", get(index))
        .route("/ws", get(ws_upgrade))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(state)
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn ws_upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| connection(socket, state))
        .into_response()
}

/// One browser. Multiplexes broadcasts, direct replies and incoming messages on the socket.
async fn connection(mut socket: WebSocket, state: AppState) {
    let mut broadcasts = state.tx.subscribe();
    let (direct_tx, mut direct_rx) = mpsc::channel::<ServerMsg>(64);
    let mut name: Option<String> = None;

    if send(&mut socket, &state.welcome()).await.is_err() {
        return;
    }

    loop {
        tokio::select! {
            incoming = socket.recv() => {
                let Some(Ok(msg)) = incoming else { break };
                let text = match msg {
                    Message::Text(t) => t.to_string(),
                    Message::Close(_) => break,
                    _ => continue,
                };
                match serde_json::from_str::<ClientMsg>(&text) {
                    Ok(ClientMsg::Join { name: wanted }) => {
                        let joined = state.lock().join(&wanted);
                        match joined {
                            Ok(n) => {
                                tracing::info!(name = %n, "joined");
                                name = Some(n.clone());
                                let _ = direct_tx.send(ServerMsg::Joined { name: n }).await;
                                state.broadcast(ServerMsg::Snapshot(state.snapshot(false)));
                            }
                            Err(e) => {
                                let _ = direct_tx.send(ServerMsg::Rejected { reason: e.to_string() }).await;
                            }
                        }
                    }
                    Ok(ClientMsg::Move { text }) => match &name {
                        Some(actor) => {
                            let (state, actor, direct) = (state.clone(), actor.clone(), direct_tx.clone());
                            tokio::spawn(async move { state.submit_move(&actor, &text, &direct).await });
                        }
                        None => {
                            let _ = direct_tx.send(ServerMsg::Rejected { reason: "join first".into() }).await;
                        }
                    },
                    Ok(ClientMsg::StartRound) => state.start_round(),
                    Err(e) => {
                        let _ = direct_tx.send(ServerMsg::Rejected { reason: format!("bad message: {e}") }).await;
                    }
                }
            }
            Some(msg) = direct_rx.recv() => {
                if send(&mut socket, &msg).await.is_err() { break }
            }
            fanned = broadcasts.recv() => match fanned {
                Ok(msg) => if send(&mut socket, &msg).await.is_err() { break },
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "slow client, resyncing");
                    if send(&mut socket, &state.welcome()).await.is_err() { break }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
    if let Some(n) = name {
        tracing::info!(name = %n, "disconnected");
    }
}

async fn send(socket: &mut WebSocket, msg: &ServerMsg) -> Result<(), axum::Error> {
    let text = serde_json::to_string(msg).expect("ServerMsg serializes");
    socket.send(Message::Text(text.into())).await
}

/// Serve until ctrl-c. The clock task is spawned here.
pub async fn serve(state: AppState, addr: SocketAddr) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(
        "listening on http://{}  (screen: /screen, phones: /)",
        listener.local_addr()?
    );
    tokio::spawn(state.clone().run_clock());
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutting down");
        })
        .await
}
