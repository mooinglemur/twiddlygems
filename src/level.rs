//! Level definitions and the objectives that end one.

use crate::rules::{Rules, MAX_COLORS};

/// A goal the player has to reach before the moves run out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Objective {
    /// Reach a score.
    Score(u32),
    /// Clear a number of gems of one color.
    Color { color: u8, count: u32 },
    /// Peel every layer of jelly on the board.
    Jelly,
    /// Break every brick on the board, cracked ones counting as half gone.
    Brick,
    /// Break every seal of one color, counted the same way.
    ///
    /// Separate from [`Objective::Brick`] so a board of seals can ask for each
    /// color in turn. One lumped total lets a player finish by breaking
    /// whatever was easiest to reach; four of these make the level about
    /// bringing the right color to each one, which is what a seal is for.
    Seal { color: u8 },
}

impl Objective {
    /// A stable tag for the front end to pick an icon and phrasing from.
    pub fn kind_code(self) -> u32 {
        match self {
            Objective::Score(_) => 0,
            Objective::Color { .. } => 1,
            Objective::Jelly => 2,
            Objective::Brick => 3,
            Objective::Seal { .. } => 4,
        }
    }

    /// The color this objective concerns, or 255 when it concerns none.
    pub fn color(self) -> u8 {
        match self {
            Objective::Color { color, .. } | Objective::Seal { color } => color,
            _ => 255,
        }
    }

    pub fn needed(self, progress: &Progress) -> u32 {
        match self {
            Objective::Score(target) => target,
            Objective::Color { count, .. } => count,
            Objective::Jelly => progress.jelly_total,
            Objective::Brick => progress.brick_total,
            Objective::Seal { color } => progress.seal_total(color),
        }
    }

    /// How far along the player is, never reported as more than needed.
    pub fn reached(self, progress: &Progress) -> u32 {
        let raw = match self {
            Objective::Score(_) => progress.score.min(u32::MAX as u64) as u32,
            Objective::Color { color, .. } => progress.cleared_by_color(color),
            Objective::Jelly => progress.jelly_total - progress.jelly_left,
            Objective::Brick => progress.brick_total - progress.brick_left,
            Objective::Seal { color } => progress.seal_total(color) - progress.seal_left(color),
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
    /// Cells with jelly under them, at the start and now.
    ///
    /// Cells rather than layers, and the same goes for the bricks and seals
    /// below. What a level asks for is that every one of them be cleared, so
    /// the counter says how many are left to clear: softening a double layer
    /// of jelly or cracking a brick moves nothing, because nothing has been
    /// finished. The board shows that work happening; the counter is for what
    /// remains.
    pub jelly_total: u32,
    pub jelly_left: u32,
    pub brick_total: u32,
    pub brick_left: u32,
    /// The same split by color, for the seals among them. A plain brick
    /// belongs to no color and appears in neither of these.
    pub seals_at_start: [u32; MAX_COLORS],
    pub seals_now: [u32; MAX_COLORS],
    /// How many moves were still in hand when the goals were met.
    ///
    /// Recorded before those moves are cashed in, so it survives the flourish
    /// spending them: afterwards the counter always reads zero, and how
    /// briskly a level was beaten would be lost. What a score tier is chased
    /// with, and the honest read on how hard a level asked.
    pub moves_spare: u32,
}

impl Progress {
    pub fn cleared_by_color(&self, color: u8) -> u32 {
        self.cleared.get(color as usize).copied().unwrap_or(0)
    }

    pub fn seal_total(&self, color: u8) -> u32 {
        self.seals_at_start.get(color as usize).copied().unwrap_or(0)
    }

    pub fn seal_left(&self, color: u8) -> u32 {
        self.seals_now.get(color as usize).copied().unwrap_or(0)
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
    /// Scores worth coming back for, over and above simply clearing the level.
    ///
    /// Not objectives: the level ends when its objectives are met, whatever
    /// the score. These are what the end-of-level flourish is chased for, and
    /// each is somewhere an item can be found, so beating a level well is
    /// worth something beyond the number. Zero means the level offers no tier.
    pub silver: u64,
    pub gold: u64,
    /// Board shape and jelly placement; a plain rectangle when absent.
    pub layout: Option<&'static [&'static str]>,
}

impl LevelSpec {
    fn new(name: &'static str, moves: u32, objectives: Vec<Objective>) -> Self {
        LevelSpec {
            name,
            rules: Rules::default(),
            moves,
            objectives,
            silver: 0,
            gold: 0,
            layout: None,
        }
    }

    /// The two scores worth coming back for. See [`LevelSpec::silver`].
    fn tiers(mut self, silver: u64, gold: u64) -> Self {
        self.silver = silver;
        self.gold = gold;
        self
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

    /// Names exactly which colors this level deals, rather than taking the
    /// first few of the palette.
    ///
    /// What a board of seals wants: the colors on it should be the colors the
    /// seals answer to, with nothing else in the way to dilute the draw.
    fn palette(mut self, colors: &[u8]) -> Self {
        self.rules.colors = colors.len() as u8;
        for (slot, color) in self.rules.palette.iter_mut().zip(colors) {
            *slot = *color;
        }
        self
    }

}

// A layout declares its own size, so these are the board on the levels that
// use one. All four are built around the middle, which an odd board actually
// has: on nine columns a centered shape sits on column four rather than
// straddling the gap between two.

const FIRST_LEVEL: &[&str] = &[
    "##....##",
    "##....##",
    "##....##",
    "##....##",
    "##2110##",
    "##1002##",
    "##2110##",
    "##1002##",
];

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
    "..=====..",
    "..o...o..",
    ".........",
    "..-----..",
    "..o...o..",
    ".........",
];

// Half a board, cut corner to corner, and the half that is cut away is the
// level: every cell of it is brick to be broken through.
//
// Play starts in the bottom right triangle, fed by the one cell of the top row
// that is not brick, so the whole board fills by running down the slope. The
// diagonal face is what can be reached to begin with, and past that it is beams
// that do the work, since a beam goes through brick rather than stopping at it.
//
// The top row is whole brick and the rest is cracked. Breaking a cell of the
// top row opens a new way in, because the top row is where gems enter.
const SLOPE: &[&str] = &[
    "========.",
    "-------..",
    "------...",
    "-----....",
    "----.....",
    "---......",
    "--.......",
    "-........",
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
// Seals, four colors, each column answering to one of them. The level names
// its palette so those four are the only colors dealt: a seal you cannot bring
// the right color to is just a wall, and diluting the draw with colors no seal
// wants would do exactly that.
//
// Colors here are palette indices: 0 red, 1 blue, 2 green, 3 yellow.
const VAULT: &[&str] = &[
    ".........",
    ".A.B.C.D.",
    ".a.b.c.d.",
    ".........",
    "..ooooo..",
    ".........",
    ".d.c.b.a.",
    ".D.C.B.A.",
    ".........",
];

const PILLARS: &[&str] = &[
    "..o###o..",
    "..o=#=o..",
    "..ooooo..",
    ".........",
    ".........",
    ".........",
    "..ooooo..",
    "..o=#=o..",
    "..o###o..",
];

/// The built-in level ladder. Ordered by difficulty; the solo campaign walks
/// it top to bottom, and the Archipelago layer will later gate the same list.
pub fn levels() -> Vec<LevelSpec> {
    vec![
        // Wide-open boards with few colors: matches fall into your lap while
        // you learn that swapping is all there is to it.
        // Nothing is switched off here any more: a new run simply holds none
        // of the unlocks yet, so the opener plays as plain matching on its
        // own. Come back to it later and it hands out specials like the rest.
        // The tiers on each level are read off the bots in `make balance`,
        // roughly the attentive bot's median at 45% and 80%. Silver is what a
        // good run reaches; gold wants the level beaten briskly, because what
        // fills the gap is the flourish spending the moves left over. The
        // opener is the exception: with no unlocks there is no flourish, so
        // its two sit just above what scraping a win pays.
        LevelSpec::new(
            "First Light",
            3,
            vec![
                Objective::Color { color: 0, count: 9 },
                Objective::Color { color: 1, count: 9 },
            ])
            .with_layout(FIRST_LEVEL)
            .colors(4)
            .palette(&[0, 5, 1, 2])
            // Out of reach without specials, which is what a mark is for: both
            // of them ask for all five unlocks, and a number a bare run scores
            // anyway would make that rule a lie. Three moves on a narrow board
            // means a long tail, so these are set off the far end of it rather
            // than off the middle: two hundred bare runs top out around 25,000
            // and a supplied one sits near 31,000.
            .tiers(30_000, 45_000),
        LevelSpec::new("Finding Fours", 22, vec![Objective::Score(7_000)])
            .colors(5)
            .tiers(12_000, 22_000),
        // Color goals ask you to aim rather than to clear whatever is nearest.
        LevelSpec::new("Ruby Hunt", 22, vec![Objective::Color { color: 0, count: 30 }])
            .colors(5)
            .tiers(23_000, 41_000),
        LevelSpec::new(
            "Two Tastes",
            26,
            vec![
                Objective::Color { color: 1, count: 28 },
                Objective::Color { color: 3, count: 28 },
            ],
        )
        .tiers(18_000, 33_000),
        // Jelly arrives: now position matters, not just volume.
        LevelSpec::new("Sticky Middle", 16, vec![Objective::Jelly])
            .with_layout(JELLY_PATCH)
            .tiers(13_000, 24_000),
        LevelSpec::new(
            "Crowded House",
            26,
            vec![Objective::Score(13_000), Objective::Color { color: 4, count: 26 }],
        )
        .tiers(17_000, 30_000),
        // Walls break the board into tubes and make cascades harder to aim.
        LevelSpec::new("Crossroads", 20, vec![Objective::Jelly, Objective::Score(12_000)])
            .with_layout(CROSS)
            .tiers(13_000, 24_000),
        LevelSpec::new("Pillars", 32, vec![Objective::Jelly])
            .with_layout(PILLARS)
            .tiers(14_000, 25_000),
        LevelSpec::new("Hourglass", 40, vec![Objective::Jelly])
            .with_layout(HOURGLASS)
            .tiers(26_000, 47_000),
        // The jelly is under the brick shelves, so it cannot be reached until
        // the bricks come down, and nothing falls into those pockets until the
        // gems above spill around the ends.
        LevelSpec::new("Quarry", 34, vec![Objective::Jelly])
            .with_layout(QUARRY)
            .tiers(13_000, 24_000),
        LevelSpec::new("Landslide", 40, vec![Objective::Brick])
            .with_layout(SLOPE)
            .tiers(25_000, 44_000),
        // A goal per color rather than one lumped total, so the level is about
        // bringing each color to its own seals rather than breaking whichever
        // happened to be easiest to reach.
        LevelSpec::new(
            "The Vault",
            30,
            vec![
                Objective::Seal { color: 0 },
                Objective::Seal { color: 1 },
                Objective::Seal { color: 2 },
                Objective::Seal { color: 3 },
            ],
        )
        .with_layout(VAULT)
        .palette(&[0, 1, 2, 3])
        .tiers(310_000, 550_000),
        LevelSpec::new(
            "Last Call",
            30,
            vec![
                Objective::Score(18_000),
                Objective::Color { color: 2, count: 32 },
            ],
        )
        .tiers(27_000, 48_000),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_brick_objective_counts_hits_and_is_met_when_the_board_is_clear() {
        let objective = Objective::Brick;
        // Two whole bricks and one cracked: five hits between them.
        let mut progress = Progress { brick_total: 5, brick_left: 5, ..Progress::default() };
        assert!(!objective.is_met(&progress));
        assert_eq!(objective.reached(&progress), 0);

        // Cracking one moves the bar, rather than it sitting still until a
        // whole brick finally goes.
        progress.brick_left = 4;
        assert_eq!(objective.reached(&progress), 1);
        assert!(!objective.is_met(&progress));

        progress.brick_left = 0;
        assert!(objective.is_met(&progress));
        assert_eq!(objective.reached(&progress), 5);
    }

    #[test]
    fn a_seal_objective_counts_only_its_own_color() {
        // Breaking every red seal should finish the red goal whatever is left
        // of the blue ones, which is the whole reason these are separate.
        let red = Objective::Seal { color: 0 };
        let blue = Objective::Seal { color: 1 };
        let mut progress = Progress {
            seals_at_start: [4, 6, 0, 0, 0, 0, 0, 0],
            seals_now: [4, 6, 0, 0, 0, 0, 0, 0],
            ..Progress::default()
        };
        assert_eq!(red.needed(&progress), 4);
        assert_eq!(blue.needed(&progress), 6);

        progress.seals_now[0] = 0;
        assert!(red.is_met(&progress), "every red seal is gone");
        assert!(!blue.is_met(&progress), "and the blue ones are somebody else's problem");
        assert_eq!(red.reached(&progress), 4);
        assert_eq!(blue.reached(&progress), 0);
    }

    #[test]
    fn a_seal_objective_asks_for_a_color_the_level_actually_seals() {
        // A goal for a color with no seals of it on the board is met before the
        // level starts, which is not a goal.
        for level in levels() {
            let Some(layout) = level.layout else { continue };
            for objective in &level.objectives {
                let Objective::Seal { color } = objective else { continue };
                let whole = (b'A' + color) as char;
                let cracked = (b'a' + color) as char;
                let count = layout
                    .iter()
                    .flat_map(|row| row.chars())
                    .filter(|ch| *ch == whole || *ch == cracked)
                    .count();
                assert!(
                    count > 0,
                    "{} asks for color {color} seals and has none",
                    level.name,
                );
            }
        }
    }

    #[test]
    fn every_seal_answers_to_a_color_the_level_deals() {
        // A seal is broken by its own color going off beside it. Key one to a
        // color the level never deals and it is a wall with a lie painted on
        // it: only a beam could ever touch it, and a brick objective built on
        // that would be a puzzle nobody could read.
        for level in levels() {
            let Some(layout) = level.layout else { continue };
            for (r, row) in layout.iter().enumerate() {
                for (c, ch) in row.chars().enumerate() {
                    let color = match ch {
                        'A'..='H' => ch as u8 - b'A',
                        'a'..='h' => ch as u8 - b'a',
                        _ => continue,
                    };
                    assert!(
                        level.rules.deals(color),
                        "{}: the seal at ({r},{c}) wants color {color}, which is never dealt",
                        level.name,
                    );
                }
            }
        }
    }

    #[test]
    fn a_named_palette_is_the_only_thing_dealt() {
        let level = levels()
            .into_iter()
            .find(|level| level.name == "The Vault")
            .expect("the seal level should be in the ladder");
        assert_eq!(level.rules.colors, 4);
        for color in 0..4u8 {
            assert!(level.rules.deals(color));
        }
        for color in 4..8u8 {
            assert!(!level.rules.deals(color), "color {color} should not be in play");
        }
    }

    #[test]
    fn every_brick_can_be_got_at() {
        // A brick is broken by something clearing beside it or by a beam going
        // through it, and a beam runs the length of a row or a column. So a
        // brick is reachable if a gem can ever stand next to it, or anywhere
        // along its row or its column. One walled off from all three could
        // never be broken, and a brick objective would be unwinnable.
        for level in levels() {
            let Some(layout) = level.layout else { continue };
            if !level.objectives.contains(&Objective::Brick) {
                continue;
            }
            let rows = layout.len();
            let cols = level.rules.cols as usize;
            let at = |r: usize, c: usize| layout[r].chars().nth(c).unwrap_or('.');
            let blocker =
                |r: usize, c: usize| matches!(at(r, c), '=' | '-' | 'A'..='H' | 'a'..='h');
            let playable = |r: usize, c: usize| at(r, c) != '#' && !blocker(r, c);

            for r in 0..rows {
                for c in 0..cols {
                    if !blocker(r, c) {
                        continue;
                    }
                    let along_row = (0..cols).any(|x| playable(r, x));
                    let along_col = (0..rows).any(|y| playable(y, c));
                    assert!(
                        along_row || along_col,
                        "{}: nothing could ever break the brick at ({r},{c})",
                        level.name,
                    );
                }
            }
        }
    }

    #[test]
    fn every_placed_gem_is_a_color_its_level_deals() {
        // A digit naming a color the level never deals puts a gem on the board
        // that nothing can ever match, which is a blocker drawn by accident.
        for level in levels() {
            let Some(layout) = level.layout else { continue };
            for (r, row) in layout.iter().enumerate() {
                for (c, ch) in row.chars().enumerate() {
                    let Some(color) = ch.to_digit(10).map(|digit| digit as u8) else { continue };
                    assert!(
                        level.rules.deals(color),
                        "{}: the gem at ({r},{c}) is color {color}, which the level never deals",
                        level.name,
                    );
                }
            }
        }
    }

    #[test]
    fn no_level_opens_on_a_match_it_placed_itself() {
        // A level opening mid-match plays a little of itself before the player
        // has touched it. The deal avoids that where it can, and accepts it
        // when a board leaves it no choice, which is the right way round for
        // something it cannot control. What it cannot avoid at all is a match
        // written into the layout: those gems are placed every time, on every
        // seed, so that level opens chaining forever.
        for level in levels() {
            let Some(layout) = level.layout else { continue };
            let board = crate::board::Board::from_layout(layout);
            let matches = crate::matching::find_matches(&board, &level.rules);
            assert!(
                matches.is_empty(),
                "{}: the gems it places are already a match at {:?}",
                level.name,
                matches[0].cells,
            );
        }
    }

    #[test]
    fn every_level_opens_with_something_to_do() {
        // The failure a deal may not accept, asked of the real ladder. A level
        // whose layout leaves too little room would show up here rather than
        // in front of somebody, as a board they can only stare at.
        for (index, level) in levels().into_iter().enumerate() {
            for seed in 0..20 {
                let game = crate::game::Game::new(level.clone(), seed * 31 + index as u64);
                assert!(
                    crate::matching::find_move(&game.board, &level.rules).is_some(),
                    "{} opens with no legal move on seed {seed}",
                    level.name,
                );
            }
        }
    }

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
                    // Against the palette rather than the count, since a level
                    // may name any set of colors rather than the first few.
                    assert!(
                        level.rules.deals(*color),
                        "{} asks for color {color}, which it never deals",
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
