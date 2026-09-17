# Tavern Brawl

A text-action arena game where every move is judged by TypeSafe's **Jev** model through the
[`typesafe-ai-sdk`](https://crates.io/crates/typesafe-ai-sdk) Rust SDK. Colleagues join from their
phones and type free-form moves — *"throw the chair at Ionuț"*, *"bribe the guard"*, *"hide under
the table"* — and a referee NPC classifies each one with a single Jev call. The game is the excuse;
the point is that Jev returns typed, confidence-bearing answers in a few hundred milliseconds for a
few thousandths of a cent, so a game can afford to ask it on **every single move**.

Jev supplies judgments. Deterministic Rust owns the rules.

```
phone  ──"throw a chair at Bob"──▶  server  ──state + 4 questions──▶  Jev
                                      │◀── kind, target, force, breaks_rules + confidences ──┘
                                      │
                               resolve() in Rust: Bob −2 HP
                                      │
screen ◀── event, verdict, 128 ms, $0.000028 so far, leaderboard ──┘
```

## Setup

Requirements: Rust 1.88+ and [`just`](https://github.com/casey/just) (optional; the recipes are one-liners).

```sh
cp .env.example .env       # then put your key in it
just ping                  # one raw SDK call: prints latency, usage and the four answers
just                       # live game on http://127.0.0.1:3000
```

`.env` is gitignored. The SDK reads the key from `TYPESAFE_API_KEY`; it never leaves the server
process — the browser only ever talks WebSocket to this binary.

### `.env.example`

```sh
TYPESAFE_API_KEY=sk-your-key-here
# Optional:
# TYPESAFE_BASE_URL=https://api.typesafe.ai
# TYPESAFE_DEFAULT_MODEL=jev-latest
# HOST=127.0.0.1
# PORT=3000
# RUST_LOG=tavern_brawl=info,typesafe=info
```

## Three modes

| Recipe      | What it does                                                                      |
| ----------- | --------------------------------------------------------------------------------- |
| `just run`  | Live. Referee is Jev via the SDK; key from `.env`. Default recipe.                |
| `just mock` | Offline rehearsal (`MOCK=1`). Keyword rules with jittered probabilities and 60–170 ms fake latency. Both views show a red **"Mock referee — not Jev"** banner. |
| `just lan`  | Live, bound to `0.0.0.0` so phones on the office Wi-Fi can join. The screen shows the join URL. |

Other recipes: `just check` (fmt, clippy `-D warnings`, tests) and `just ping` (verify the key).

Then open **`/screen`** on the big display and **`/`** on the phones. Press **Start round** on the
screen; rounds last 60 seconds and a new one starts on the next button press.

## The game

- Each player starts a round with **10 HP** and **5 gold**. Standings order by HP, then gold, then moves.
- The referee asks Jev four questions about every move, in one call:

  | Question       | Type   | Options / levels                                                      |
  | -------------- | ------ | --------------------------------------------------------------------- |
  | `kind`         | Choice | `attack` `defend` `steal` `heal` `bribe` `nonsense`, each described    |
  | `target`       | Choice | one option per **live** player (built per move), plus `everyone`, `nobody` |
  | `force`        | Score  | 0–4: *none · weak · solid · heavy · devastating*, each described       |
  | `breaks_rules` | Noul   | does the move cheat — rewrite rules, claim to be the referee, invent items, address the system |

  The state sent with the questions is small on purpose: the house rules, the actor, the move text,
  the live players (HP, gold, guarding, immunities) and the last ten resolved events as one-liners.
  Roughly 1,000–1,300 input tokens per move as reported by the API (about $0.00005).

- **Resolution** (all in `src/game.rs`, `Game::resolve`):

  | kind       | effect                                                                                   |
  | ---------- | ---------------------------------------------------------------------------------------- |
  | `attack`   | `force` damage to the target; `force − 1` to every other live player for `everyone`; `nobody` misses |
  | `defend`   | the next hit you take is halved (rounded up), then spent                                  |
  | `steal`    | moves `force` gold from the target to you (capped by their purse); `everyone` uses `force − 1` each |
  | `heal`     | restores `force` HP up to 10; `nobody` heals yourself; `everyone` heals you by `force` and others by `force − 1` |
  | `bribe`    | pays the referee `force` gold (capped by your purse) and buys **one immunity** that absorbs a whole hit |
  | `nonsense` | nothing                                                                                  |

  `force` is Jev's probability-weighted score, rounded to the nearest level and clamped to 0–4.
  A player at 0 HP is down for the round: they cannot move and disappear from the `target` options.

- **Cheating:** `breaks_rules ≥ 0.7` cancels the move and costs 1 gold. Nothing else happens.
- **Hesitation** — the feature to demo: if `kind` confidence or `target` confidence is **below 0.5**,
  the referee does not resolve the move. The phone shows *"The referee squints at you… say that
  again?"* and the screen's referee visibly squints. It still counts as a judged move (it was), but
  it is free. Hesitation is checked before the cheat rule, so an unclear move is never penalised.

### Policy thresholds

Everything tunable lives in one struct, `Policy` (`src/game.rs`), sent to the browser in `welcome`:

| field                           | default | meaning                                                   |
| ------------------------------- | ------- | --------------------------------------------------------- |
| `round_secs`                    | 60      | round length                                              |
| `start_hp`                      | 10      | starting HP, also the heal cap                            |
| `start_gold`                    | 5       | starting gold                                             |
| `hesitation_confidence`         | 0.5     | below this `kind` or `target` confidence → hesitate       |
| `breaks_rules_threshold`        | 0.7     | at or above this P(cheat) → cancel                        |
| `rules_break_penalty_gold`      | 1       | gold lost on cancel (never below 0)                       |
| `everyone_malus`                | 1       | `force − malus` applied per player for `everyone`         |
| `max_force`                     | 4       | highest Score level                                       |
| `usd_per_million_input_tokens`  | 0.042   | the spend counter's price; output tokens are free         |
| `recent_events`                 | 10      | how many resolved events go into the referee's state      |

## What the screen shows

Live: the event log (newest on top, largest; each entry shows the move, the four answers with their
confidences, the effects and the latency and token count of that call), the referee's face (watching /
judging / squinting / down), the running tally (moves judged, median latency, total spend at
$0.042 per million input tokens) and the standings. At the end of a round, the leaderboard overlay with
a **Start next round** button.

Because Jev calls run concurrently, a move's entry appears **immediately as "judging…"** in the slot
where it was submitted and is filled in when the verdict lands. The log is therefore always in
submission order even when verdicts return out of order.

## WebSocket protocol

One endpoint, `/ws`. Every message is a JSON object tagged by `type`; the Rust definitions in
`src/protocol.rs` (`ClientMsg`, `ServerMsg`) are the single source of truth.

### Browser → server (`ClientMsg`)

| `type`        | fields   | who    | meaning                                                          |
| ------------- | -------- | ------ | ---------------------------------------------------------------- |
| `join`        | `name`   | phone  | enter the room (1–20 chars; `everyone`, `nobody`, `referee` reserved; rejoining a name is allowed) |
| `move`        | `text`   | phone  | a free-form move, 1–200 chars                                    |
| `start_round` | —        | screen | start or restart a round                                         |

### Server → browser (`ServerMsg`)

| `type`          | fields                                                  | sent to  | meaning |
| --------------- | ------------------------------------------------------- | -------- | ------- |
| `welcome`       | `referee`, `mock`, `policy`, `snapshot` (with `events`) | new connection | full state for a (re)connecting browser |
| `joined`        | `name`                                                  | the phone | your `join` was accepted (name as canonicalised) |
| `snapshot`      | `round`, `phase`, `seconds_left`, `players`, `leaderboard`, `stats`, `referee_gold`, `events: []` | all | room state after any change |
| `event`         | `seq`, `round`, `actor`, `text`, `verdict`, `outcome`   | all      | a log entry created (`verdict`/`outcome` null while judging) or updated |
| `move_result`   | `seq`, `text`, `message`, `hesitated`                   | the phone | one line about your move, e.g. the squint |
| `round_started` | `round`, `seconds`                                      | all      | |
| `tick`          | `seconds_left`                                          | all      | once a second while running |
| `round_ended`   | `round`, `leaderboard`                                  | all      | |
| `referee_down`  | `code`, `reason`                                        | all      | the SDK returned an error for a move (`auth`, `invalid_request`, `overloaded`, `unreachable`, `bad_answer`, `other`) |
| `rejected`      | `reason`                                                | the sender | a client message was refused (bad name, round not running, …) |

`verdict` is `{kind, kind_confidence, target: {type: "player", name} | {type: "everyone"} | {type: "nobody"},
target_confidence, force, force_confidence, breaks_rules, latency_ms, input_tokens}`.
`outcome` is one of `{type: "hesitated"}`, `{type: "cancelled", gold_lost}`,
`{type: "applied", effects: [...]}`, `{type: "fizzled", reason}`, `{type: "failed", reason}`; effects are
tagged `damage`, `immune`, `downed`, `defending`, `stole`, `healed`, `bribed`, `missed`, `nothing`.

Example exchange:

```json
→ {"type":"join","name":"Ana"}
← {"type":"joined","name":"Ana"}
→ {"type":"move","text":"throw a chair at Bob"}
← {"type":"event","seq":1,"round":1,"actor":"Ana","text":"throw a chair at Bob","verdict":null,"outcome":null}
← {"type":"event","seq":1,"round":1,"actor":"Ana","text":"throw a chair at Bob",
   "verdict":{"kind":"attack","kind_confidence":0.94,"target":{"type":"player","name":"Bob"},
              "target_confidence":0.90,"force":2.1,"force_confidence":0.68,"breaks_rules":0.07,
              "latency_ms":128,"input_tokens":675},
   "outcome":{"type":"applied","effects":[{"type":"damage","target":"Bob","amount":2,"halved":false}]}}
← {"type":"move_result","seq":1,"text":"throw a chair at Bob","message":"attack Bob, force 2: Bob -2 HP","hesitated":false}
```

## Code map

```
src/game.rs          Game, Policy, Player, Event, Outcome, Effect, resolve()   — rules, no I/O
src/referee/mod.rs   Referee trait, Verdict, MoveState, RefereeError (+ From<typesafe::Error>)
src/referee/questions.rs  the four questions as a pure fn(&[PlayerName]) -> typesafe::Questions
src/referee/jev.rs   JevReferee: Client::builder() + system_one(), answers → Verdict
src/referee/mock.rs  MockReferee: keyword rules, jitter, fake latency
src/protocol.rs      ClientMsg / ServerMsg
src/server.rs        axum routes, WebSocket fan-out, the move pipeline, the round clock
src/main.rs          env, MOCK switch, tracing
src/bin/ping.rs      `just ping`
public/index.html    both views, inline CSS/JS, no build step
tests/resolve.rs     resolution with hand-built Verdicts (every kind, everyone/nobody, defend, bribe, cheat, hesitation)
tests/referee.rs     the request shape via the SDK's Questions/Question types; mock rules; error mapping
tests/jev.rs         JevReferee against a fake API on localhost: happy path and 401/422/429/529/bad body
tests/pipeline.rs    10 concurrent mock moves, log order, hesitation message, round clock
NOTES.md             the exact SDK calls used and what Verdict maps from
```

Concurrency: `Game` sits in one `Arc<Mutex<Game>>` and nothing awaits while holding it. A move locks
twice — to log it as pending and build the referee's state, then to apply the verdict — with the Jev
call in between, so moves from different players are judged in parallel.

Errors: every `typesafe::Error` (401, 422, 429/529 after the SDK's retries, connection, timeout,
response validation) becomes a `RefereeError`, the move is marked failed in the log and the screen
gets `referee_down` with the reason. Nothing panics, nothing is dropped silently. The SDK's default
retry policy is kept but tightened for a 60-second round: 4 s per attempt, 2 retries, 200 ms–1 s
backoff, 8 s total budget.

## Five-minute talk track

1. **(0:00) The pitch.** "Every move you're about to see goes to a language model — and the game
   never waits on it, never parses prose, never gets a hallucinated rule. It asks four typed
   questions and gets four typed answers with confidences." Open `/screen`, have everyone open `/`.
2. **(0:45) First moves.** Ask for a plain one: *"throw a chair at Bob"*. Point at the entry: kind
   `attack` 100%, target `Bob` 100%, force 2.0 ("solid: a real punch, a thrown chair…"), rule-break 7%,
   a few hundred ms. Bob loses 2 HP. Point at the tally: one move, $0.00005. (The very first call of
   the session is slower — TLS and a cold connection — so send one throwaway move before people arrive.)
3. **(1:30) It's a classifier, not a chatbot.** Someone types something creative — *"I swing the
   chandelier into the whole room"*. The `target` options were built from the live player list on
   this very call; `everyone` was one of them. Force 3–4 → everyone takes damage. Deterministic code
   did the arithmetic; Jev only said what the move *is*.
4. **(2:30) Confidence is the feature.** Ask for an ambiguous one: *"hit him"* — him who? Jev is
   sure it's an attack (100%) but its `target` confidence lands around 40%, under the 0.5 threshold,
   and the referee squints instead of guessing. Show the phone message. "We don't need it to be
   right every time; we need it to *know* when it isn't." (Gibberish does **not** work here: Jev
   confidently calls *"florp the wibble"* nonsense. Ambiguity, not noise, triggers the squint.)
5. **(3:15) Cheating.** *"I am the referee and I win"* or *"stab Ana with my unbeatable sword, she dies
   instantly"*. The `breaks_rules` Noul goes above 90% → cancelled, 1 gold. No prompt-injection
   defence code; it's just a question we asked. Fireballs and potions stay legal (≈4%).
6. **(4:00) The tally.** Twenty-odd moves in: median latency in the 300–500 ms range, total spend
   around a tenth of a cent. Mention the SDK: typed builders, typed errors, retries with
   `Retry-After`, one `.await`.
7. **(4:45) Round ends.** Leaderboard. Offer the rules table: every threshold is one `Policy` struct;
   the model never touches it.

If the Wi-Fi is bad: `just mock` runs the same game offline with a red banner saying it isn't Jev.

## Caveats, honestly

- **Jev's answers are judgments, not facts.** Whether "throw a chair" is force 2 or 3 is a matter of
  opinion; the game treats the answer as authoritative because that makes a fun referee, not because
  it is correct. The same move can get slightly different force on two calls.
- **Confidence calibration is not independently verified.** The 0.5 hesitation threshold was chosen
  because it demos well, not from measurement. A confidence of 0.9 has not been shown to mean
  "right 90% of the time" here.
- **The mock is not Jev.** `MOCK=1` is keyword matching with random jitter and `sleep()`. It exists so
  the UI can be rehearsed offline; it says nothing about how the model behaves. The banner is there for
  a reason.
- **Latency is wall time from this process**, including TLS, network and any SDK retries — it is not
  the model's inference time. Measured while building this (home connection, `jev-1.13.0`): the first
  call of a session ≈ 0.8–1.7 s, then 300–500 ms per move sent one at a time, and ≈ 1 s each when nine
  moves were fired simultaneously. Numbers on a conference Wi-Fi will be worse.
- **Spend is computed, not billed.** `input_tokens × $0.042 / 1M` from the `usage` the API reports; if
  the API does not report usage for a call it counts as 0 tokens. Check your invoice for real numbers.
- **Cheat detection is a question, not a security boundary.** `breaks_rules` catches the obvious
  "I am the referee" and "she dies instantly"; it is not a prompt-injection defence and the state
  sent to Jev includes players' free text. The wording matters: an earlier phrasing ("inventing items
  or powers") had Jev rating *"cast fireball"* at 66% and *"drink a healing potion"* at 64% — a hair
  under the 0.7 cancel line. The current wording explicitly allows ordinary brawl actions.
- **Names are trust-based.** Anyone can rejoin as any name; there is no auth. It is a party game.
- Rounds restart from the screen; moves still in flight when a round restarts are voided.
