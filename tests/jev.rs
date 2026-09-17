//! `JevReferee` against a fake TypeSafe API served by axum on localhost: the SDK's request goes
//! over real HTTP, and we check both the happy path (answers → `Verdict`) and that API errors map
//! to `RefereeError` instead of panicking. No real key, no network.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use serde_json::{Value, json};
use tavern_brawl::game::{Kind, Target};
use tavern_brawl::referee::{JevReferee, MoveState, Referee, RefereeError};
use tokio::sync::Mutex;
use typesafe::{Client, RetryPolicy};

#[derive(Clone)]
struct Fake {
    status: StatusCode,
    body: Value,
    hits: Arc<AtomicUsize>,
    last_request: Arc<Mutex<Option<(HeaderMap, Value)>>>,
}

async fn systemone(State(f): State<Fake>, headers: HeaderMap, body: Bytes) -> impl IntoResponse {
    f.hits.fetch_add(1, Ordering::SeqCst);
    *f.last_request.lock().await = Some((headers, serde_json::from_slice(&body).unwrap()));
    (
        f.status,
        [
            ("content-type", "application/json"),
            ("x-typesafe-request-id", "req_test_123"),
        ],
        f.body.to_string(),
    )
}

async fn fake_api(status: StatusCode, body: Value) -> (Fake, SocketAddr) {
    let fake = Fake {
        status,
        body,
        hits: Arc::new(AtomicUsize::new(0)),
        last_request: Arc::new(Mutex::new(None)),
    };
    let app = Router::new()
        .route("/v1/systemone", post(systemone))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (fake, addr)
}

fn referee_for(addr: SocketAddr, retries: u32) -> JevReferee {
    let client = Client::builder()
        .api_key("test-key")
        .base_url(format!("http://{addr}"))
        .timeout(Duration::from_secs(2))
        .retry(
            RetryPolicy::default()
                .max_retries(retries)
                .backoff(Duration::from_millis(10), Duration::from_millis(20)),
        )
        .build()
        .unwrap();
    JevReferee::with_client(client)
}

fn state() -> MoveState {
    MoveState {
        arena_rules: tavern_brawl::game::ARENA_RULES,
        round: 1,
        actor: "Ana".into(),
        move_text: "throw a chair at Bob".into(),
        live_players: vec![],
        recent_events: vec!["Bob: \"hide\" -> defend nobody, force 0".into()],
    }
}

fn players() -> Vec<String> {
    vec!["Ana".into(), "Bob".into()]
}

fn good_answers() -> Value {
    json!({
        "model": "jev-latest",
        "usage": {"input_tokens": 812, "output_tokens": 0},
        "answers": {
            "kind": {"type": "choice", "choice": "attack", "confidence": 0.91,
                     "probabilities": {"attack": 0.93, "defend": 0.01, "steal": 0.01, "heal": 0.01, "bribe": 0.01, "nonsense": 0.03}},
            "target": {"type": "choice", "choice": "Bob", "confidence": 0.88,
                       "probabilities": {"Ana": 0.02, "Bob": 0.9, "everyone": 0.04, "nobody": 0.04}},
            "force": {"type": "score", "score": 2.3, "confidence": 0.6,
                      "legend": {"0": "none", "1": "weak", "2": "solid", "3": "heavy", "4": "devastating"},
                      "probabilities": {"0": 0.0, "1": 0.1, "2": 0.5, "3": 0.4, "4": 0.0}},
            "breaks_rules": {"type": "noul", "noul": 0.03}
        }
    })
}

#[tokio::test]
async fn happy_path_maps_answers_to_a_verdict_and_sends_our_questions() {
    let (fake, addr) = fake_api(StatusCode::OK, good_answers()).await;
    let referee = referee_for(addr, 0);
    assert!(!referee.is_mock());
    let v = referee.judge(&state(), &players()).await.unwrap();
    assert_eq!(v.kind, Kind::Attack);
    assert!((v.kind_confidence - 0.91).abs() < 1e-6);
    assert_eq!(v.target, Target::Player("Bob".into()));
    assert!((v.force - 2.3).abs() < 1e-6);
    assert!((v.breaks_rules - 0.03).abs() < 1e-6);
    assert_eq!(v.input_tokens, 812);
    assert!(v.latency > Duration::ZERO);

    let (headers, body) = fake.last_request.lock().await.clone().unwrap();
    assert_eq!(headers["authorization"], "Bearer test-key");
    assert_eq!(body["model"], "jev-latest");
    assert_eq!(body["state"]["actor"], "Ana");
    assert_eq!(body["state"]["move_text"], "throw a chair at Bob");
    assert_eq!(
        body["state"]["arena_rules"],
        tavern_brawl::game::ARENA_RULES
    );
    let q = body["questions"].as_object().unwrap();
    // (serde_json::Value sorts keys; wire order is asserted in tests/referee.rs)
    assert_eq!(
        q.keys().collect::<Vec<_>>(),
        ["breaks_rules", "force", "kind", "target"]
    );
    assert_eq!(
        q["target"]["criteria"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["Ana", "Bob", "everyone", "nobody"]
    );
    assert_eq!(fake.hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn everyone_and_nobody_labels_map_back_to_targets() {
    let mut body = good_answers();
    body["answers"]["target"]["choice"] = json!("everyone");
    let (_, addr) = fake_api(StatusCode::OK, body).await;
    let v = referee_for(addr, 0)
        .judge(&state(), &players())
        .await
        .unwrap();
    assert_eq!(v.target, Target::Everyone);
}

#[tokio::test]
async fn unauthorized_is_an_auth_error() {
    let (fake, addr) = fake_api(
        StatusCode::UNAUTHORIZED,
        json!({"detail": "Invalid API key"}),
    )
    .await;
    let err = referee_for(addr, 2)
        .judge(&state(), &players())
        .await
        .unwrap_err();
    assert!(matches!(err, RefereeError::Auth(_)), "{err:?}");
    assert!(err.to_string().contains("401"));
    assert!(err.to_string().contains("Invalid API key"));
    assert_eq!(fake.hits.load(Ordering::SeqCst), 1, "401 is not retried");
}

#[tokio::test]
async fn unprocessable_is_an_invalid_request() {
    let body = json!({"detail": [{"loc": ["body", "questions", "force", "criteria"],
                                  "msg": "List should have at least 2 items", "type": "too_short"}]});
    let (_, addr) = fake_api(StatusCode::UNPROCESSABLE_ENTITY, body).await;
    let err = referee_for(addr, 2)
        .judge(&state(), &players())
        .await
        .unwrap_err();
    assert!(matches!(err, RefereeError::InvalidRequest(_)), "{err:?}");
    assert_eq!(err.code(), "invalid_request");
}

#[tokio::test]
async fn rate_limit_after_retries_is_overloaded() {
    let (fake, addr) = fake_api(
        StatusCode::TOO_MANY_REQUESTS,
        json!({"detail": "slow down"}),
    )
    .await;
    let err = referee_for(addr, 2)
        .judge(&state(), &players())
        .await
        .unwrap_err();
    assert!(matches!(err, RefereeError::Overloaded(_)), "{err:?}");
    assert_eq!(
        fake.hits.load(Ordering::SeqCst),
        3,
        "first attempt + 2 retries"
    );
}

#[tokio::test]
async fn overloaded_529_after_retries_is_overloaded() {
    let (fake, addr) = fake_api(
        StatusCode::from_u16(529).unwrap(),
        json!({"detail": "Overloaded"}),
    )
    .await;
    let err = referee_for(addr, 1)
        .judge(&state(), &players())
        .await
        .unwrap_err();
    assert!(matches!(err, RefereeError::Overloaded(_)), "{err:?}");
    assert_eq!(fake.hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_missing_answer_is_a_bad_answer_not_a_panic() {
    let mut body = good_answers();
    body["answers"].as_object_mut().unwrap().remove("force");
    let (_, addr) = fake_api(StatusCode::OK, body).await;
    let err = referee_for(addr, 0)
        .judge(&state(), &players())
        .await
        .unwrap_err();
    assert!(matches!(err, RefereeError::BadAnswer(_)), "{err:?}");
    assert!(err.to_string().contains("force"));
}

#[tokio::test]
async fn an_unknown_kind_label_is_a_bad_answer() {
    let mut body = good_answers();
    body["answers"]["kind"]["choice"] = json!("tickle");
    let (_, addr) = fake_api(StatusCode::OK, body).await;
    let err = referee_for(addr, 0)
        .judge(&state(), &players())
        .await
        .unwrap_err();
    assert!(matches!(err, RefereeError::BadAnswer(_)), "{err:?}");
}

#[tokio::test]
async fn a_malformed_2xx_body_is_a_bad_answer() {
    let (_, addr) = fake_api(
        StatusCode::OK,
        json!({"answers": {"kind": {"type": "choice"}}}),
    )
    .await;
    let err = referee_for(addr, 0)
        .judge(&state(), &players())
        .await
        .unwrap_err();
    assert!(matches!(err, RefereeError::BadAnswer(_)), "{err:?}");
}

#[tokio::test]
async fn an_unreachable_api_is_unreachable() {
    // nothing listens here
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let err = referee_for(addr, 0)
        .judge(&state(), &players())
        .await
        .unwrap_err();
    assert!(matches!(err, RefereeError::Unreachable(_)), "{err:?}");
}
