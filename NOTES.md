# NOTES — SDK surface used by tavern-brawl

SDK: `typesafe-ai-sdk = "0.1"` (crates.io; library name `typesafe`, MSRV 1.88). Source read:
`README.md`, `src/lib.rs`, `src/question.rs`, `src/client.rs`, `src/response.rs`, `src/error.rs`,
`src/retry.rs`, `examples/triage.rs`. Nothing needed is missing; no workarounds.

## Client

- `typesafe::Client::from_env()` — key from **`TYPESAFE_API_KEY`** (SDK's name; agreed to use it
  instead of the `TYPESAFE_AI_API_KEY` from the brief), base URL `TYPESAFE_BASE_URL`, model
  `TYPESAFE_DEFAULT_MODEL` (default `jev-latest`). Returns `Error::Config` when the key is missing → we
  turn that into `RefereeError` before serving and print a clear message.
- `Client::builder().timeout(..).retry(..).build()` — we use the builder to tighten the per-attempt
  timeout (4 s) and the retry policy for a 60-second round:
  `RetryPolicy::default().max_retries(2).backoff(200ms, 1s).budget(Some(8s))`.
  Default policy retries 408/429/5xx (incl. 529), connection errors, timeouts; honours `Retry-After`.
- `client.system_one(state, questions).await -> typesafe::Result<SystemOneResponse>`.
  `state` is any `Serialize` → our `MoveState` struct. One call per move, four questions.

## Questions (`typesafe::question`)

All builders are pure data (`Serialize`, `PartialEq`, public fields) so requests can be inspected
without sending — criterion 5 needs no SDK seam. The demo's seam is `referee::questions(players)`,
a pure `fn(&[PlayerName]) -> Questions`; `Questions::iter()` yields `(&str, &Question)` and
`Question::{Noul, Choice, Score}` expose `instructions` / `criteria`.

| name           | builder                                                                | fields we read back            |
| -------------- | ---------------------------------------------------------------------- | ------------------------------ |
| `kind`         | `Choice::new(instr).option(label, desc)` × 6 (attack…nonsense)         | `ChoiceAnswer{choice, confidence}` |
| `target`       | `Choice::new(instr)` + `.option(name, "…")` per live player + `everyone` + `nobody` | `ChoiceAnswer{choice, confidence}` |
| `force`        | `Score::new(instr, [5 level strings])` → levels 0–4                    | `ScoreAnswer{score, confidence}`   |
| `breaks_rules` | `Noul::new(instr).when_true(..).when_false(..)`                         | `NoulAnswer{noul}`             |

`Questions::new().with(name, q)` keeps insertion order on the wire.

## Response → `Verdict`

`SystemOneResponse`:
- `res.choice("kind")   -> Option<&ChoiceAnswer>`  → `Verdict.kind` (parsed with `ChoiceAnswer::parse::<Kind>()`, `Kind: FromStr`), `Verdict.kind_confidence = confidence as f32`
- `res.choice("target") -> Option<&ChoiceAnswer>`  → `Verdict.target` (`everyone` / `nobody` / player name), `target_confidence`
- `res.score("force")   -> Option<&ScoreAnswer>`   → `Verdict.force = score as f32` (probability-weighted level 0–4; `resolve()` rounds it), `force_confidence`
- `res.noul("breaks_rules") -> Option<&NoulAnswer>` → `Verdict.breaks_rules = noul as f32` (P(yes))
- `res.usage.input_tokens: Option<u64>` → `Verdict.input_tokens` (0 if unreported; spend counter uses this)
- `res.meta.attempts`, `res.request_id()` → tracing only
- `Verdict.latency` is measured by us with `Instant` around the `.await` (wall time incl. retries).
`None` from any accessor (question unanswered / unknown answer type skipped) → `RefereeError::BadAnswer`.

## Errors → `RefereeError` → `ServerMsg::RefereeDown { reason }`

`typesafe::Error` (non_exhaustive) mapping:
- `Api(e)` by `e.kind: ApiErrorKind` — `Authentication`/`PermissionDenied` (401/403) → `Auth`;
  `BadRequest`/`UnprocessableEntity` (400/422) → `InvalidRequest` (includes flattened FastAPI message);
  `RateLimit`/`InternalServer` (429, 5xx, 529 — only surfaces after retries) → `Overloaded`
  (with `e.retry_after()`); other → `Other`.
- `Connection(_)`, `Timeout(_)` → `Unreachable`
- `InvalidRequest(_)`, `Config(_)` → `InvalidRequest` (our bug / setup)
- `ResponseValidation(e)` → `BadAnswer` (`e.field_path`)
- wildcard arm for future variants → `Other`.

## Not used

`Question::Raw`, `extra_body`, `blocking` feature, `models()` (the `ping` bin uses a real
`system_one` call instead, so it verifies the key *and* the question path).
