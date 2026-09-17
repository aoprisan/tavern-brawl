//! Tavern Brawl — a text-action arena game where every move is judged by TypeSafe's Jev model.
//!
//! Layering, from the inside out:
//! - [`game`]: deterministic rules. `Game`, `Policy`, `resolve()`. No I/O, no Jev.
//! - [`referee`]: the `Referee` trait (`JevReferee` on the SDK, `MockReferee` offline) and the
//!   `Verdict` it returns.
//! - [`protocol`]: the WebSocket messages, defined once.
//! - [`server`]: axum routes, WebSocket fan-out, the move pipeline.

pub mod game;
pub mod protocol;
pub mod referee;
pub mod server;
