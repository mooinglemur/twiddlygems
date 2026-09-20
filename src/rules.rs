//! Tunable rules for a board.
//!
//! Everything a puzzle variant would want to vary lives here as data rather
//! than as branches inside the engine, so a second variant is a different
//! `Rules` value and not a fork of `Game`.

/// Where replacement gems enter the board from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RefillMode {
    /// New gems drop in at the top of each open column segment.
    TopSpawn,
    /// Cleared cells stay empty; the board drains.
    None,
}

/// Which swaps the player may attempt.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SwapMode {
    /// The classic: one step up, down, left or right.
    Orthogonal,
}

/// Which special gems a match can create.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SpecialSet {
    /// A run of exactly 4 creates a gem that clears across the run.
    pub line: bool,
    /// An L or T shaped match creates a gem that clears a row and a column.
    pub cross: bool,
    /// A run of 5 or more creates a gem that clears a whole color.
    pub rainbow: bool,
    /// A 2x2 block creates a rocket that flies off at another gem.
    pub rocket: bool,
}

impl SpecialSet {
    pub const ALL: SpecialSet =
        SpecialSet { line: true, cross: true, rainbow: true, rocket: true };
    pub const NONE: SpecialSet =
        SpecialSet { line: false, cross: false, rainbow: false, rocket: false };
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rules {
    pub rows: i32,
    pub cols: i32,
    /// Distinct gem colors in play, at most [`MAX_COLORS`].
    pub colors: u8,
    /// Shortest run that counts as a match.
    pub min_match: i32,
    /// Whether a 2x2 block of one color counts as a match on its own.
    pub square_match: bool,
    pub specials: SpecialSet,
    pub refill: RefillMode,
    pub swap: SwapMode,
    /// Whether a swap that matches nothing slides back.
    pub revert_invalid: bool,
    /// Reshuffle in place when no legal move is left, rather than ending the level.
    pub shuffle_when_stuck: bool,
}

/// The engine indexes per-color counters with fixed arrays, so colors are capped.
pub const MAX_COLORS: usize = 8;

impl Default for Rules {
    fn default() -> Self {
        Rules {
            rows: 8,
            cols: 8,
            colors: 6,
            min_match: 3,
            square_match: true,
            specials: SpecialSet::ALL,
            refill: RefillMode::TopSpawn,
            swap: SwapMode::Orthogonal,
            revert_invalid: true,
            shuffle_when_stuck: true,
        }
    }
}
