//! The referee: one Jev call per move, four typed questions, one [`Verdict`].
//!
//! [`JevReferee`] talks to the API through the `typesafe` SDK; [`MockReferee`] answers offline
//! from keyword rules. The game only ever sees the [`Referee`] trait.

mod jev;
mod mock;
mod questions;

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::game::{Kind, PlayerName, Target};

pub use jev::JevReferee;
pub use mock::MockReferee;
pub use questions::{
    BREAKS_RULES_INSTRUCTIONS, FORCE_INSTRUCTIONS, FORCE_LEVELS, KIND_INSTRUCTIONS,
    TARGET_INSTRUCTIONS, questions,
};

/// Judges moves. `judge` must be safe to call concurrently from many tasks.
#[async_trait::async_trait]
pub trait Referee: Send + Sync {
    /// Judge one move. `players` are the live players — the dynamic `target` options.
    async fn judge(
        &self,
        state: &MoveState,
        players: &[PlayerName],
    ) -> Result<Verdict, RefereeError>;

    /// Shown in the UI banner.
    fn name(&self) -> &'static str;

    /// True for the keyword mock, so the UI can say so.
    fn is_mock(&self) -> bool {
        false
    }
}

/// What the referee is told about the room. Kept small: Jev's state + questions budget is ~32k
/// tokens, and we want ~100 ms answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MoveState {
    pub arena_rules: &'static str,
    pub round: u32,
    pub actor: PlayerName,
    pub move_text: String,
    pub live_players: Vec<PlayerSummary>,
    /// Last few resolved events as one-liners, oldest first.
    pub recent_events: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerSummary {
    pub name: PlayerName,
    pub hp: i32,
    pub gold: i32,
    pub defending: bool,
    pub immunities: u32,
}

/// The referee's typed answers for one move, plus what it cost.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub kind: Kind,
    pub kind_confidence: f32,
    pub target: Target,
    pub target_confidence: f32,
    /// Probability-weighted force level, 0–4.
    pub force: f32,
    pub force_confidence: f32,
    /// Probability that the move tries to cheat.
    pub breaks_rules: f32,
    /// Wall time of the call, retries included.
    pub latency: Duration,
    /// Input tokens billed for this call (0 if the API did not report them).
    pub input_tokens: u64,
}

/// Why the referee could not judge a move. Every variant reaches the screen as `RefereeDown`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefereeError {
    /// 401/403: the key is missing, wrong or not allowed.
    Auth(String),
    /// 400/422 or a request the SDK rejected locally — our bug or setup.
    InvalidRequest(String),
    /// 429/5xx (incl. 529) after the SDK's retries.
    Overloaded(String),
    /// Connection error or timeout after retries.
    Unreachable(String),
    /// 2xx but the answers are unusable (missing question, unknown label, validation error).
    BadAnswer(String),
    /// Anything the SDK adds in a future version.
    Other(String),
}

impl RefereeError {
    /// Short label for the UI.
    pub fn code(&self) -> &'static str {
        match self {
            RefereeError::Auth(_) => "auth",
            RefereeError::InvalidRequest(_) => "invalid_request",
            RefereeError::Overloaded(_) => "overloaded",
            RefereeError::Unreachable(_) => "unreachable",
            RefereeError::BadAnswer(_) => "bad_answer",
            RefereeError::Other(_) => "other",
        }
    }
}

impl fmt::Display for RefereeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RefereeError::Auth(m) => write!(f, "authentication failed: {m}"),
            RefereeError::InvalidRequest(m) => write!(f, "invalid request: {m}"),
            RefereeError::Overloaded(m) => write!(f, "API overloaded or rate-limited: {m}"),
            RefereeError::Unreachable(m) => write!(f, "API unreachable: {m}"),
            RefereeError::BadAnswer(m) => write!(f, "unusable answer: {m}"),
            RefereeError::Other(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for RefereeError {}

impl From<typesafe::Error> for RefereeError {
    fn from(e: typesafe::Error) -> Self {
        use typesafe::ApiErrorKind as K;
        match &e {
            typesafe::Error::Api(api) => {
                let msg = api.to_string();
                match api.kind {
                    K::Authentication | K::PermissionDenied => RefereeError::Auth(msg),
                    K::BadRequest | K::UnprocessableEntity | K::NotFound => {
                        RefereeError::InvalidRequest(msg)
                    }
                    K::RateLimit | K::InternalServer => {
                        let retry_after = api
                            .retry_after()
                            .map(|d| format!(" (retry after {:.1}s)", d.as_secs_f64()))
                            .unwrap_or_default();
                        RefereeError::Overloaded(format!("{msg}{retry_after}"))
                    }
                    _ => RefereeError::Other(msg),
                }
            }
            typesafe::Error::Connection(_) | typesafe::Error::Timeout(_) => {
                RefereeError::Unreachable(e.to_string())
            }
            typesafe::Error::InvalidRequest(m) | typesafe::Error::Config(m) => {
                RefereeError::InvalidRequest(m.clone())
            }
            typesafe::Error::ResponseValidation(v) => {
                RefereeError::BadAnswer(format!("{} at {}", v.detail, v.field_path))
            }
            _ => RefereeError::Other(e.to_string()),
        }
    }
}
