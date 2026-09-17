//! WebSocket protocol, defined once. Both directions are JSON objects tagged by `type`.
use serde::{Deserialize, Serialize};

use crate::game::{Event, LeaderboardRow, Phase, Player, PlayerName, Policy, Stats};

/// Browser → server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Phone: enter the room under a name (rejoining with the same name is allowed).
    Join { name: String },
    /// Phone: a free-form move.
    Move { text: String },
    /// Screen: start (or restart) a round.
    StartRound,
}

/// Server → browser.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    /// First message on every connection.
    Welcome {
        referee: String,
        mock: bool,
        policy: Policy,
        snapshot: Snapshot,
    },
    /// Reply to `Join`.
    Joined {
        name: PlayerName,
    },
    /// Room state after any change (players, phase, stats, timer).
    Snapshot(Snapshot),
    /// A log entry was created (pending) or updated (verdict + outcome).
    Event(Event),
    /// Direct reply to the phone that sent a `Move`.
    MoveResult {
        seq: u64,
        /// The move as submitted, so the phone can pair reply and text even out of order.
        text: String,
        /// Short line for the phone, e.g. "The referee squints at you… say that again?".
        message: String,
        hesitated: bool,
    },
    RoundStarted {
        round: u32,
        seconds: u64,
    },
    Tick {
        seconds_left: u64,
    },
    RoundEnded {
        round: u32,
        leaderboard: Vec<LeaderboardRow>,
    },
    /// The referee failed to judge a move (SDK error). Shown on the screen.
    RefereeDown {
        code: String,
        reason: String,
    },
    /// A client message was refused (bad name, round not running, …).
    Rejected {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub round: u32,
    pub phase: Phase,
    pub seconds_left: u64,
    pub players: Vec<Player>,
    pub leaderboard: Vec<LeaderboardRow>,
    pub stats: Stats,
    pub referee_gold: i32,
    /// The full log in `Welcome`; empty in incremental snapshots (the log arrives as `Event`s).
    #[serde(default)]
    pub events: Vec<Event>,
}
