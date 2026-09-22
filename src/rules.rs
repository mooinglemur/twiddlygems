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
///
/// The two line clearers are separate flags rather than one, because they are
/// unlocked separately. Note which run makes which: a run is finished by
/// sliding a gem across it, and the gem left behind clears the other way, so a
/// run of four along a row leaves a clearer that fires down its column. A set
/// that allows one and not the other is therefore lopsided in a way players
/// will feel, with four in a row worth a gem and four in a column worth
/// nothing, or the reverse.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SpecialSet {
    /// A run of exactly 4 down a column creates a gem that clears a row.
    pub line_h: bool,
    /// A run of exactly 4 along a row creates a gem that clears a column.
    pub line_v: bool,
    /// An L or T shaped match creates a gem that clears a row and a column.
    pub cross: bool,
    /// A run of 5 or more creates a gem that clears a whole color.
    pub rainbow: bool,
    /// A 2x2 block creates a rocket that flies off at another gem.
    pub rocket: bool,
}

impl SpecialSet {
    pub const ALL: SpecialSet =
        SpecialSet { line_h: true, line_v: true, cross: true, rainbow: true, rocket: true };
    pub const NONE: SpecialSet =
        SpecialSet { line_h: false, line_v: false, cross: false, rainbow: false, rocket: false };
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rules {
    pub rows: i32,
    pub cols: i32,
    /// How many distinct gem colors are in play, at most [`MAX_COLORS`].
    pub colors: u8,
    /// Which colors those are, in dealing order.
    ///
    /// Only the first `colors` entries mean anything. A level normally takes
    /// the first few of the palette and this is the identity, but it can name
    /// any set instead: a board of seals wants the colors on the board to be
    /// the colors the seals answer to, and nothing else cluttering it.
    pub palette: [u8; MAX_COLORS],
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

impl Rules {
    /// Draws one of the colors this level deals.
    ///
    /// The single place a gem's color is invented, so a level that names its
    /// palette gets those colors everywhere: the deal, the refill, and anything
    /// that comes along later.
    pub fn draw_color(&self, rng: &mut crate::rng::Rng) -> u8 {
        let count = (self.colors as usize).clamp(1, MAX_COLORS);
        self.palette[rng.below(count as u32) as usize]
    }

    /// Whether this level deals that color at all.
    pub fn deals(&self, color: u8) -> bool {
        let count = (self.colors as usize).clamp(0, MAX_COLORS);
        self.palette[..count].contains(&color)
    }
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            rows: 9,
            cols: 9,
            colors: 6,
            palette: [0, 1, 2, 3, 4, 5, 6, 7],
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Rng;

    #[test]
    fn a_named_palette_deals_those_colors_and_no_others() {
        let rules = Rules { colors: 3, palette: [4, 6, 7, 0, 0, 0, 0, 0], ..Rules::default() };

        let mut rng = Rng::new(9);
        let mut seen = [false; MAX_COLORS];
        for _ in 0..600 {
            let color = rules.draw_color(&mut rng);
            assert!(
                matches!(color, 4 | 6 | 7),
                "dealt color {color}, which is not in the palette",
            );
            seen[color as usize] = true;
        }
        assert!(seen[4] && seen[6] && seen[7], "all three should come up");
        assert!(rules.deals(6));
        assert!(!rules.deals(5), "a color left out of the palette is not dealt");
    }
}
