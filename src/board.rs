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
    /// Clears the 3x3 block around it.
    Bomb,
    /// Clears every gem sharing a color with whatever it was swapped against.
    Rainbow,
}

impl Special {
    pub fn code(self) -> u8 {
        match self {
            Special::None => 0,
            Special::LineH => 1,
            Special::LineV => 2,
            Special::Bomb => 3,
            Special::Rainbow => 4,
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
}

impl Cell {
    const OPEN: Cell = Cell { terrain: Terrain::Open, gem: None, jelly: 0 };
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
    /// `.` open, `#` wall, `o` one layer of jelly, `O` two layers.
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

    /// Settles the board after a clear: gems fall into the holes below them and
    /// fresh gems enter from above.
    ///
    /// Returns, for every cell, the row its current gem started at — the same
    /// row when it did not move, a negative row for a gem that just spawned.
    /// The renderer turns that into a fall animation; the board itself is
    /// already in its final state.
    pub fn collapse(&mut self, rules: &Rules, rng: &mut Rng) -> Vec<f32> {
        let mut origin: Vec<f32> = self
            .positions()
            .map(|p| p.r as f32)
            .collect();

        for c in 0..self.cols {
            // A column of walls is several independent tubes; each one packs
            // down onto its own floor and refills from its own ceiling.
            let mut r = self.rows - 1;
            while r >= 0 {
                if !self.is_open(Pos::new(r, c)) {
                    r -= 1;
                    continue;
                }
                let bottom = r;
                let mut top = r;
                while top - 1 >= 0 && self.is_open(Pos::new(top - 1, c)) {
                    top -= 1;
                }
                self.collapse_segment(c, top, bottom, rules, rng, &mut origin);
                r = top - 1;
            }
        }

        origin
    }

    fn collapse_segment(
        &mut self,
        c: i32,
        top: i32,
        bottom: i32,
        rules: &Rules,
        rng: &mut Rng,
        origin: &mut [f32],
    ) {
        // Walk upward, pulling each surviving gem down to the next free slot.
        let mut write = bottom;
        let mut read = bottom;
        while read >= top {
            let from = Pos::new(read, c);
            if let Some(gem) = self.gem(from) {
                let to = Pos::new(write, c);
                if write != read {
                    self.set_gem(to, Some(gem));
                    self.set_gem(from, None);
                }
                origin[self.index(to)] = read as f32;
                write -= 1;
            }
            read -= 1;
        }

        if rules.refill == RefillMode::None {
            return;
        }

        // Everything left above the write head is new, entering from off-board.
        let mut spawned = 0;
        while write >= top {
            let to = Pos::new(write, c);
            self.set_gem(to, Some(Gem::plain(rng.below(rules.colors as u32) as u8)));
            origin[self.index(to)] = (top - 1 - spawned) as f32;
            write -= 1;
            spawned += 1;
        }
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
        let origin = board.collapse(&rules, &mut rng);

        assert!(board.positions().all(|p| board.gem(p).is_some()), "column is full again");
        // The survivors from rows 0 and 1 are now at the bottom.
        assert_eq!(origin[3], 1.0);
        assert_eq!(origin[2], 0.0);
        // The two newcomers came from off the top of the board.
        assert_eq!(origin[1], -1.0);
        assert_eq!(origin[0], -2.0);
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
        let origin = board.collapse(&rules, &mut rng);

        assert!(board.gem(Pos::new(3, 0)).is_some(), "the lower tube refills itself");
        assert_eq!(origin[3], 2.0, "its gem entered from just above its own ceiling");
        assert!(board.gem(Pos::new(2, 0)).is_none(), "the wall stays empty");
        assert_eq!(origin[0], 0.0, "the upper tube was already packed");
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
        board.collapse(&rules, &mut rng);

        assert!(board.gem(Pos::new(0, 0)).is_none(), "nothing entered from above");
        assert!(board.gem(Pos::new(2, 0)).is_some(), "the survivors still fell");
    }
}
