//! Level definitions and the objectives that end one.

use crate::rules::{Rules, SpecialSet, MAX_COLORS};

/// A goal the player has to reach before the moves run out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Objective {
    /// Reach a score.
    Score(u32),
    /// Clear a number of gems of one color.
    Color { color: u8, count: u32 },
    /// Peel every layer of jelly on the board.
    Jelly,
}

impl Objective {
    /// A stable tag for the front end to pick an icon and phrasing from.
    pub fn kind_code(self) -> u32 {
        match self {
            Objective::Score(_) => 0,
            Objective::Color { .. } => 1,
            Objective::Jelly => 2,
        }
    }

    /// The color this objective concerns, or 255 when it concerns none.
    pub fn color(self) -> u8 {
        match self {
            Objective::Color { color, .. } => color,
            _ => 255,
        }
    }

    pub fn needed(self, progress: &Progress) -> u32 {
        match self {
            Objective::Score(target) => target,
            Objective::Color { count, .. } => count,
            Objective::Jelly => progress.jelly_total,
        }
    }

    /// How far along the player is, never reported as more than needed.
    pub fn reached(self, progress: &Progress) -> u32 {
        let raw = match self {
            Objective::Score(_) => progress.score.min(u32::MAX as u64) as u32,
            Objective::Color { color, .. } => progress.cleared_by_color(color),
            Objective::Jelly => progress.jelly_total - progress.jelly_left,
        };
        raw.min(self.needed(progress))
    }

    pub fn is_met(self, progress: &Progress) -> bool {
        self.reached(progress) >= self.needed(progress)
    }
}

/// Running totals for the level in play.
#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub score: u64,
    pub cleared: [u32; MAX_COLORS],
    pub jelly_total: u32,
    pub jelly_left: u32,
}

impl Progress {
    pub fn cleared_by_color(&self, color: u8) -> u32 {
        self.cleared.get(color as usize).copied().unwrap_or(0)
    }
}

/// Everything that makes one level distinct from the next.
#[derive(Clone, Debug)]
pub struct LevelSpec {
    pub name: &'static str,
    pub rules: Rules,
    /// Moves the player gets. 0 means unlimited.
    pub moves: u32,
    pub objectives: Vec<Objective>,
    /// Board shape and jelly placement; a plain rectangle when absent.
    pub layout: Option<&'static [&'static str]>,
}

impl LevelSpec {
    fn new(name: &'static str, moves: u32, objectives: Vec<Objective>) -> Self {
        LevelSpec { name, rules: Rules::default(), moves, objectives, layout: None }
    }

    fn with_layout(mut self, layout: &'static [&'static str]) -> Self {
        self.rules.rows = layout.len() as i32;
        self.rules.cols = layout.iter().map(|row| row.chars().count()).max().unwrap_or(0) as i32;
        self.layout = Some(layout);
        self
    }

    fn colors(mut self, colors: u8) -> Self {
        self.rules.colors = colors;
        self
    }

    fn specials(mut self, specials: SpecialSet) -> Self {
        self.rules.specials = specials;
        self
    }
}

// A layout declares its own size, so these are the board on the levels that
// use one. All four are built around the middle, which an odd board actually
// has: on nine columns a centered shape sits on column four rather than
// straddling the gap between two.

const JELLY_PATCH: &[&str] = &[
    ".........",
    ".........",
    ".........",
    "..ooooo..",
    "..ooooo..",
    "..ooooo..",
    ".........",
    ".........",
    ".........",
];

const CROSS: &[&str] = &[
    "##.....##",
    "#.......#",
    ".........",
    "....o....",
    "...ooo...",
    "....o....",
    ".........",
    "#.......#",
    "##.....##",
];

const HOURGLASS: &[&str] = &[
    "ooooooooo",
    ".ooooooo.",
    "..OOOOO..",
    "...OOO...",
    "....O....",
    "...OOO...",
    "..OOOOO..",
    ".ooooooo.",
    "ooooooooo",
];

// Bricks, which nothing falls through and nothing swaps with, so the gems
// above them have to spill around the ends. The shelf is deliberately wide
// enough that the pocket under it can only fill from the sides.
const QUARRY: &[&str] = &[
    ".........",
    ".........",
    ".........",
    "..BBBBB..",
    "..o...o..",
    ".........",
    "..bbbbb..",
    "..o...o..",
    ".........",
];

// Half a board, cut corner to corner. Everything above and left of the
// diagonal is solid, and the diagonal itself is in play, so this is a shade
// over half the cells. Built to watch gravity work: every column is a
// different depth and every one of them ends against the slope.
const SLOPE: &[&str] = &[
    "########.",
    "#######..",
    "######...",
    "#####....",
    "####.....",
    "###......",
    "##.......",
    "#........",
    ".........",
];

// The inner corners of each pillar are brick rather than wall, and that is
// load bearing rather than decoration.
//
// Gems arrive from off the top of the board or by spilling in from the side,
// so a cell is only reachable from the three cells above it. Under a solid
// three-wide block that leaves the middle of the row below it with nothing
// over it but wall, and the jelly there is orphaned the moment it is first
// cleared: the level cannot then be finished at all. Brick has the shape of
// wall until something breaks it, and a broken brick is a way through.
const PILLARS: &[&str] = &[
    "..o###o..",
    "..oB#Bo..",
    "..ooooo..",
    ".........",
    ".........",
    ".........",
    "..ooooo..",
    "..oB#Bo..",
    "..o###o..",
];

/// The built-in level ladder. Ordered by difficulty; the solo campaign walks
/// it top to bottom, and the Archipelago layer will later gate the same list.
pub fn levels() -> Vec<LevelSpec> {
    vec![
        // Wide-open boards with few colors: matches fall into your lap while
        // you learn that swapping is all there is to it.
        LevelSpec::new("First Light", 20, vec![Objective::Score(4_000)])
            .colors(5)
            .specials(SpecialSet::NONE),
        LevelSpec::new("Finding Fours", 22, vec![Objective::Score(7_000)]).colors(5),
        // Color goals ask you to aim rather than to clear whatever is nearest.
        LevelSpec::new("Ruby Hunt", 22, vec![Objective::Color { color: 0, count: 30 }]).colors(5),
        LevelSpec::new(
            "Two Tastes",
            26,
            vec![
                Objective::Color { color: 1, count: 28 },
                Objective::Color { color: 3, count: 28 },
            ],
        ),
        // Jelly arrives: now position matters, not just volume.
        LevelSpec::new("Sticky Middle", 16, vec![Objective::Jelly]).with_layout(JELLY_PATCH),
        LevelSpec::new(
            "Crowded House",
            26,
            vec![Objective::Score(13_000), Objective::Color { color: 4, count: 26 }],
        ),
        // Walls break the board into tubes and make cascades harder to aim.
        LevelSpec::new("Crossroads", 20, vec![Objective::Jelly, Objective::Score(12_000)])
            .with_layout(CROSS),
        LevelSpec::new("Pillars", 32, vec![Objective::Jelly]).with_layout(PILLARS),
        LevelSpec::new("Hourglass", 40, vec![Objective::Jelly]).with_layout(HOURGLASS),
        // The jelly is under the brick shelves, so it cannot be reached until
        // the bricks come down, and nothing falls into those pockets until the
        // gems above spill around the ends.
        LevelSpec::new("Quarry", 34, vec![Objective::Jelly]).with_layout(QUARRY),
        LevelSpec::new("Landslide", 40, vec![Objective::Score(9_000)]).with_layout(SLOPE),
        LevelSpec::new(
            "Last Call",
            30,
            vec![
                Objective::Score(18_000),
                Objective::Color { color: 2, count: 32 },
            ],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_jelly_cell_can_be_reached() {
        // Gems arrive from off the top of the board or by spilling in from the
        // side, so a cell is only ever fed from the three cells above it. Jelly
        // that nothing can reach can be cleared once and then never covered
        // again, and the level becomes unwinnable the moment it is.
        //
        // Brick counts as a way through, because it can be broken. Wall does
        // not. Pillars was unwinnable for exactly this reason: a three-wide
        // block of wall left the middle of the row under it fed by nothing.
        for level in levels() {
            let Some(layout) = level.layout else { continue };
            let rows = layout.len();
            let cols = level.rules.cols as usize;
            let at = |r: usize, c: usize| layout[r].chars().nth(c).unwrap_or('.');
            let wall = |r: usize, c: usize| at(r, c) == '#';

            let mut fed = vec![vec![false; cols]; rows];
            for r in 0..rows {
                for c in 0..cols {
                    if wall(r, c) {
                        continue;
                    }
                    fed[r][c] = r == 0
                        || [c.wrapping_sub(1), c, c + 1]
                            .into_iter()
                            .any(|over| over < cols && !wall(r - 1, over) && fed[r - 1][over]);
                }
            }

            for r in 0..rows {
                for c in 0..cols {
                    let jelly = matches!(at(r, c), 'o' | 'O');
                    assert!(
                        !jelly || fed[r][c],
                        "{}: nothing can ever reach the jelly at ({r},{c})",
                        level.name,
                    );
                }
            }
        }
    }

    #[test]
    fn every_level_is_winnable_in_principle() {
        for level in levels() {
            assert!(level.moves > 0, "{} has no moves", level.name);
            assert!(!level.objectives.is_empty(), "{} has no objectives", level.name);
            for objective in &level.objectives {
                if let Objective::Color { color, .. } = objective {
                    assert!(
                        *color < level.rules.colors,
                        "{} asks for a color that is not in play",
                        level.name
                    );
                }
            }
        }
    }

    #[test]
    fn jelly_levels_actually_have_jelly() {
        for level in levels() {
            if level.objectives.contains(&Objective::Jelly) {
                let layout = level.layout.expect("a jelly level needs a layout");
                assert!(
                    layout.iter().any(|row| row.contains('o') || row.contains('O')),
                    "{} has a jelly objective but no jelly",
                    level.name
                );
            }
        }
    }

    #[test]
    fn layouts_match_their_declared_size() {
        for level in levels() {
            if let Some(layout) = level.layout {
                assert_eq!(layout.len() as i32, level.rules.rows, "{}", level.name);
                for row in layout {
                    assert_eq!(row.chars().count() as i32, level.rules.cols, "{}", level.name);
                }
            }
        }
    }

    #[test]
    fn progress_clamps_at_the_target() {
        let objective = Objective::Score(1000);
        let progress = Progress { score: 5000, ..Progress::default() };
        assert_eq!(objective.reached(&progress), 1000);
        assert!(objective.is_met(&progress));
    }

    #[test]
    fn a_jelly_objective_is_met_when_the_board_is_clean() {
        let objective = Objective::Jelly;
        let mut progress = Progress { jelly_total: 12, jelly_left: 12, ..Progress::default() };
        assert!(!objective.is_met(&progress));
        assert_eq!(objective.reached(&progress), 0);
        progress.jelly_left = 0;
        assert!(objective.is_met(&progress));
        assert_eq!(objective.reached(&progress), 12);
    }
}
