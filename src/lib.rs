//! Twiddly Gems: a match-3 puzzle engine.
//!
//! The engine is deliberately free of browser concerns. It owns the board, the
//! rules, the clock and the state machine; a front end asks it to advance time,
//! hands it input, and reads a snapshot to draw. That split keeps the whole of
//! the gameplay testable without a browser, and leaves room for the front end
//! to be replaced.

/// What this build is: the crate version and the commit it was built from.
///
/// Shown in the page's footer, and nowhere else. It says which build somebody
/// is looking at, which a version alone cannot: the crate version moves once a
/// release and the game moves rather more often than that.
///
/// The commit comes from `build.rs`, and reads `unknown` where there was no
/// checkout to ask.
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "+", env!("TG_GIT_HASH"));

pub mod board;
/// Compression for the server that ships the built game. Nothing the engine
/// itself does needs it, and nothing in the wasm module reaches it.
pub mod deflate;
pub mod ffi;
pub mod game;
pub mod level;
pub mod matching;
pub mod options;
pub mod progression;
pub mod rng;
pub mod rules;
pub mod session;
