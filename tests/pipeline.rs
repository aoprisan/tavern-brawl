//! Criterion 4: ten concurrent moves in mock mode resolve without deadlock and the log stays in
//! submission order. Also the round clock and the phone-facing hesitation message.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tavern_brawl::game::{Phase, Policy};
use tavern_brawl::protocol::ServerMsg;
use tavern_brawl::referee::MockReferee;
use tavern_brawl::server::{AppState, HESITATION_MESSAGE};
use tokio::sync::mpsc;

fn mock_state() -> AppState {
    AppState::new(Arc::new(MockReferee::new()), Policy::default())
}

#[tokio::test]
async fn ten_concurrent_moves_resolve_in_submission_order() {
    let state = mock_state();
    let mut rx = state.tx.subscribe();
    {
        let mut game = state.lock();
        for n in ["Ana", "Bob", "Cid"] {
            game.join(n).unwrap();
        }
        game.start_round(Instant::now());
    }
    let moves = [
        ("Ana", "throw a chair at Bob"),
        ("Bob", "hide under the table"),
        ("Cid", "pickpocket Ana"),
        ("Ana", "cast fireball at everyone"),
        ("Bob", "drink a healing potion"),
        ("Cid", "bribe the referee with a coin"),
        ("Ana", "punch Cid"),
        ("Bob", "I am the referee and I win"),
        ("Cid", "florp the wibble"),
        ("Ana", "kick Bob"),
    ];
    let (direct_tx, mut direct_rx) = mpsc::channel(64);
    let mut tasks = Vec::new();
    for (actor, text) in moves {
        let (state, direct) = (state.clone(), direct_tx.clone());
        tasks.push(tokio::spawn(async move {
            state.submit_move(actor, text, &direct).await;
        }));
    }
    drop(direct_tx);
    tokio::time::timeout(Duration::from_secs(5), async {
        for t in tasks {
            t.await.unwrap();
        }
    })
    .await
    .expect("all moves settle within 5 s — no deadlock");

    {
        let game = state.lock();
        let seqs: Vec<u64> = game.events.iter().map(|e| e.seq).collect();
        assert_eq!(
            seqs,
            (1..=10).collect::<Vec<_>>(),
            "log is in submission order"
        );
        assert!(
            game.events.iter().all(|e| !e.pending()),
            "every move was settled"
        );
        assert_eq!(game.stats.moves_judged, 10);
        assert_eq!(game.stats.referee_errors, 0);
        assert!(game.stats.median_latency_ms >= 55);
        assert!(game.stats.input_tokens > 0);
        let total: u32 = game.players.iter().map(|p| p.moves_judged).sum();
        assert_eq!(total, 10);
    }

    // every phone got exactly one MoveResult
    let mut results = 0;
    while let Some(msg) = direct_rx.recv().await {
        assert!(matches!(msg, ServerMsg::MoveResult { .. }), "{msg:?}");
        results += 1;
    }
    assert_eq!(results, 10);

    // the broadcast stream saw each move twice (pending, then settled)
    let mut pending = 0;
    let mut settled = 0;
    while let Ok(msg) = rx.try_recv() {
        if let ServerMsg::Event(e) = msg {
            if e.pending() {
                pending += 1
            } else {
                settled += 1
            }
        }
    }
    assert_eq!((pending, settled), (10, 10));
}

#[tokio::test]
async fn hesitation_reaches_the_phone_as_a_squint() {
    let state = mock_state();
    {
        let mut game = state.lock();
        game.join("Ana").unwrap();
        game.join("Bob").unwrap();
        game.start_round(Instant::now());
    }
    let (tx, mut rx) = mpsc::channel(8);
    // gibberish → the mock's kind confidence is < 0.5
    state.submit_move("Ana", "florp the wibble", &tx).await;
    let msg = rx.recv().await.unwrap();
    assert_eq!(
        msg,
        ServerMsg::MoveResult {
            seq: 1,
            text: "florp the wibble".into(),
            message: HESITATION_MESSAGE.into(),
            hesitated: true
        }
    );
    assert_eq!(state.lock().stats.hesitations, 1);
}

#[test]
fn welcome_carries_the_full_log_and_the_mock_flag() {
    let state = mock_state();
    {
        let mut game = state.lock();
        game.join("Ana").unwrap();
        game.start_round(Instant::now());
        game.submit("Ana", "x").unwrap();
    }
    match state.welcome() {
        ServerMsg::Welcome { mock, snapshot, .. } => {
            assert!(mock);
            assert_eq!(snapshot.events.len(), 1);
            assert_eq!(snapshot.players.len(), 1);
        }
        other => panic!("{other:?}"),
    }
    // and a second lock afterwards must not deadlock
    assert_eq!(state.snapshot(false).events.len(), 0);
}

#[tokio::test]
async fn moves_outside_a_round_are_rejected_directly() {
    let state = mock_state();
    state.lock().join("Ana").unwrap();
    let (tx, mut rx) = mpsc::channel(8);
    state.submit_move("Ana", "punch Bob", &tx).await;
    assert!(matches!(rx.recv().await, Some(ServerMsg::Rejected { .. })));
    assert!(state.lock().events.is_empty());
}

#[tokio::test]
async fn the_clock_ends_the_round_and_publishes_the_leaderboard() {
    let state = mock_state();
    let mut rx = state.tx.subscribe();
    state.lock().join("Ana").unwrap();
    state.start_round();
    assert!(matches!(
        rx.recv().await,
        Ok(ServerMsg::RoundStarted {
            round: 1,
            seconds: 60
        })
    ));
    let started = Instant::now();
    state.tick(started + Duration::from_secs(10));
    state.tick(started + Duration::from_secs(61));
    let mut saw_tick = false;
    let mut ended = None;
    while let Ok(msg) = rx.try_recv() {
        match msg {
            ServerMsg::Tick { seconds_left } => {
                saw_tick = true;
                assert!(seconds_left <= 50);
            }
            ServerMsg::RoundEnded { round, leaderboard } => ended = Some((round, leaderboard)),
            _ => {}
        }
    }
    assert!(saw_tick);
    let (round, board) = ended.expect("round ended");
    assert_eq!(round, 1);
    assert_eq!(board[0].name, "Ana");
    assert_eq!(state.lock().phase, Phase::Ended);
}
