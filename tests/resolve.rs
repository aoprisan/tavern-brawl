//! Criterion 3: resolution is fully unit-tested with hand-built verdicts. No referee involved.

use std::time::{Duration, Instant};

use tavern_brawl::game::{Effect, Game, Kind, Outcome, Phase, Policy, Target};
use tavern_brawl::referee::Verdict;

fn verdict(kind: Kind, target: Target, force: f32) -> Verdict {
    Verdict {
        kind,
        kind_confidence: 0.9,
        target: target.clone(),
        target_confidence: 0.9,
        force,
        force_confidence: 0.8,
        breaks_rules: 0.05,
        latency: Duration::from_millis(100),
        input_tokens: 500,
    }
}

fn player(name: &str) -> Target {
    Target::Player(name.to_owned())
}

/// A running room with Ana, Bob and Cid at starting HP/gold.
fn room() -> Game {
    let mut g = Game::new(Policy::default());
    for n in ["Ana", "Bob", "Cid"] {
        g.join(n).unwrap();
    }
    g.start_round(Instant::now());
    g
}

fn hp(g: &Game, name: &str) -> i32 {
    g.player(name).unwrap().hp
}

fn gold(g: &Game, name: &str) -> i32 {
    g.player(name).unwrap().gold
}

fn effects(outcome: &Outcome) -> &[Effect] {
    match outcome {
        Outcome::Applied { effects } => effects,
        other => panic!("expected Applied, got {other:?}"),
    }
}

#[test]
fn attack_deals_force_damage_to_target() {
    let mut g = room();
    let out = g.resolve("Ana", &verdict(Kind::Attack, player("Bob"), 3.0));
    assert_eq!(
        effects(&out),
        [Effect::Damage {
            target: "Bob".into(),
            amount: 3,
            halved: false
        }]
    );
    assert_eq!(hp(&g, "Bob"), 7);
    assert_eq!(hp(&g, "Ana"), 10);
    assert_eq!(g.player("Ana").unwrap().moves_judged, 1);
}

#[test]
fn force_is_rounded_and_clamped() {
    let mut g = room();
    g.resolve("Ana", &verdict(Kind::Attack, player("Bob"), 2.6));
    assert_eq!(hp(&g, "Bob"), 7, "2.6 rounds to 3");
    g.resolve("Ana", &verdict(Kind::Attack, player("Bob"), 9.0));
    assert_eq!(hp(&g, "Bob"), 3, "clamped to max_force 4");
    g.resolve("Ana", &verdict(Kind::Attack, player("Bob"), -1.0));
    assert_eq!(hp(&g, "Bob"), 3, "negative force does nothing");
}

#[test]
fn attack_everyone_hits_every_other_live_player_with_force_minus_one() {
    let mut g = room();
    g.player_mut("Cid").unwrap().hp = 0; // Cid is down and must be skipped
    let out = g.resolve("Ana", &verdict(Kind::Attack, Target::Everyone, 4.0));
    assert_eq!(
        effects(&out),
        [Effect::Damage {
            target: "Bob".into(),
            amount: 3,
            halved: false
        }]
    );
    assert_eq!(hp(&g, "Ana"), 10, "the actor is not hit");
    assert_eq!(hp(&g, "Bob"), 7);
    assert_eq!(hp(&g, "Cid"), 0);
}

#[test]
fn attack_nobody_misses() {
    let mut g = room();
    let out = g.resolve("Ana", &verdict(Kind::Attack, Target::Nobody, 4.0));
    assert!(matches!(effects(&out), [Effect::Missed { .. }]));
    assert!(g.players.iter().all(|p| p.hp == 10));
}

#[test]
fn attack_on_downed_or_unknown_player_misses() {
    let mut g = room();
    g.player_mut("Bob").unwrap().hp = 0;
    let out = g.resolve("Ana", &verdict(Kind::Attack, player("Bob"), 4.0));
    assert!(matches!(effects(&out), [Effect::Missed { .. }]));
    let out = g.resolve("Ana", &verdict(Kind::Attack, player("Zed"), 4.0));
    assert!(matches!(effects(&out), [Effect::Missed { .. }]));
}

#[test]
fn lethal_hit_downs_the_target_and_floors_hp_at_zero() {
    let mut g = room();
    g.player_mut("Bob").unwrap().hp = 2;
    let out = g.resolve("Ana", &verdict(Kind::Attack, player("Bob"), 4.0));
    assert_eq!(
        effects(&out),
        [
            Effect::Damage {
                target: "Bob".into(),
                amount: 4,
                halved: false
            },
            Effect::Downed {
                target: "Bob".into()
            }
        ]
    );
    assert_eq!(hp(&g, "Bob"), 0);
    assert_eq!(g.live_players(), ["Ana", "Cid"]);
}

#[test]
fn defend_halves_the_next_hit_only() {
    let mut g = room();
    let out = g.resolve("Bob", &verdict(Kind::Defend, Target::Nobody, 0.0));
    assert_eq!(
        effects(&out),
        [Effect::Defending {
            player: "Bob".into()
        }]
    );
    assert!(g.player("Bob").unwrap().defending);

    let out = g.resolve("Ana", &verdict(Kind::Attack, player("Bob"), 3.0));
    assert_eq!(
        effects(&out),
        [Effect::Damage {
            target: "Bob".into(),
            amount: 2,
            halved: true
        }],
        "3 halved rounds up to 2"
    );
    assert_eq!(hp(&g, "Bob"), 8);
    assert!(!g.player("Bob").unwrap().defending, "defence is spent");

    g.resolve("Ana", &verdict(Kind::Attack, player("Bob"), 4.0));
    assert_eq!(hp(&g, "Bob"), 4, "second hit lands in full");
}

#[test]
fn steal_moves_force_gold_capped_by_victims_purse() {
    let mut g = room();
    let out = g.resolve("Ana", &verdict(Kind::Steal, player("Bob"), 3.0));
    assert_eq!(
        effects(&out),
        [Effect::Stole {
            from: "Bob".into(),
            to: "Ana".into(),
            gold: 3
        }]
    );
    assert_eq!(gold(&g, "Ana"), 8);
    assert_eq!(gold(&g, "Bob"), 2);

    g.resolve("Ana", &verdict(Kind::Steal, player("Bob"), 4.0));
    assert_eq!(gold(&g, "Bob"), 0, "cannot go negative");
    assert_eq!(gold(&g, "Ana"), 10);
}

#[test]
fn steal_from_self_or_nobody_misses_and_everyone_uses_the_malus() {
    let mut g = room();
    let out = g.resolve("Ana", &verdict(Kind::Steal, player("Ana"), 3.0));
    assert!(matches!(effects(&out), [Effect::Missed { .. }]));
    let out = g.resolve("Ana", &verdict(Kind::Steal, Target::Nobody, 3.0));
    assert!(matches!(effects(&out), [Effect::Missed { .. }]));
    assert_eq!(gold(&g, "Ana"), 5);

    g.resolve("Ana", &verdict(Kind::Steal, Target::Everyone, 3.0));
    assert_eq!(gold(&g, "Bob"), 3);
    assert_eq!(gold(&g, "Cid"), 3);
    assert_eq!(gold(&g, "Ana"), 9);
}

#[test]
fn heal_restores_force_hp_up_to_the_cap() {
    let mut g = room();
    g.player_mut("Ana").unwrap().hp = 5;
    let out = g.resolve("Ana", &verdict(Kind::Heal, Target::Nobody, 3.0));
    assert_eq!(
        effects(&out),
        [Effect::Healed {
            target: "Ana".into(),
            amount: 3
        }],
        "heal with no target heals the actor"
    );
    assert_eq!(hp(&g, "Ana"), 8);

    g.resolve("Ana", &verdict(Kind::Heal, player("Ana"), 4.0));
    assert_eq!(hp(&g, "Ana"), 10, "capped at start_hp");

    g.player_mut("Bob").unwrap().hp = 1;
    g.resolve("Ana", &verdict(Kind::Heal, player("Bob"), 2.0));
    assert_eq!(hp(&g, "Bob"), 3, "can heal others");
}

#[test]
fn heal_everyone_heals_self_fully_and_others_with_the_malus() {
    let mut g = room();
    for p in &mut g.players {
        p.hp = 1;
    }
    g.resolve("Ana", &verdict(Kind::Heal, Target::Everyone, 3.0));
    assert_eq!(hp(&g, "Ana"), 4);
    assert_eq!(hp(&g, "Bob"), 3);
    assert_eq!(hp(&g, "Cid"), 3);
}

#[test]
fn bribe_pays_the_referee_and_buys_one_immunity() {
    let mut g = room();
    let out = g.resolve("Bob", &verdict(Kind::Bribe, Target::Nobody, 2.0));
    assert_eq!(
        effects(&out),
        [Effect::Bribed {
            gold: 2,
            immunity: true
        }]
    );
    assert_eq!(gold(&g, "Bob"), 3);
    assert_eq!(g.referee_gold, 2);
    assert_eq!(g.player("Bob").unwrap().immunities, 1);

    let out = g.resolve("Ana", &verdict(Kind::Attack, player("Bob"), 4.0));
    assert_eq!(
        effects(&out),
        [Effect::Immune {
            target: "Bob".into()
        }]
    );
    assert_eq!(hp(&g, "Bob"), 10, "the hit was absorbed");
    assert_eq!(g.player("Bob").unwrap().immunities, 0, "immunity is spent");

    g.resolve("Ana", &verdict(Kind::Attack, player("Bob"), 4.0));
    assert_eq!(hp(&g, "Bob"), 6, "the next one lands");
}

#[test]
fn bribe_without_gold_buys_nothing() {
    let mut g = room();
    g.player_mut("Bob").unwrap().gold = 0;
    let out = g.resolve("Bob", &verdict(Kind::Bribe, Target::Nobody, 3.0));
    assert_eq!(
        effects(&out),
        [Effect::Bribed {
            gold: 0,
            immunity: false
        }]
    );
    assert_eq!(g.player("Bob").unwrap().immunities, 0);
    assert_eq!(g.referee_gold, 0);
}

#[test]
fn bribe_is_capped_by_the_purse() {
    let mut g = room();
    g.player_mut("Bob").unwrap().gold = 1;
    g.resolve("Bob", &verdict(Kind::Bribe, Target::Nobody, 4.0));
    assert_eq!(gold(&g, "Bob"), 0);
    assert_eq!(g.referee_gold, 1);
    assert_eq!(g.player("Bob").unwrap().immunities, 1);
}

#[test]
fn nonsense_does_nothing() {
    let mut g = room();
    let out = g.resolve("Ana", &verdict(Kind::Nonsense, Target::Everyone, 4.0));
    assert_eq!(effects(&out), [Effect::Nothing]);
    assert!(g.players.iter().all(|p| p.hp == 10 && p.gold == 5));
}

#[test]
fn rules_break_cancels_the_move_and_costs_one_gold() {
    let mut g = room();
    let mut v = verdict(Kind::Attack, player("Bob"), 4.0);
    v.breaks_rules = 0.7; // exactly at the threshold counts
    let out = g.resolve("Ana", &v);
    assert_eq!(out, Outcome::Cancelled { gold_lost: 1 });
    assert_eq!(gold(&g, "Ana"), 4);
    assert_eq!(hp(&g, "Bob"), 10, "nothing else happened");

    v.breaks_rules = 0.69;
    let out = g.resolve("Ana", &v);
    assert!(
        matches!(out, Outcome::Applied { .. }),
        "just below: resolves"
    );
    assert_eq!(hp(&g, "Bob"), 6);
}

#[test]
fn rules_break_penalty_cannot_push_gold_negative() {
    let mut g = room();
    g.player_mut("Ana").unwrap().gold = 0;
    let mut v = verdict(Kind::Attack, player("Bob"), 4.0);
    v.breaks_rules = 0.99;
    assert_eq!(g.resolve("Ana", &v), Outcome::Cancelled { gold_lost: 0 });
    assert_eq!(gold(&g, "Ana"), 0);
}

#[test]
fn hesitation_on_low_kind_confidence() {
    let mut g = room();
    let mut v = verdict(Kind::Attack, player("Bob"), 4.0);
    v.kind_confidence = 0.49;
    assert_eq!(g.resolve("Ana", &v), Outcome::Hesitated);
    assert_eq!(hp(&g, "Bob"), 10);
    assert_eq!(gold(&g, "Ana"), 5, "hesitation is free");
    assert_eq!(
        g.player("Ana").unwrap().moves_judged,
        1,
        "but it was judged"
    );
}

#[test]
fn hesitation_on_low_target_confidence() {
    let mut g = room();
    let mut v = verdict(Kind::Attack, player("Bob"), 4.0);
    v.target_confidence = 0.3;
    assert_eq!(g.resolve("Ana", &v), Outcome::Hesitated);
    assert_eq!(hp(&g, "Bob"), 10);
}

#[test]
fn hesitation_takes_precedence_over_rules_break() {
    let mut g = room();
    let mut v = verdict(Kind::Attack, player("Bob"), 4.0);
    v.kind_confidence = 0.2;
    v.breaks_rules = 0.99;
    assert_eq!(g.resolve("Ana", &v), Outcome::Hesitated);
    assert_eq!(gold(&g, "Ana"), 5, "no penalty when the referee is unsure");
}

#[test]
fn confidence_exactly_at_threshold_resolves() {
    let mut g = room();
    let mut v = verdict(Kind::Attack, player("Bob"), 2.0);
    v.kind_confidence = 0.5;
    v.target_confidence = 0.5;
    assert!(matches!(g.resolve("Ana", &v), Outcome::Applied { .. }));
    assert_eq!(hp(&g, "Bob"), 8);
}

#[test]
fn downed_actor_fizzles() {
    let mut g = room();
    g.player_mut("Ana").unwrap().hp = 0;
    let out = g.resolve("Ana", &verdict(Kind::Attack, player("Bob"), 4.0));
    assert!(matches!(out, Outcome::Fizzled { .. }));
    assert_eq!(hp(&g, "Bob"), 10);
}

#[test]
fn thresholds_come_from_the_policy() {
    let policy = Policy {
        hesitation_confidence: 0.9,
        breaks_rules_threshold: 0.2,
        rules_break_penalty_gold: 3,
        everyone_malus: 0,
        ..Policy::default()
    };
    let mut g = Game::new(policy);
    g.join("Ana").unwrap();
    g.join("Bob").unwrap();
    g.start_round(Instant::now());

    let v = verdict(Kind::Attack, player("Bob"), 4.0); // confidences 0.9 → ok at 0.9
    let mut low = v.clone();
    low.kind_confidence = 0.85;
    assert_eq!(g.resolve("Ana", &low), Outcome::Hesitated);

    let mut cheat = v.clone();
    cheat.breaks_rules = 0.25;
    assert_eq!(
        g.resolve("Ana", &cheat),
        Outcome::Cancelled { gold_lost: 3 }
    );
    assert_eq!(gold(&g, "Ana"), 2);

    g.resolve("Ana", &verdict(Kind::Attack, Target::Everyone, 4.0));
    assert_eq!(hp(&g, "Bob"), 6, "everyone_malus 0 → full force");
}

#[test]
fn submit_and_settle_keep_the_log_in_submission_order_and_update_stats() {
    let mut g = room();
    let a = g.submit("Ana", "throw a chair at Bob").unwrap();
    let b = g.submit("Bob", "hide under the table").unwrap();
    assert_eq!(a.event.seq, 1);
    assert_eq!(b.event.seq, 2);
    assert_eq!(a.players, ["Ana", "Bob", "Cid"]);
    assert_eq!(a.state.actor, "Ana");
    assert_eq!(a.state.move_text, "throw a chair at Bob");
    assert!(g.events.iter().all(|e| e.pending()));

    // verdicts arrive out of order
    let eb = g
        .settle(2, &verdict(Kind::Defend, Target::Nobody, 0.0))
        .unwrap();
    let ea = g
        .settle(1, &verdict(Kind::Attack, player("Bob"), 3.0))
        .unwrap();
    assert_eq!(eb.seq, 2);
    assert_eq!(ea.seq, 1);
    let seqs: Vec<u64> = g.events.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, [1, 2]);
    assert_eq!(hp(&g, "Bob"), 8, "Bob was defending when the chair landed");

    assert_eq!(g.stats.moves_judged, 2);
    assert_eq!(g.stats.input_tokens, 1000);
    assert_eq!(g.stats.median_latency_ms, 100);
    assert!((g.stats.spend_usd - 1000.0 * 0.042 / 1e6).abs() < 1e-12);

    // the next move's state carries the resolved history, oldest first
    let c = g.submit("Cid", "steal Ana's purse").unwrap();
    assert_eq!(c.state.recent_events.len(), 2);
    assert!(c.state.recent_events[0].starts_with("Ana:"));
    assert!(c.state.recent_events[1].starts_with("Bob:"));
    assert_eq!(c.state.live_players.len(), 3);
}

#[test]
fn settle_counts_hesitations_and_cancellations() {
    let mut g = room();
    g.submit("Ana", "a").unwrap();
    g.submit("Ana", "b").unwrap();
    let mut v = verdict(Kind::Attack, player("Bob"), 1.0);
    v.kind_confidence = 0.1;
    let e = g.settle(1, &v).unwrap();
    assert_eq!(e.outcome, Some(Outcome::Hesitated));
    v.kind_confidence = 0.9;
    v.breaks_rules = 0.9;
    g.settle(2, &v).unwrap();
    assert_eq!(g.stats.hesitations, 1);
    assert_eq!(g.stats.cancelled, 1);
}

#[test]
fn fail_marks_the_event_and_counts_the_error() {
    let mut g = room();
    g.submit("Ana", "x").unwrap();
    let e = g.fail(1, "401 boom").unwrap();
    assert_eq!(
        e.outcome,
        Some(Outcome::Failed {
            reason: "401 boom".into()
        })
    );
    assert_eq!(g.stats.referee_errors, 1);
    assert_eq!(g.stats.moves_judged, 0);
}

#[test]
fn submit_is_refused_outside_a_round_or_for_unknown_or_downed_players() {
    let mut g = Game::new(Policy::default());
    g.join("Ana").unwrap();
    assert!(g.submit("Ana", "x").is_err(), "lobby");
    g.start_round(Instant::now());
    assert!(g.submit("Zed", "x").is_err(), "not joined");
    assert!(g.submit("Ana", "   ").is_err(), "empty");
    g.player_mut("Ana").unwrap().hp = 0;
    assert!(g.submit("Ana", "x").is_err(), "down");
    g.end_round();
    assert_eq!(g.phase, Phase::Ended);
}

#[test]
fn join_validates_names_and_rejoin_is_idempotent() {
    let mut g = Game::new(Policy::default());
    assert_eq!(g.join("  Ana ").unwrap(), "Ana");
    assert_eq!(g.join("ana").unwrap(), "Ana", "case-insensitive rejoin");
    assert_eq!(g.players.len(), 1);
    assert!(g.join("everyone").is_err());
    assert!(g.join("Nobody").is_err());
    assert!(g.join("").is_err());
    assert!(g.join(&"x".repeat(21)).is_err());
    assert!(g.join("a\"b").is_err());
}

#[test]
fn start_round_resets_players_and_log_but_not_stats() {
    let mut g = room();
    g.submit("Ana", "x").unwrap();
    g.settle(1, &verdict(Kind::Attack, player("Bob"), 4.0))
        .unwrap();
    g.end_round();
    let now = Instant::now();
    g.start_round(now);
    assert_eq!(g.round, 2);
    assert!(g.events.is_empty());
    assert!(
        g.players
            .iter()
            .all(|p| p.hp == 10 && p.gold == 5 && p.moves_judged == 0)
    );
    assert_eq!(g.stats.moves_judged, 1);
    assert_eq!(g.seconds_left(now), 60);
    assert_eq!(g.seconds_left(now + Duration::from_millis(500)), 60);
    assert_eq!(g.seconds_left(now + Duration::from_millis(1500)), 59);
    assert!(!g.round_expired(now));
    assert!(g.round_expired(now + Duration::from_secs(61)));
    assert_eq!(g.seconds_left(now + Duration::from_secs(61)), 0);
}

#[test]
fn leaderboard_orders_by_hp_then_gold_then_moves() {
    let mut g = room();
    g.player_mut("Ana").unwrap().hp = 4;
    g.player_mut("Bob").unwrap().hp = 9;
    g.player_mut("Cid").unwrap().hp = 9;
    g.player_mut("Cid").unwrap().gold = 7;
    let names: Vec<_> = g.leaderboard().into_iter().map(|r| r.name).collect();
    assert_eq!(names, ["Cid", "Bob", "Ana"]);
    assert_eq!(g.leaderboard()[0].rank, 1);
}
