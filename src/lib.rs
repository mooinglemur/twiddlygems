//! Twiddly Gems: a match-3 puzzle engine.
//!
//! The engine is deliberately free of browser concerns. It owns the board, the
//! rules, the clock and the state machine; a front end asks it to advance time,
//! hands it input, and reads a snapshot to draw. That split keeps the whole of
//! the gameplay testable without a browser, and leaves room for the front end
//! to be replaced.

pub mod board;
pub mod ffi;
pub mod game;
pub mod level;
pub mod matching;
pub mod progression;
pub mod rng;
pub mod rules;
pub mod session;
