//! The grid itself: cells, gems, and gravity.

use crate::rng::Rng;
use crate::rules::{RefillMode, Rules};

/// A cell coordinate. Signed so neighbor arithmetic can run off the edge
/// and be rejected by `contains` instead of underflowing.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Pos {
    pub r: i32,
    pub c: i32,
}

impl Pos {
    pub const fn new(r: i32, c: i32) -> Self {
        Pos { r, c }
    }

    pub fn is_adjacent(self, other: Pos) -> bool {
        (self.r - other.r).abs() + (self.c - other.c).abs() == 1
    }
}

/// What a gem does when it is cleared.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Special {
    None,
    /// Clears its whole row.
    LineH,
    /// Clears its whole column.
    LineV,
    /// Clears its row and its column together.
    Cross,
    /// Clears every gem sharing a color with whatever it was swapped against.
    Rainbow,
    /// Holds its cell until the clear has finished resolving, then flies off
    /// and takes out one other gem. Nothing falls until it lands.
    Rocket,
}

impl Special {
    pub fn code(self) -> u8 {
        match self {
            Special::None => 0,
            Special::LineH => 1,
            Special::LineV => 2,
            Special::Cross => 3,
            Special::Rainbow => 4,
            Special::Rocket => 5,
        }
    }

    pub fn is_special(self) -> bool {
        self != Special::None
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Gem {
    pub color: u8,
    pub special: Special,
}

impl Gem {
    pub fn plain(color: u8) -> Self {
        Gem { color, special: Special::None }
    }
}

/// A cell's permanent shape. Walls are holes in the board outline: nothing
/// occupies them and nothing falls through them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Terrain {
    Open,
    Wall,
}

#[derive(Clone, Copy, Debug)]
pub struct Cell {
    pub terrain: Terrain,
    pub gem: Option<Gem>,
    /// Layers of jelly under the gem; clearing a gem here peels one layer.
    pub jelly: u8,
    /// How much brick is in the way: 0 none, 1 cracked, 2 whole.
    ///
    /// A brick is not a gem and not a wall. It holds a cell the way a wall
    /// does, so nothing swaps with it and nothing falls through it, but it can
    /// be broken: twice over, whole to cracked to gone. It has no color, takes
    /// no part in matching, and is never swapped or moved.
    pub brick: u8,
}

impl Cell {
    const OPEN: Cell = Cell { terrain: Terrain::Open, gem: None, jelly: 0, brick: 0 };
}

#[derive(Clone, Debug)]
pub struct Board {
    pub rows: i32,
    pub cols: i32,
    cells: Vec<Cell>,
}

impl Board {
    pub fn new(rows: i32, cols: i32) -> Self {
        Board { rows, cols, cells: vec![Cell::OPEN; (rows * cols) as usize] }
    }

    /// Builds a board from an ASCII sketch, one string per row:
    /// `.` open, `#` wall, `o` one layer of jelly, `O` two layers,
    /// `b` a cracked brick, `B` a whole one.
    ///
    /// Lower case is the lesser of the pair in both cases, the way `o` is one
    /// layer of jelly where `O` is two.
    ///
    /// Rows shorter than `cols` are padded with open cells, so ragged art
    /// still yields a rectangle.
    pub fn from_layout(layout: &[&str]) -> Self {
        let rows = layout.len() as i32;
        let cols = layout.iter().map(|row| row.chars().count()).max().unwrap_or(0) as i32;
        let mut board = Board::new(rows, cols);
        for (r, line) in layout.iter().enumerate() {
            for (c, ch) in line.chars().enumerate() {
                let pos = Pos::new(r as i32, c as i32);
                let cell = board.cell_mut(pos).expect("layout position is inside the board");
                match ch {
                    '#' => cell.terrain = Terrain::Wall,
                    'o' => cell.jelly = 1,
                    'O' => cell.jelly = 2,
                    'b' => cell.brick = 1,
                    'B' => cell.brick = 2,
                    _ => {}
                }
            }
        }
        board
    }

    pub fn contains(&self, p: Pos) -> bool {
        p.r >= 0 && p.c >= 0 && p.r < self.rows && p.c < self.cols
    }

    fn index(&self, p: Pos) -> usize {
        (p.r * self.cols + p.c) as usize
    }

    pub fn cell(&self, p: Pos) -> Option<&Cell> {
        if self.contains(p) {
            Some(&self.cells[self.index(p)])
        } else {
            None
        }
    }

    pub fn cell_mut(&mut self, p: Pos) -> Option<&mut Cell> {
        if self.contains(p) {
            let i = self.index(p);
            Some(&mut self.cells[i])
        } else {
            None
        }
    }

    /// True when a gem can occupy this cell at all.
    pub fn is_open(&self, p: Pos) -> bool {
        self.cell(p).map_or(false, |cell| cell.terrain == Terrain::Open)
    }

    pub fn gem(&self, p: Pos) -> Option<Gem> {
        self.cell(p).and_then(|cell| cell.gem)
    }

    pub fn color(&self, p: Pos) -> Option<u8> {
        self.gem(p).map(|g| g.color)
    }

    /// The color a match may be built from.
    ///
    /// Rockets and rainbows are items sitting on the board rather than gems in
    /// the pool of colors, and neither takes part in matching. A rocket is
    /// waiting to launch, and left matchable a cascade could clear it before it
    /// ever fires, quietly costing the player the reward they earned. A rainbow
    /// answers to any color, which is exactly why it belongs to none.
    pub fn match_color(&self, p: Pos) -> Option<u8> {
        match self.gem(p) {
            Some(gem) if gem.special != Special::Rocket && gem.special != Special::Rainbow => {
                Some(gem.color)
            }
            _ => None,
        }
    }

    pub fn set_gem(&mut self, p: Pos, gem: Option<Gem>) {
        if let Some(cell) = self.cell_mut(p) {
            cell.gem = gem;
        }
    }

    pub fn swap_gems(&mut self, a: Pos, b: Pos) {
        let ga = self.gem(a);
        let gb = self.gem(b);
        self.set_gem(a, gb);
        self.set_gem(b, ga);
    }

    pub fn jelly(&self, p: Pos) -> u8 {
        self.cell(p).map_or(0, |cell| cell.jelly)
    }

    /// Peels one jelly layer, reporting whether there was one to peel.
    pub fn peel_jelly(&mut self, p: Pos) -> bool {
        match self.cell_mut(p) {
            Some(cell) if cell.jelly > 0 => {
                cell.jelly -= 1;
                true
            }
            _ => false,
        }
    }

    pub fn jelly_remaining(&self) -> u32 {
        self.cells.iter().map(|cell| cell.jelly as u32).sum()
    }

    pub fn positions(&self) -> impl Iterator<Item = Pos> + '_ {
        let cols = self.cols;
        (0..self.rows * self.cols).map(move |i| Pos::new(i / cols, i % cols))
    }

    /// True when a gem could come to rest here: on the board, not a wall, and
    /// nothing in the way.
    ///
    /// The one question gravity, dealing and swapping all really ask. Note that
    /// an open cell is not always fillable: a board can leave holes nothing can
    /// reach, under a shelf of bricks.
    pub fn is_free(&self, p: Pos) -> bool {
        self.is_open(p) && self.gem(p).is_none() && self.brick(p) == 0
    }

    /// How much brick is in this cell: 0 none, 1 cracked, 2 whole.
    pub fn brick(&self, p: Pos) -> u8 {
        self.cell(p).map_or(0, |cell| cell.brick)
    }

    /// Knocks one hit off the brick here, reporting what is left of it.
    ///
    /// `None` when there was no brick to hit, `Some(0)` when that hit was the
    /// one that broke it.
    pub fn damage_brick(&mut self, p: Pos) -> Option<u8> {
        match self.cell_mut(p) {
            Some(cell) if cell.brick > 0 => {
                cell.brick -= 1;
                Some(cell.brick)
            }
            _ => None,
        }
    }

    pub fn bricks_remaining(&self) -> u32 {
        self.cells.iter().map(|cell| cell.brick as u32).sum()
    }

    /// Settles the board after a clear: gems fall into the holes below them and
    /// fresh gems enter from above.
    ///
    /// Gravity is not column by column. A gem with something under it will
    /// spill sideways into a gap rather than sit on a ledge: down and to the
    /// left first, then down and to the right. That is what lets a board with
    /// obstacles in it fill back up instead of stranding columns, and it is why
    /// this runs as repeated passes over the whole board rather than as one
    /// walk up each column.
    ///
    /// Returns, for every cell, the row and column its current gem started at:
    /// its own position when it did not move, and a negative row for a gem that
    /// has just entered from off the top. The renderer turns that into a fall;
    /// the board itself is already in its final state.
    pub fn settle_stage(&mut self, rules: &Rules, rng: &mut Rng) -> Option<Vec<(f32, f32)>> {
        let mut origin: Vec<(f32, f32)> =
            self.positions().map(|p| (p.r as f32, p.c as f32)).collect();
        // How many gems each refill point has already let in this stage, so
        // they queue above the board instead of arriving stacked on each other.
        let mut spawned = vec![0_i32; (self.rows * self.cols) as usize];
        let mut moved = false;

        // Every move puts a gem one row further down, so this cannot cycle; the
        // bound is only here so a mistake shows up as a wrong board rather than
        // as a hang.
        for _ in 0..(self.rows + self.cols) * 2 + 8 {
            let mut resting = true;
            // Bottom row first: a gem that moves out of the way this pass is
            // what lets the one above it move next pass, which is the stepping
            // that makes a fall read as a fall.
            for r in (0..self.rows).rev() {
                for c in 0..self.cols {
                    let from = Pos::new(r, c);
                    let below = Pos::new(r + 1, c);
                    if self.gem(from).is_none() || !self.is_free(below) {
                        continue;
                    }
                    self.shift(from, below, &mut origin);
                    resting = false;
                }
            }

            if rules.refill == RefillMode::TopSpawn {
                for mouth in self.refill_mouths() {
                    if !self.is_free(mouth) {
                        continue;
                    }
                    let i = self.index(mouth);
                    self.set_gem(mouth, Some(Gem::plain(rng.below(rules.colors as u32) as u8)));
                    origin[i] = ((mouth.r - 1 - spawned[i]) as f32, mouth.c as f32);
                    spawned[i] += 1;
                    resting = false;
                }
            }

            moved |= !resting;
            if resting {
                break;
            }
        }

        if moved {
            return Some(origin);
        }

        // Nothing can fall any further, so this is where whatever is perched on
        // a shelf slides off it. One step only: what it slides onto is usually
        // a drop, and that is the next stage rather than part of this one.
        for r in (0..self.rows).rev() {
            for c in 0..self.cols {
                let from = Pos::new(r, c);
                if self.gem(from).is_none() {
                    continue;
                }
                if let Some(to) = self.spill_for(from) {
                    self.shift(from, to, &mut origin);
                    moved = true;
                }
            }
        }

        moved.then_some(origin)
    }

    /// True when another call to [`Board::settle_stage`] would do something.
    ///
    /// Asked before a stage commits to its timing, so that the beat separating
    /// a landing from the clear it causes is spent once at the end rather than
    /// between every stage of the same settle.
    pub fn will_move(&self, rules: &Rules) -> bool {
        if rules.refill == RefillMode::TopSpawn
            && self.refill_mouths().iter().any(|p| self.is_free(*p))
        {
            return true;
        }
        self.positions().any(|p| {
            self.gem(p).is_some()
                && (self.is_free(Pos::new(p.r + 1, p.c)) || self.spill_for(p).is_some())
        })
    }

    fn shift(&mut self, from: Pos, to: Pos, origin: &mut [(f32, f32)]) {
        let gem = self.gem(from);
        self.set_gem(to, gem);
        self.set_gem(from, None);
        let (i, j) = (self.index(from), self.index(to));
        origin[j] = origin[i];
        origin[i] = (from.r as f32, from.c as f32);
    }

    /// Where the gem at `p` slides off to, if it slides at all.
    ///
    /// Only ever for a gem that cannot fall: down and to the left for
    /// preference, then down and to the right.
    ///
    /// It slides into any free gap, and needs no test for whether the column
    /// above that gap might have filled it instead, because by the time this is
    /// asked the answer is always no. Spilling only happens once straight-down
    /// dropping is exhausted, and a cell still free at that point cannot be fed
    /// from above: a gem over it would already have dropped into it, and a
    /// clear run up to a refill mouth would already have spawned into it. What
    /// is left is capped by a wall or a brick, however far up the cap sits.
    ///
    /// That ordering is doing the work that a one-cell check for an overhang
    /// cannot. The cell over a gap in the middle of a brick shelf is itself an
    /// ordinary empty cell, so looking only at that one says the column is
    /// still coming, when the bricks two rows up mean nothing is.
    fn spill_for(&self, p: Pos) -> Option<Pos> {
        if self.is_free(Pos::new(p.r + 1, p.c)) {
            return None;
        }
        [p.c - 1, p.c + 1]
            .into_iter()
            .map(|c| Pos::new(p.r + 1, c))
            .find(|side| self.is_free(*side))
    }

    /// The cells fresh gems enter through: the top of each run of open cells in
    /// a column.
    ///
    /// A column split by walls is several tubes, and a tube with a wall over it
    /// cannot be fed from off the board, so each one is fed from its own
    /// ceiling. Gems can now also spill into a tube from the side, but a tube
    /// with no neighbor to spill from would otherwise never fill at all.
    fn refill_mouths(&self) -> Vec<Pos> {
        let mut mouths = Vec::new();
        for c in 0..self.cols {
            for r in 0..self.rows {
                let here = Pos::new(r, c);
                if self.is_open(here) && !self.is_open(Pos::new(r - 1, c)) {
                    mouths.push(here);
                }
            }
        }
        mouths
    }

    /// Every open cell currently holding a gem.
    pub fn occupied(&self) -> Vec<Pos> {
        self.positions().filter(|p| self.gem(*p).is_some()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Rules;

    fn filled(rows: i32, cols: i32, color: u8) -> Board {
        let mut board = Board::new(rows, cols);
        for p in board.positions().collect::<Vec<_>>() {
            board.set_gem(p, Some(Gem::plain(color)));
        }
        board
    }

    /// Settles a board the whole way, which the game does across as many
    /// animated stages as it takes. Hands back the origins of the last stage
    /// that actually moved something.
    fn settle_all(board: &mut Board, rules: &Rules, rng: &mut Rng) -> Vec<(f32, f32)> {
        let mut last: Vec<(f32, f32)> =
            board.positions().map(|p| (p.r as f32, p.c as f32)).collect();
        while let Some(origin) = board.settle_stage(rules, rng) {
            last = origin;
        }
        last
    }

    #[test]
    fn a_board_with_nothing_in_the_way_never_moves_a_gem_sideways() {
        // The column has priority, so on a plain rectangle nothing should ever
        // spill: every hole is fed from above, and by the time spilling is
        // asked about there is no hole left. Said as "no gem changes column",
        // which is the thing that would be visible if it were wrong.
        let rules = Rules { rows: 8, cols: 6, ..Rules::default() };
        for seed in 0..60 {
            let mut rng = Rng::new(seed);
            let mut board = Board::new(8, 6);
            for p in board.positions().collect::<Vec<_>>() {
                board.set_gem(p, Some(Gem::plain(rng.below(5) as u8)));
            }
            // Roughly a third punched out, a far rougher shape than any real
            // clear leaves behind.
            for p in board.positions().collect::<Vec<_>>() {
                if rng.below(3) == 0 {
                    board.set_gem(p, None);
                }
            }

            let origin = settle_all(&mut board, &rules, &mut Rng::new(seed));
            for p in board.positions() {
                let from = origin[(p.r * board.cols + p.c) as usize];
                assert_eq!(
                    from.1, p.c as f32,
                    "seed {seed}: the gem at {p:?} arrived from column {}",
                    from.1
                );
            }
            assert!(
                board.positions().all(|p| board.gem(p).is_some()),
                "seed {seed} left a hole on a board with nothing in the way",
            );
        }
    }

    #[test]
    fn a_gem_spills_off_a_shelf_into_the_gap_beside_it() {
        // (1,0) is resting on (2,0) and cannot go down. (2,1) is free, and with
        // no refill nothing is ever coming down to fill it.
        let mut board = Board::from_layout(&["...", ".#.", "..."]);
        board.set_gem(Pos::new(1, 0), Some(Gem::plain(1)));
        board.set_gem(Pos::new(2, 0), Some(Gem::plain(2)));

        let rules = Rules { rows: 3, cols: 3, refill: RefillMode::None, ..Rules::default() };
        let origin = settle_all(&mut board, &rules, &mut Rng::new(1));

        assert!(board.gem(Pos::new(1, 0)).is_none(), "it should not still be on the shelf");
        assert_eq!(board.gem(Pos::new(2, 1)).map(|g| g.color), Some(1), "it spilled right");
        assert_eq!(
            origin[(2 * 3 + 1) as usize],
            (1.0, 0.0),
            "and it should be animated from where it actually came from",
        );
    }

    #[test]
    fn a_pocket_under_a_brick_shelf_fills_from_both_ends() {
        // A four-wide shelf with a two-deep pocket under it. Nothing can reach
        // the pocket from above, so the only way in is off the ends of the
        // shelf, and gems have to keep walking in one step at a time.
        //
        // The cells directly under the middle of the shelf can never be
        // reached at all: everything over them, and diagonally over them, is
        // brick. The row below that is reachable from the ends.
        let mut board = Board::from_layout(&[
            "......",
            ".BBBB.",
            "......",
            "......",
        ]);
        let rules = Rules { rows: 4, cols: 6, colors: 4, ..Rules::default() };
        settle_all(&mut board, &rules, &mut Rng::new(7));

        for c in 1..5 {
            assert!(
                board.gem(Pos::new(3, c)).is_some(),
                "the floor under the shelf should have filled from the ends, missing {c}",
            );
        }
        // The ends of the row under the shelf are reachable diagonally from
        // outside it; the middle two are walled in by brick on every side a gem
        // could arrive from.
        assert!(board.gem(Pos::new(2, 1)).is_some(), "under the left end of the shelf");
        assert!(board.gem(Pos::new(2, 4)).is_some(), "under the right end of the shelf");
        assert!(board.gem(Pos::new(2, 2)).is_none(), "nothing can reach the middle");
        assert!(board.gem(Pos::new(2, 3)).is_none(), "nothing can reach the middle");
    }

    #[test]
    fn layout_marks_walls_and_jelly() {
        let board = Board::from_layout(&["..#", "oO."]);
        assert_eq!(board.rows, 2);
        assert_eq!(board.cols, 3);
        assert!(!board.is_open(Pos::new(0, 2)));
        assert!(board.is_open(Pos::new(0, 0)));
        assert_eq!(board.jelly(Pos::new(1, 0)), 1);
        assert_eq!(board.jelly(Pos::new(1, 1)), 2);
        assert_eq!(board.jelly_remaining(), 3);
    }

    #[test]
    fn collapse_drops_gems_and_refills_from_above() {
        let rules = Rules { rows: 4, cols: 1, ..Rules::default() };
        let mut board = filled(4, 1, 0);
        board.set_gem(Pos::new(3, 0), None);
        board.set_gem(Pos::new(2, 0), None);

        let mut rng = Rng::new(1);
        let origin = settle_all(&mut board, &rules, &mut rng);

        assert!(board.positions().all(|p| board.gem(p).is_some()), "column is full again");
        // The survivors from rows 0 and 1 are now at the bottom.
        assert_eq!(origin[3], (1.0, 0.0));
        assert_eq!(origin[2], (0.0, 0.0));
        // The two newcomers came from off the top of the board.
        assert_eq!(origin[1], (-1.0, 0.0));
        assert_eq!(origin[0], (-2.0, 0.0));
    }

    #[test]
    fn walls_split_a_column_into_independent_tubes() {
        let mut board = Board::from_layout(&[".", ".", "#", "."]);
        for p in board.positions().collect::<Vec<_>>() {
            if board.is_open(p) {
                board.set_gem(p, Some(Gem::plain(0)));
            }
        }
        // Empty the cell directly under the wall.
        board.set_gem(Pos::new(3, 0), None);

        let rules = Rules { rows: 4, cols: 1, ..Rules::default() };
        let mut rng = Rng::new(2);
        let origin = settle_all(&mut board, &rules, &mut rng);

        assert!(board.gem(Pos::new(3, 0)).is_some(), "the lower tube refills itself");
        assert_eq!(origin[3], (2.0, 0.0), "its gem entered from just above its own ceiling");
        assert!(board.gem(Pos::new(2, 0)).is_none(), "the wall stays empty");
        assert_eq!(origin[0], (0.0, 0.0), "the upper tube was already packed");
    }

    #[test]
    fn no_refill_mode_leaves_holes() {
        let rules = Rules {
            rows: 3,
            cols: 1,
            refill: RefillMode::None,
            ..Rules::default()
        };
        let mut board = filled(3, 1, 0);
        board.set_gem(Pos::new(2, 0), None);

        let mut rng = Rng::new(3);
        settle_all(&mut board, &rules, &mut rng);

        assert!(board.gem(Pos::new(0, 0)).is_none(), "nothing entered from above");
        assert!(board.gem(Pos::new(2, 0)).is_some(), "the survivors still fell");
    }
}
