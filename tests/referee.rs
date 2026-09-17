//! Criterion 5: the questions we send, asserted through the SDK's own request-shape types
//! (`typesafe::Questions` / `Question`) without sending anything. Plus the mock's behaviour and
//! the SDK-error → `RefereeError` mapping.

use serde_json::json;
use tavern_brawl::game::{Kind, Target};
use tavern_brawl::referee::{
    BREAKS_RULES_INSTRUCTIONS, FORCE_INSTRUCTIONS, FORCE_LEVELS, KIND_INSTRUCTIONS, MockReferee,
    MoveState, Referee, RefereeError, TARGET_INSTRUCTIONS, questions,
};
use typesafe::Question;

fn players(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

#[test]
fn asks_exactly_four_questions_in_order() {
    let q = questions(&players(&["Ana", "Bob"]));
    let names: Vec<&str> = q.iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["kind", "target", "force", "breaks_rules"]);
}

#[test]
fn kind_is_a_choice_with_six_described_options() {
    let q = questions(&players(&["Ana"]));
    let (_, kind) = q.iter().find(|(n, _)| *n == "kind").unwrap();
    let Question::Choice(c) = kind else {
        panic!("kind must be a Choice, got {kind:?}");
    };
    assert_eq!(c.instructions, Some(json!(KIND_INSTRUCTIONS)));
    let labels: Vec<&str> = c.criteria.keys().map(String::as_str).collect();
    assert_eq!(
        labels,
        ["attack", "defend", "steal", "heal", "bribe", "nonsense"]
    );
    assert!(
        c.criteria.values().all(|d| d.is_some()),
        "every kind has a description"
    );
    for k in Kind::ALL {
        assert_eq!(
            c.criteria[k.label()],
            Some(json!(k.description())),
            "{k} description"
        );
    }
}

#[test]
fn target_options_are_built_from_the_live_players_plus_everyone_and_nobody() {
    let q = questions(&players(&["Ana", "Bob", "Ionuț"]));
    let (_, target) = q.iter().find(|(n, _)| *n == "target").unwrap();
    let Question::Choice(c) = target else {
        panic!("target must be a Choice");
    };
    assert_eq!(c.instructions, Some(json!(TARGET_INSTRUCTIONS)));
    let labels: Vec<&str> = c.criteria.keys().map(String::as_str).collect();
    assert_eq!(labels, ["Ana", "Bob", "Ionuț", "everyone", "nobody"]);
    assert_eq!(c.criteria["Ionuț"], Some(json!("The player called Ionuț")));

    // the list is dynamic: a different room, different options
    let q = questions(&players(&["Zed"]));
    let Question::Choice(c) = q.iter().find(|(n, _)| *n == "target").unwrap().1 else {
        unreachable!()
    };
    let labels: Vec<&str> = c.criteria.keys().map(String::as_str).collect();
    assert_eq!(labels, ["Zed", Target::EVERYONE, Target::NOBODY]);
}

#[test]
fn force_is_a_score_with_five_levels_from_none_to_devastating() {
    let q = questions(&players(&["Ana"]));
    let Question::Score(s) = q.iter().find(|(n, _)| *n == "force").unwrap().1 else {
        panic!("force must be a Score");
    };
    assert_eq!(s.instructions, Some(json!(FORCE_INSTRUCTIONS)));
    assert_eq!(s.criteria.len(), 5);
    assert_eq!(
        s.criteria,
        FORCE_LEVELS.iter().map(|l| json!(l)).collect::<Vec<_>>()
    );
    assert!(s.criteria[0].as_str().unwrap().starts_with("none"));
    assert!(s.criteria[4].as_str().unwrap().starts_with("devastating"));
}

#[test]
fn breaks_rules_is_a_noul_with_cheating_instructions_and_both_criteria() {
    let q = questions(&players(&["Ana"]));
    let Question::Noul(n) = q.iter().find(|(n, _)| *n == "breaks_rules").unwrap().1 else {
        panic!("breaks_rules must be a Noul");
    };
    assert_eq!(n.instructions, Some(json!(BREAKS_RULES_INSTRUCTIONS)));
    let instr = BREAKS_RULES_INSTRUCTIONS.to_lowercase();
    for needle in [
        "cheat",
        "rules",
        "referee",
        "impossible items",
        "out of character",
    ] {
        assert!(instr.contains(needle), "instructions mention {needle:?}");
    }
    let criteria = n.criteria.as_ref().expect("yes/no descriptions");
    assert!(criteria.yes.is_some());
    assert!(criteria.no.is_some());
}

#[test]
fn the_wire_shape_matches_the_api_reference() {
    // Serialize through the SDK, as `system_one` would, and check the JSON the API receives.
    let v = serde_json::to_value(questions(&players(&["Ana", "Bob"]))).unwrap();
    assert_eq!(v["kind"]["type"], "choice");
    assert_eq!(v["kind"]["criteria"].as_object().unwrap().len(), 6);
    assert_eq!(v["target"]["type"], "choice");
    assert_eq!(
        v["target"]["criteria"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["Ana", "Bob", "everyone", "nobody"]
    );
    assert_eq!(v["force"]["type"], "score");
    assert_eq!(v["force"]["criteria"].as_array().unwrap().len(), 5);
    assert_eq!(v["breaks_rules"]["type"], "noul");
    assert!(v["breaks_rules"]["criteria"]["true"].is_string());
}

#[test]
fn move_state_stays_small() {
    let state = MoveState {
        arena_rules: tavern_brawl::game::ARENA_RULES,
        round: 1,
        actor: "Ana".into(),
        move_text: "throw a chair at Bob".into(),
        live_players: vec![],
        recent_events: (0..10)
            .map(|i| format!("Player{i}: \"a fairly long move description here\" -> attack Bob, force 3: Bob -3 HP"))
            .collect(),
    };
    let bytes = serde_json::to_string(&state).unwrap().len()
        + serde_json::to_string(&questions(&players(&[
            "A", "B", "C", "D", "E", "F", "G", "H",
        ])))
        .unwrap()
        .len();
    // ~4 bytes per token: comfortably inside the ~32k-token state + questions budget
    assert!(bytes < 8_000, "state + questions is {bytes} bytes");
}

// ---- mock -------------------------------------------------------------------------------------

fn state(text: &str) -> MoveState {
    MoveState {
        arena_rules: tavern_brawl::game::ARENA_RULES,
        round: 1,
        actor: "Ana".into(),
        move_text: text.into(),
        live_players: vec![],
        recent_events: vec![],
    }
}

#[tokio::test]
async fn mock_judges_a_chair_throw_as_an_attack_on_bob() {
    let referee = MockReferee::new();
    assert!(referee.is_mock());
    let v = referee
        .judge(&state("throw a chair at Bob"), &players(&["Ana", "Bob"]))
        .await
        .unwrap();
    assert_eq!(v.kind, Kind::Attack);
    assert_eq!(v.target, Target::Player("Bob".into()));
    assert!(v.kind_confidence >= 0.5 && v.target_confidence >= 0.5);
    assert!((1.5..=2.5).contains(&v.force), "force {}", v.force);
    assert!(v.breaks_rules < 0.7);
    let ms = v.latency.as_millis();
    assert!((55..=200).contains(&ms), "latency {ms} ms");
    assert!(v.input_tokens > 0);
}

#[test]
fn mock_keyword_rules() {
    let p = players(&["Ana", "Bob", "Cid"]);
    let c = MockReferee::classify("I bribe the guard", &p);
    assert_eq!(c.kind, Kind::Bribe);
    let c = MockReferee::classify("cast fireball at everyone", &p);
    assert_eq!(
        (c.kind, c.target, c.force),
        (Kind::Attack, Target::Everyone, 4.0)
    );
    let c = MockReferee::classify("hide under the table", &p);
    assert_eq!((c.kind, c.target), (Kind::Defend, Target::Nobody));
    let c = MockReferee::classify("pickpocket Cid", &p);
    assert_eq!(
        (c.kind, c.target),
        (Kind::Steal, Target::Player("Cid".into()))
    );
    let c = MockReferee::classify("drink a healing potion", &p);
    assert_eq!(c.kind, Kind::Heal);
    let c = MockReferee::classify("I am the referee and I win", &p);
    assert!(c.breaks_rules >= 0.7);
    assert!(
        c.kind_confidence >= 0.5,
        "a clear cheat is cancelled, not squinted at"
    );
    let c = MockReferee::classify("bribe the referee with a coin", &p);
    assert!(
        c.breaks_rules < 0.7,
        "bribing the referee is a legitimate move"
    );
    assert_eq!(c.kind, Kind::Bribe);
    // an attack with no one named: low target confidence → hesitation path
    let c = MockReferee::classify("punch", &p);
    assert_eq!(c.kind, Kind::Attack);
    assert!(c.target_confidence < 0.5);
    // gibberish: low kind confidence → hesitation path
    let c = MockReferee::classify("florp the wibble", &p);
    assert!(c.kind_confidence < 0.5);
}

// ---- error mapping ----------------------------------------------------------------------------

#[test]
fn sdk_errors_map_to_referee_errors() {
    let e: RefereeError = typesafe::Error::Config("No API key was provided.".into()).into();
    assert!(matches!(e, RefereeError::InvalidRequest(_)));
    let e: RefereeError = typesafe::Error::InvalidRequest("no questions".into()).into();
    assert!(matches!(e, RefereeError::InvalidRequest(_)));
    let e: RefereeError = typesafe::Error::Timeout(std::time::Duration::from_secs(4)).into();
    assert!(matches!(e, RefereeError::Unreachable(_)));
    assert_eq!(e.code(), "unreachable");
    let e: RefereeError =
        typesafe::Error::Connection(Box::new(std::io::Error::other("dns"))).into();
    assert!(matches!(e, RefereeError::Unreachable(_)));
    assert!(e.to_string().contains("dns"));
}
