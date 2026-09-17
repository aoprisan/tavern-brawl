//! Offline referee: keyword rules with jittered probabilities and a fake 60–170 ms latency.
//! Plausible enough to rehearse the demo; it is not Jev and the UI says so.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::game::{Kind, PlayerName, Target};

use super::{MoveState, Referee, RefereeError, Verdict};

pub struct MockReferee {
    rng: AtomicU64,
}

impl Default for MockReferee {
    fn default() -> Self {
        Self::new()
    }
}

impl MockReferee {
    pub fn new() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15)
            | 1;
        Self {
            rng: AtomicU64::new(seed),
        }
    }

    /// xorshift64*, good enough for jitter; avoids a `rand` dependency.
    fn next_f32(&self) -> f32 {
        let mut x = self.rng.load(Ordering::Relaxed);
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng.store(x, Ordering::Relaxed);
        let bits = x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40;
        bits as f32 / (1u64 << 24) as f32
    }

    /// `base ± spread`, clamped to [0, 1].
    fn jitter(&self, base: f32, spread: f32) -> f32 {
        (base + (self.next_f32() * 2.0 - 1.0) * spread).clamp(0.0, 1.0)
    }

    /// Pure classification, exposed for tests.
    pub fn classify(text: &str, players: &[PlayerName]) -> Classification {
        let lower = text.to_lowercase();
        let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));

        // "bribe the referee" is a legitimate move; impersonating or instructing the referee is not
        let breaks_rules = has(&[
            "i am the referee",
            "as the referee",
            "i'm the referee",
            "the rules",
            "rule:",
            "i win",
            "infinite",
            "ignore",
            "system",
            "you are",
            "you must",
            "instant kill",
            "god mode",
            "immortal",
            "invincible",
            "instruction",
            "prompt",
        ]);

        let (kind, kind_confidence) = if has(&["bribe", "slip", "coin to", "tip the", "pay the"]) {
            (Kind::Bribe, 0.88)
        } else if has(&[
            "steal",
            "pickpocket",
            "rob",
            "snatch",
            "purse",
            "pocket",
            "loot",
        ]) {
            (Kind::Steal, 0.86)
        } else if has(&["heal", "potion", "bandage", "drink", "rest", "eat", "cure"]) {
            (Kind::Heal, 0.85)
        } else if has(&[
            "hide", "block", "duck", "dodge", "shield", "cover", "parry", "defend",
        ]) {
            (Kind::Defend, 0.87)
        } else if has(&[
            "throw", "hit", "punch", "kick", "stab", "smash", "fireball", "cast", "attack", "slap",
            "shove", "swing", "hurl", "bottle", "chair", "table", "strike", "burn", "kill",
        ]) {
            (Kind::Attack, 0.9)
        } else if breaks_rules || has(&["sing", "dance", "talk", "say", "shout", "laugh", "drink"])
        {
            // a clear cheat is confidently "nonsense", so the rule-break cancels it instead of a squint
            (Kind::Nonsense, 0.8)
        } else {
            (Kind::Nonsense, 0.42)
        };

        let mut target_confidence = 0.9;
        let named: Vec<&PlayerName> = players
            .iter()
            .filter(|p| lower.contains(&p.to_lowercase()))
            .collect();
        let target = if has(&[
            "everyone",
            "everybody",
            "all of",
            "the room",
            "whole tavern",
        ]) {
            Target::Everyone
        } else if let [one] = named[..] {
            Target::Player(one.clone())
        } else if named.len() > 1 {
            target_confidence = 0.38;
            Target::Player(named[0].clone())
        } else if matches!(kind, Kind::Attack | Kind::Steal) {
            // an attack with no one named: the referee cannot tell who
            target_confidence = 0.35;
            Target::Nobody
        } else {
            Target::Nobody
        };

        let force = if has(&[
            "fireball", "explode", "collapse", "devastat", "kill", "burn",
        ]) {
            4.0
        } else if has(&["table", "bottle", "smash", "swing", "stab", "axe"]) {
            3.0
        } else if has(&[
            "chair", "throw", "punch", "kick", "hurl", "mug", "hit", "bribe", "steal",
        ]) {
            2.0
        } else if has(&["slap", "shove", "poke", "pinch", "flick"]) {
            1.0
        } else if kind == Kind::Nonsense {
            0.0
        } else {
            2.0
        };

        Classification {
            kind,
            kind_confidence,
            target,
            target_confidence,
            force,
            breaks_rules: if breaks_rules { 0.92 } else { 0.05 },
        }
    }
}

/// Keyword classification before jitter.
#[derive(Debug, Clone, PartialEq)]
pub struct Classification {
    pub kind: Kind,
    pub kind_confidence: f32,
    pub target: Target,
    pub target_confidence: f32,
    pub force: f32,
    pub breaks_rules: f32,
}

#[async_trait::async_trait]
impl Referee for MockReferee {
    async fn judge(
        &self,
        state: &MoveState,
        players: &[PlayerName],
    ) -> Result<Verdict, RefereeError> {
        let started = Instant::now();
        let delay_ms = 60 + (self.next_f32() * 110.0) as u64;
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;

        let c = Self::classify(&state.move_text, players);
        // roughly what the API would bill for this state + question set
        let input_tokens = (serde_json::to_string(state).map(|s| s.len()).unwrap_or(0)
            + serde_json::to_string(&super::questions(players))
                .map(|s| s.len())
                .unwrap_or(0))
            / 4;

        Ok(Verdict {
            kind: c.kind,
            kind_confidence: self.jitter(c.kind_confidence, 0.06),
            target: c.target,
            target_confidence: self.jitter(c.target_confidence, 0.06),
            force: (c.force + (self.next_f32() - 0.5) * 0.4).clamp(0.0, 4.0),
            force_confidence: self.jitter(0.7, 0.1),
            breaks_rules: self.jitter(c.breaks_rules, 0.04),
            latency: started.elapsed(),
            input_tokens: input_tokens as u64,
        })
    }

    fn name(&self) -> &'static str {
        "Mock referee — not Jev"
    }

    fn is_mock(&self) -> bool {
        true
    }
}
