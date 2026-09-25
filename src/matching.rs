//! Finding matches, awarding specials, and resolving chain reactions.

use crate::board::{Board, Pos, Special};
use crate::rng::Rng;
use crate::rules::{Rules, SpecialSet, MAX_COLORS};

/// One connected clump of matched gems. Overlapping shapes (an L, a T, a 2x2
/// with a run hanging off it) arrive as a single group, so a shape earns one
/// special rather than one per run.
#[derive(Clone, Debug)]
pub struct MatchGroup {
    pub cells: Vec<Pos>,
    pub color: u8,
    /// Longest horizontal run inside the group, 0 if none reached `min_match`.
    pub h_run: i32,
    pub v_run: i32,
    /// How many 2x2 blocks the group contains.
    pub squares: u32,
    /// Where a special created by this group should land.
    pub pivot: Pos,
}

impl MatchGroup {
    /// The special this shape earns, if any.
    ///
    /// A run outranks a square. A 2x2 on its own leaves a rocket, but if the
    /// same clump also earns a line gem, a cross or a rainbow, that is what the
    /// player gets: the rocket is the consolation prize, not the trophy.
    ///
    /// The line gems run against the grain on purpose. To finish a row of four
    /// you slide a gem in from above or below, so the gem you are left with
    /// clears in the direction you were moving: down the column, not along the
    /// row you just completed.
    pub fn award(&self, specials: &SpecialSet) -> Special {
        let longest = self.h_run.max(self.v_run);
        let from_runs = if specials.rainbow && longest >= 5 {
            Special::Rainbow
        } else if specials.cross && self.h_run >= 3 && self.v_run >= 3 {
            Special::Cross
        } else if longest == 4 {
            // The gem clears across the run rather than along it, so a row of
            // four leaves a clearer that fires down its column. Each direction
            // is its own unlock, so one orientation can be worth a gem while
            // the same run turned ninety degrees is worth nothing.
            let (made, allowed) = if self.h_run >= self.v_run {
                (Special::LineV, specials.line_v)
            } else {
                (Special::LineH, specials.line_h)
            };
            if allowed {
                made
            } else {
                Special::None
            }
        } else {
            Special::None
        };

        if from_runs != Special::None {
            return from_runs;
        }
        if self.squares > 0 && specials.rocket {
            Special::Rocket
        } else {
            Special::None
        }
    }
}

/// A run or a square found during the scan.
struct Shape {
    cells: Vec<Pos>,
    color: u8,
    kind: ShapeKind,
}

#[derive(Clone, Copy, PartialEq)]
enum ShapeKind {
    Row,
    Column,
    Square,
}

/// Scans the whole board for runs of at least `rules.min_match` and, when the
/// rules allow it, 2x2 blocks; overlapping shapes are merged into one group.
pub fn find_matches(board: &Board, rules: &Rules) -> Vec<MatchGroup> {
    let mut shapes: Vec<Shape> = Vec::new();

    for r in 0..board.rows {
        collect_runs(board, rules.min_match, &mut shapes, true, r);
    }
    for c in 0..board.cols {
        collect_runs(board, rules.min_match, &mut shapes, false, c);
    }
    if rules.square_match {
        collect_squares(board, &mut shapes);
    }

    // Shapes that share a cell are one clump.
    let mut parent: Vec<usize> = (0..shapes.len()).collect();
    let mut owner: Vec<Option<usize>> = vec![None; (board.rows * board.cols) as usize];
    for (i, shape) in shapes.iter().enumerate() {
        for cell in &shape.cells {
            let slot = (cell.r * board.cols + cell.c) as usize;
            match owner[slot] {
                Some(j) => union(&mut parent, i, j),
                None => owner[slot] = Some(i),
            }
        }
    }

    let mut groups: Vec<MatchGroup> = Vec::new();
    let mut group_of: Vec<Option<usize>> = vec![None; shapes.len()];
    for (i, shape) in shapes.iter().enumerate() {
        let root = find(&mut parent, i);
        let index = match group_of[root] {
            Some(g) => g,
            None => {
                groups.push(MatchGroup {
                    cells: Vec::new(),
                    color: shape.color,
                    h_run: 0,
                    v_run: 0,
                    squares: 0,
                    pivot: shape.cells[0],
                });
                group_of[root] = Some(groups.len() - 1);
                groups.len() - 1
            }
        };
        let group = &mut groups[index];
        let len = shape.cells.len() as i32;
        match shape.kind {
            ShapeKind::Row => group.h_run = group.h_run.max(len),
            ShapeKind::Column => group.v_run = group.v_run.max(len),
            ShapeKind::Square => group.squares += 1,
        }
        for cell in &shape.cells {
            if !group.cells.contains(cell) {
                group.cells.push(*cell);
            }
        }
    }

    for group in &mut groups {
        group.pivot = natural_pivot(group);
    }
    groups
}

/// Walks one row (or column) accumulating same-colored stretches.
fn collect_runs(board: &Board, min: i32, shapes: &mut Vec<Shape>, horizontal: bool, line: i32) {
    let length = if horizontal { board.cols } else { board.rows };
    let mut start = 0;
    while start < length {
        let pos = |i: i32| if horizontal { Pos::new(line, i) } else { Pos::new(i, line) };
        let color = match board.match_color(pos(start)) {
            Some(color) => color,
            None => {
                start += 1;
                continue;
            }
        };
        let mut end = start + 1;
        while end < length && board.match_color(pos(end)) == Some(color) {
            end += 1;
        }
        if end - start >= min {
            shapes.push(Shape {
                cells: (start..end).map(pos).collect(),
                color,
                kind: if horizontal { ShapeKind::Row } else { ShapeKind::Column },
            });
        }
        start = end;
    }
}

/// Every 2x2 block of one color, by its top-left corner.
fn collect_squares(board: &Board, shapes: &mut Vec<Shape>) {
    for r in 0..board.rows - 1 {
        for c in 0..board.cols - 1 {
            let corner = Pos::new(r, c);
            let color = match board.match_color(corner) {
                Some(color) => color,
                None => continue,
            };
            let cells = [
                corner,
                Pos::new(r, c + 1),
                Pos::new(r + 1, c),
                Pos::new(r + 1, c + 1),
            ];
            if cells.iter().all(|cell| board.match_color(*cell) == Some(color)) {
                shapes.push(Shape { cells: cells.to_vec(), color, kind: ShapeKind::Square });
            }
        }
    }
}

/// Where a created special lands when the player's own swap is not part of the
/// group: the junction of an L or T, the far corner of a square, else the
/// middle of the run.
fn natural_pivot(group: &MatchGroup) -> Pos {
    if group.squares > 0 {
        return *group
            .cells
            .iter()
            .max_by_key(|cell| (cell.r, cell.c))
            .expect("a group always has cells");
    }
    if group.h_run >= 3 && group.v_run >= 3 {
        for cell in &group.cells {
            let horizontal_neighbor = group
                .cells
                .iter()
                .any(|o| o.r == cell.r && (o.c - cell.c).abs() == 1);
            let vertical_neighbor = group
                .cells
                .iter()
                .any(|o| o.c == cell.c && (o.r - cell.r).abs() == 1);
            if horizontal_neighbor && vertical_neighbor {
                return *cell;
            }
        }
    }
    group.cells[group.cells.len() / 2]
}

fn find(parent: &mut Vec<usize>, mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

fn union(parent: &mut Vec<usize>, a: usize, b: usize) {
    let (ra, rb) = (find(parent, a), find(parent, b));
    if ra != rb {
        parent[rb] = ra;
    }
}

/// Reads the board as if `a` and `b` had traded places, without touching it.
pub struct SwapView<'a> {
    pub board: &'a Board,
    pub a: Pos,
    pub b: Pos,
}

impl<'a> SwapView<'a> {
    /// The color a match could be built from here after the swap.
    ///
    /// The matching color rather than the raw one, so this answers the same
    /// question [`find_matches`] will: whatever is predicted here has to be
    /// what actually happens, and a rocket, a rainbow and an Archipelago gem
    /// all carry a color byte that takes no part in matching. An Archipelago
    /// gem is the one that bites, because every one of them carries the same
    /// [`NO_COLOR`](crate::board::NO_COLOR): read raw, two of them beside a
    /// third would look like a run.
    pub fn color(&self, p: Pos) -> Option<u8> {
        if p == self.a {
            self.board.match_color(self.b)
        } else if p == self.b {
            self.board.match_color(self.a)
        } else {
            self.board.match_color(p)
        }
    }
}

/// Whether the gem that would sit at `p` after the swap lands in a run.
fn forms_match(view: &SwapView, p: Pos, min: i32) -> bool {
    let color = match view.color(p) {
        Some(color) => color,
        None => return false,
    };
    let reach = |dr: i32, dc: i32| {
        let mut n = 0;
        let mut cur = Pos::new(p.r + dr, p.c + dc);
        while view.board.contains(cur) && view.color(cur) == Some(color) {
            n += 1;
            cur = Pos::new(cur.r + dr, cur.c + dc);
        }
        n
    };
    1 + reach(0, -1) + reach(0, 1) >= min || 1 + reach(-1, 0) + reach(1, 0) >= min
}

/// Whether the gem that would sit at `p` completes a 2x2 block. Checks all four
/// blocks that touch the cell, since the gem could be any corner of one.
fn forms_square(view: &SwapView, p: Pos) -> bool {
    let color = match view.color(p) {
        Some(color) => color,
        None => return false,
    };
    for dr in [-1, 0] {
        for dc in [-1, 0] {
            let corner = Pos::new(p.r + dr, p.c + dc);
            let cells = [
                corner,
                Pos::new(corner.r, corner.c + 1),
                Pos::new(corner.r + 1, corner.c),
                Pos::new(corner.r + 1, corner.c + 1),
            ];
            if cells
                .iter()
                .all(|cell| view.board.contains(*cell) && view.color(*cell) == Some(color))
            {
                return true;
            }
        }
    }
    false
}

/// Whether swapping these two cells is a legal move.
///
/// Specials are inert against ordinary gems: a line gem or a cross goes off
/// when a match of its color sweeps it up, not because it was pushed around.
/// Two of them swapped together is a different matter: they set each other
/// off. And the rainbow has no match of its own to wait for, so swapping it
/// against anything is how it fires.
pub fn is_useful_swap(board: &Board, rules: &Rules, a: Pos, b: Pos) -> bool {
    if !a.is_adjacent(b) || !board.is_open(a) || !board.is_open(b) {
        return false;
    }
    let (ga, gb) = match (board.gem(a), board.gem(b)) {
        (Some(ga), Some(gb)) => (ga, gb),
        _ => return false,
    };
    // Two Archipelago gems take every one on the board, and so does one
    // against a rainbow, which the next line covers. Against an ordinary gem
    // it is an ordinary move: the gem it changes places with may land in a
    // match, and the check below is what sees that. The gem itself never does,
    // because it has no matching color.
    if ga.special == Special::Archipelago && gb.special == Special::Archipelago {
        return true;
    }
    if ga.special == Special::Rainbow || gb.special == Special::Rainbow {
        return true;
    }
    if ga.special.is_special() && gb.special.is_special() {
        return true;
    }
    let view = SwapView { board, a, b };
    if forms_match(&view, a, rules.min_match) || forms_match(&view, b, rules.min_match) {
        return true;
    }
    rules.square_match && (forms_square(&view, a) || forms_square(&view, b))
}

/// The first legal move on the board, scanning top-left to bottom-right.
/// `None` means the board is stuck and needs a shuffle.
pub fn find_move(board: &Board, rules: &Rules) -> Option<(Pos, Pos)> {
    for p in board.positions() {
        for q in [Pos::new(p.r, p.c + 1), Pos::new(p.r + 1, p.c)] {
            if board.contains(q) && is_useful_swap(board, rules, p, q) {
                return Some((p, q));
            }
        }
    }
    None
}

/// Every move the board allows, rather than the first one found.
///
/// Each pair once: a swap is the same swap from either end, so only the
/// neighbor to the right and the one below are tried, the same way
/// [`find_move`] walks. The order is the board's, top left to bottom right,
/// which is exactly why nothing should show one of these to a player without
/// choosing between them first.
pub fn legal_moves(board: &Board, rules: &Rules) -> Vec<(Pos, Pos)> {
    let mut moves = Vec::new();
    for p in board.positions() {
        for q in [Pos::new(p.r, p.c + 1), Pos::new(p.r + 1, p.c)] {
            if board.contains(q) && is_useful_swap(board, rules, p, q) {
                moves.push((p, q));
            }
        }
    }
    moves
}

/// What a special does when it goes off. A rocket does nothing here: it waits
/// for the clear to finish and then flies, which the game drives as its own
/// phase.
pub fn blast(board: &Board, p: Pos, special: Special, rainbow_color: u8, out: &mut Vec<Pos>) {
    match special {
        // An Archipelago gem is a check, not a charge: clearing it hands over
        // what it was hiding and takes nothing else with it.
        Special::None | Special::Rocket | Special::Archipelago => {}
        Special::LineH => {
            for c in 0..board.cols {
                out.push(Pos::new(p.r, c));
            }
        }
        Special::LineV => {
            for r in 0..board.rows {
                out.push(Pos::new(r, p.c));
            }
        }
        Special::Cross => {
            for c in 0..board.cols {
                out.push(Pos::new(p.r, c));
            }
            for r in 0..board.rows {
                out.push(Pos::new(r, p.c));
            }
        }
        Special::Rainbow => {
            for q in board.positions() {
                if board.color(q) == Some(rainbow_color) {
                    out.push(q);
                }
            }
        }
    }
}

/// The color a rainbow gem picks when it is caught in a blast rather than
/// swapped deliberately: whichever color is most common, so it pays off.
pub fn most_common_color(board: &Board, rng: &mut Rng) -> u8 {
    let mut counts = [0u32; MAX_COLORS];
    for p in board.positions() {
        if let Some(color) = board.color(p) {
            if (color as usize) < MAX_COLORS {
                counts[color as usize] += 1;
            }
        }
    }
    // Drawn from the colors that tie rather than taking the lowest of them.
    // Two colors level on a board is common, and always answering with the
    // same one of them makes a rainbow feel rigged.
    let most = counts.iter().copied().max().unwrap_or(0);
    let tied: Vec<u8> =
        (0..MAX_COLORS).filter(|c| counts[*c] == most).map(|c| c as u8).collect();
    tied[rng.below(tied.len() as u32) as usize]
}

/// How long each cell of a blast waits before it pops, per cell of distance
/// from whatever set it off. A row clearer sweeps outward rather than taking
/// the whole row at once, and because the delay accumulates through a chain,
/// one special setting off another sends the clear traveling across the board.
///
/// At this pace a row sweeps in about a third of a second, and a chain of three
/// specials takes most of a second to play out.
pub const SPREAD_STEP_MS: f32 = 50.0;

/// A rainbow takes a whole color at once, scattered all over the board, so
/// there is no direction for it to spread in. Its cells go off at random within
/// this window instead, which reads as the color crackling out rather than
/// vanishing in one frame.
pub const RAINBOW_SPREAD_MS: f32 = 420.0;

/// The full set of cells a clear takes with it.
pub struct Detonation {
    /// Every cell that ends up cleared, in the order it was reached.
    pub cleared: Vec<Pos>,
    /// When each cleared cell pops, in milliseconds from the start of the
    /// clear. Parallel to `cleared`.
    pub delays: Vec<f32>,
    /// Whether each cleared cell hits the bricks beside it. Parallel to
    /// `cleared`. True for gems a match took and for a color a rainbow swept
    /// up, false for whatever a beam ran over on its way across the board.
    pub cracks: Vec<bool>,
    /// The specials that went off, in firing order.
    pub fired: Vec<(Pos, Special)>,
    /// Bricks a clearing gem's beam passed through.
    ///
    /// A beam does not stop at a brick, it goes through it and marks it on the
    /// way. Brick cells hold no gem, so they are not in `cleared` and would
    /// otherwise be invisible to the caller.
    pub struck: Vec<Pos>,
}

/// Accumulates a blast: which cells it takes, and when each one goes.
struct Wave {
    marked: Vec<bool>,
    cols: i32,
    cleared: Vec<Pos>,
    delays: Vec<f32>,
    cracks: Vec<bool>,
    queue: Vec<(Pos, f32)>,
}

impl Wave {
    fn new(board: &Board) -> Self {
        Wave {
            marked: vec![false; (board.rows * board.cols) as usize],
            cols: board.cols,
            cleared: Vec::new(),
            delays: Vec::new(),
            cracks: Vec::new(),
            queue: Vec::new(),
        }
    }

    /// Claims a cell for the blast. The first claim wins, so a cell caught by
    /// two blasts pops on the earlier one.
    ///
    /// `cracks` says whether this gem going away hits the bricks beside it: a
    /// gem taken by a match or swept up by a rainbow does, a gem a beam simply
    /// ran over does not.
    ///
    /// `seeded` says the cell was named by whatever started this, rather than
    /// swept up along the way. It is what spares a rocket. A rocket is holding
    /// its cell until it launches, and a beam crossing it would take away the
    /// reward the player has already earned without it ever firing, which is
    /// the same reason a rocket takes no part in matching. Named directly it
    /// still goes, because that is somebody choosing to spend it.
    fn push(&mut self, board: &Board, p: Pos, delay: f32, cracks: bool, seeded: bool) {
        if !board.contains(p) || board.gem(p).is_none() {
            return;
        }
        if !seeded && board.gem(p).map_or(false, |gem| gem.special == Special::Rocket) {
            return;
        }
        let slot = (p.r * self.cols + p.c) as usize;
        if self.marked[slot] {
            return;
        }
        self.marked[slot] = true;
        self.cleared.push(p);
        self.delays.push(delay);
        self.cracks.push(cracks);
        self.queue.push((p, delay));
    }
}

/// How far into a blast a given cell sits, measured along the shape the
/// special actually clears, so the pops travel outward the way the blast does.
fn spread_delay(origin: Pos, cell: Pos, special: Special) -> f32 {
    let steps = match special {
        Special::LineH => (cell.c - origin.c).abs(),
        Special::LineV => (cell.r - origin.r).abs(),
        Special::Cross => {
            if cell.r == origin.r {
                (cell.c - origin.c).abs()
            } else {
                (cell.r - origin.r).abs()
            }
        }
        // A rainbow is handled by its caller, which scatters it at random.
        _ => 0,
    };
    steps as f32 * SPREAD_STEP_MS
}

/// Grows an initial clear into its chain reaction: specials caught in the blast
/// fire in turn, and so do the ones they catch.
///
/// `seed_jitter_ms` scatters the starting cells in time rather than popping
/// them together, which is what a rainbow wants; an ordinary match passes 0 so
/// its three gems go as one.
/// Cells in `spent` still pop, and still pop as whatever they are, but do not
/// fire: they are specials whose power the caller has already accounted for.
/// Lays one special's blast into the wave: the cells it takes, when each of
/// them goes, and the bricks its beam passes through on the way.
///
/// Shared by the specials the wave finds on the board and the ones named by
/// `fires`, so the two cannot come out differently. They did: a caller that
/// worked the shape out for itself and handed the cells in as seeds got a
/// clear that took the gems and nothing else, because the bricks are marked
/// here and nowhere else.
#[allow(clippy::too_many_arguments)]
fn lay_blast(
    board: &Board,
    wave: &mut Wave,
    struck: &mut Vec<Pos>,
    hits: &mut Vec<Pos>,
    rng: &mut Rng,
    origin: Pos,
    special: Special,
    delay: f32,
    fallback: u8,
) {
    hits.clear();
    blast(board, origin, special, fallback, hits);
    for hit in std::mem::take(hits) {
        // A beam goes through a brick rather than stopping at it, marking
        // it in passing. A rainbow has no beam, only a list of cells of one
        // color, so it never strikes a brick this way.
        if special != Special::Rainbow && board.brick(hit) > 0 && !struck.contains(&hit) {
            struck.push(hit);
        }
        // A blast starts when the gem that carried it pops, and spreads
        // outward from there, except a rainbow, whose cells are scattered
        // and so go off in no particular order.
        let step = if special == Special::Rainbow {
            rng.below(RAINBOW_SPREAD_MS as u32) as f32
        } else {
            spread_delay(origin, hit, special)
        };
        // A rainbow takes a color wherever it is, which is a clear like
        // any other and hits what it is next to. A beam is not: it is a
        // line drawn across the board, and a gem it happens to run over is
        // not a match. The beam marks the bricks it passes through itself,
        // just above, and that is the whole of its effect on them.
        wave.push(board, hit, delay + step, special == Special::Rainbow, false);
    }
}

/// `fires` sets a special off at a cell that is not carrying one, which is what
/// spending one out of the inventory is: the beam is real, but there was never
/// a gem on the board to hold it.
pub fn detonate(
    board: &Board,
    seeds: &[Pos],
    spent: &[Pos],
    fires: &[(Pos, Special)],
    rng: &mut Rng,
    seed_jitter_ms: f32,
) -> Detonation {
    let mut wave = Wave::new(board);
    let mut fired: Vec<(Pos, Special)> = Vec::new();
    let mut struck: Vec<Pos> = Vec::new();

    for seed in seeds {
        let delay = if seed_jitter_ms > 0.0 {
            rng.below(seed_jitter_ms as u32) as f32
        } else {
            0.0
        };
        // Seeds are the cells a match took, or the color a rainbow was spent
        // on. Both are gems going away because the player lined something up,
        // so both hit the bricks beside them.
        wave.push(board, *seed, delay, true, true);
    }

    let fallback = most_common_color(board, rng);

    let mut hits: Vec<Pos> = Vec::new();

    // The ones nothing on the board is carrying, laid in before the queue is
    // walked so their cells are in the wave from the start. After the fallback
    // color rather than before, so a clear with none of these draws from the
    // generator exactly as it always did.
    for (p, special) in fires {
        fired.push((*p, *special));
        lay_blast(board, &mut wave, &mut struck, &mut hits, rng, *p, *special, 0.0, fallback);
    }

    let mut head = 0;
    while head < wave.queue.len() {
        let (p, delay) = wave.queue[head];
        head += 1;
        let special = match board.gem(p) {
            Some(gem) => gem.special,
            None => continue,
        };
        if !special.is_special() || special == Special::Rocket {
            continue;
        }
        if spent.contains(&p) {
            continue;
        }
        fired.push((p, special));
        lay_blast(board, &mut wave, &mut struck, &mut hits, rng, p, special, delay, fallback);
    }

    Detonation { cleared: wave.cleared, delays: wave.delays, cracks: wave.cracks, fired, struck }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Gem;

    /// Builds a board from rows of digits, where each digit is a color and `#`
    /// is a wall.
    fn board_of(rows: &[&str]) -> Board {
        let layout: Vec<String> = rows
            .iter()
            .map(|row| row.chars().map(|ch| if ch == '#' { '#' } else { '.' }).collect())
            .collect();
        let refs: Vec<&str> = layout.iter().map(|s| s.as_str()).collect();
        let mut board = Board::from_layout(&refs);
        for (r, row) in rows.iter().enumerate() {
            for (c, ch) in row.chars().enumerate() {
                if let Some(color) = ch.to_digit(10) {
                    board.set_gem(Pos::new(r as i32, c as i32), Some(Gem::plain(color as u8)));
                }
            }
        }
        board
    }

    fn rules_for(board: &Board) -> Rules {
        Rules { rows: board.rows, cols: board.cols, ..Rules::default() }
    }

    #[test]
    fn finds_a_horizontal_three() {
        let board = board_of(&["111", "234", "567"]);
        let groups = find_matches(&board, &rules_for(&board));
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].cells.len(), 3);
        assert_eq!(groups[0].h_run, 3);
        assert_eq!(groups[0].v_run, 0);
        assert_eq!(groups[0].award(&SpecialSet::ALL), Special::None);
    }

    #[test]
    fn ignores_runs_shorter_than_the_minimum() {
        let board = board_of(&["112", "234", "567"]);
        assert!(find_matches(&board, &rules_for(&board)).is_empty());
    }

    #[test]
    fn a_row_of_four_earns_a_gem_that_clears_downward() {
        // Four across is finished by sliding a gem in vertically, so the gem it
        // leaves clears vertically too.
        let board = board_of(&["1111", "2345", "6789", "2345"]);
        let groups = find_matches(&board, &rules_for(&board));
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].h_run, 4);
        assert_eq!(groups[0].award(&SpecialSet::ALL), Special::LineV);
    }

    #[test]
    fn a_column_of_four_earns_a_gem_that_clears_across() {
        let board = board_of(&["1234", "1345", "1456", "1567"]);
        let groups = find_matches(&board, &rules_for(&board));
        assert_eq!(groups[0].v_run, 4);
        assert_eq!(groups[0].award(&SpecialSet::ALL), Special::LineH);
    }

    #[test]
    fn each_line_clearer_is_its_own_unlock() {
        // The two are unlocked separately, and which run makes which is the
        // opposite of what the name suggests: a row of four leaves a clearer
        // that fires down a column. So a player holding only the horizontal
        // one is rewarded for stacking four in a column and gets nothing at
        // all for four along a row.
        let across = board_of(&["1111", "2345", "6789", "2345"]);
        let down = board_of(&["1234", "1345", "1456", "1567"]);
        let row = find_matches(&across, &rules_for(&across));
        let column = find_matches(&down, &rules_for(&down));

        let only_h = SpecialSet { line_h: true, ..SpecialSet::NONE };
        assert_eq!(column[0].award(&only_h), Special::LineH, "the one it does hold");
        assert_eq!(row[0].award(&only_h), Special::None, "and nothing for the other run");

        let only_v = SpecialSet { line_v: true, ..SpecialSet::NONE };
        assert_eq!(row[0].award(&only_v), Special::LineV);
        assert_eq!(column[0].award(&only_v), Special::None);
    }

    #[test]
    fn a_locked_line_still_leaves_a_square_its_rocket() {
        // The awards are a chain, and a run that earns nothing has to fall
        // through it rather than ending the search: this shape is four across
        // and a 2x2 at once.
        let board = board_of(&["1111", "1145", "6789", "2345"]);
        let groups = find_matches(&board, &rules_for(&board));
        assert!(groups[0].squares > 0, "the shape under test is not a square");
        assert_eq!(groups[0].award(&SpecialSet::ALL), Special::LineV, "the run wins while held");
        assert_eq!(
            groups[0].award(&SpecialSet { rocket: true, ..SpecialSet::NONE }),
            Special::Rocket,
            "with the line locked the square should still pay out",
        );
    }

    #[test]
    fn a_run_of_five_earns_a_rainbow() {
        let board = board_of(&["11111", "23452", "67893", "23454", "67895"]);
        let groups = find_matches(&board, &rules_for(&board));
        assert_eq!(groups[0].award(&SpecialSet::ALL), Special::Rainbow);
    }

    #[test]
    fn an_l_shape_is_one_group_and_earns_a_cross() {
        // A vertical three down the left meeting a horizontal three along the
        // top, with the corner cells kept apart so no 2x2 forms.
        let board = board_of(&["1112", "1231", "1452", "2345"]);
        let groups = find_matches(&board, &rules_for(&board));
        assert_eq!(groups.len(), 1, "the two runs are one shape");
        assert_eq!(groups[0].squares, 0);
        assert_eq!(groups[0].award(&SpecialSet::ALL), Special::Cross);
        assert_eq!(groups[0].pivot, Pos::new(0, 0), "the special lands on the corner");
    }

    #[test]
    fn a_two_by_two_is_a_match_on_its_own() {
        let board = board_of(&["1123", "1145", "6789", "2345"]);
        let groups = find_matches(&board, &rules_for(&board));
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].cells.len(), 4);
        assert_eq!(groups[0].squares, 1);
        assert_eq!(groups[0].award(&SpecialSet::ALL), Special::Rocket);
    }

    #[test]
    fn a_square_still_wins_against_a_run_that_earns_nothing() {
        // A 2x2 with a third gem extending the top row into a run of three.
        // Three in a row is worth no gem of its own, so the rocket stands.
        let board = board_of(&["1114", "1145", "6789", "2345"]);
        let groups = find_matches(&board, &rules_for(&board));
        assert_eq!(groups.len(), 1, "the run and the square are one group");
        assert_eq!(groups[0].cells.len(), 5, "the whole shape clears");
        assert_eq!(groups[0].squares, 1);
        assert_eq!(
            groups[0].award(&SpecialSet::ALL),
            Special::Rocket,
            "the square wins and the run earns nothing of its own"
        );
    }

    #[test]
    fn overlapping_squares_are_one_group_and_one_rocket() {
        // A 2x3 block holds two overlapping 2x2s.
        let board = board_of(&["1114", "1115", "6789", "2345"]);
        let groups = find_matches(&board, &rules_for(&board));
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].cells.len(), 6);
        assert!(groups[0].squares >= 2);
        assert_eq!(groups[0].award(&SpecialSet::ALL), Special::Rocket);
    }

    #[test]
    fn a_run_of_four_outranks_the_square_it_contains() {
        // Row 0 is four across and the left half of it is also a 2x2. The run
        // is worth more, so that is what the player gets.
        let board = board_of(&["1111", "1123", "4567", "8901"]);
        let groups = find_matches(&board, &rules_for(&board));
        assert_eq!(groups.len(), 1);
        assert!(groups[0].squares > 0, "the shape really does contain a square");
        assert_eq!(groups[0].h_run, 4);
        assert_eq!(groups[0].award(&SpecialSet::ALL), Special::LineV);
    }

    #[test]
    fn a_square_tangled_in_an_l_still_yields_the_cross() {
        let board = board_of(&["1114", "1145", "1567", "8901"]);
        let groups = find_matches(&board, &rules_for(&board));
        assert_eq!(groups.len(), 1);
        assert!(groups[0].squares > 0);
        assert_eq!(groups[0].award(&SpecialSet::ALL), Special::Cross);
    }

    #[test]
    fn two_specials_swapped_together_are_always_a_move() {
        let mut board = board_of(&["12", "34"]);
        let rules = rules_for(&board);
        board.set_gem(Pos::new(0, 0), Some(Gem { color: 1, special: Special::Cross }));
        assert!(
            !is_useful_swap(&board, &rules, Pos::new(0, 0), Pos::new(0, 1)),
            "a special against an ordinary gem still does nothing"
        );
        board.set_gem(Pos::new(0, 1), Some(Gem { color: 2, special: Special::LineH }));
        assert!(
            is_useful_swap(&board, &rules, Pos::new(0, 0), Pos::new(0, 1)),
            "but two specials set each other off"
        );
    }

    #[test]
    fn squares_can_be_switched_off() {
        let board = board_of(&["1123", "1145", "6789", "2345"]);
        let rules = Rules { square_match: false, ..rules_for(&board) };
        assert!(find_matches(&board, &rules).is_empty());
    }

    #[test]
    fn disabled_specials_award_nothing() {
        let board = board_of(&["1111", "2345", "6789", "2345"]);
        let groups = find_matches(&board, &rules_for(&board));
        assert_eq!(groups[0].award(&SpecialSet::NONE), Special::None);
    }

    #[test]
    fn a_square_still_clears_when_rockets_are_switched_off() {
        let board = board_of(&["1123", "1145", "6789", "2345"]);
        let groups = find_matches(&board, &rules_for(&board));
        assert_eq!(groups.len(), 1, "the square is a match whatever it earns");
        assert_eq!(groups[0].award(&SpecialSet::NONE), Special::None);
    }

    #[test]
    fn walls_break_a_run() {
        let board = board_of(&["11#11", "23452", "67893"]);
        assert!(find_matches(&board, &rules_for(&board)).is_empty());
    }

    #[test]
    fn useful_swap_sees_the_match_it_would_make() {
        let board = board_of(&["121", "112", "345"]);
        let rules = rules_for(&board);
        assert!(is_useful_swap(&board, &rules, Pos::new(0, 1), Pos::new(1, 1)));
        assert!(!is_useful_swap(&board, &rules, Pos::new(2, 0), Pos::new(2, 1)));
    }

    #[test]
    fn useful_swap_sees_a_square_it_would_make() {
        // Swapping (1,1) with (1,2) brings a 1 under the pair above it,
        // closing a 2x2 in the corner without forming any run of three.
        let board = board_of(&["1123", "1213", "4567", "5678"]);
        let rules = rules_for(&board);
        assert!(is_useful_swap(&board, &rules, Pos::new(1, 1), Pos::new(1, 2)));
        let no_squares = Rules { square_match: false, ..rules };
        assert!(
            !is_useful_swap(&board, &no_squares, Pos::new(1, 1), Pos::new(1, 2)),
            "with squares off that swap does nothing"
        );
    }

    #[test]
    fn specials_no_longer_make_any_swap_worth_it() {
        let mut board = board_of(&["12", "34"]);
        let rules = rules_for(&board);
        board.set_gem(Pos::new(0, 0), Some(Gem { color: 1, special: Special::Cross }));
        assert!(
            !is_useful_swap(&board, &rules, Pos::new(0, 0), Pos::new(0, 1)),
            "a cross waits to be matched, it is not a battering ram"
        );
    }

    #[test]
    fn a_rainbow_is_the_one_gem_worth_swapping_anywhere() {
        let mut board = board_of(&["12", "34"]);
        let rules = rules_for(&board);
        board.set_gem(Pos::new(0, 0), Some(Gem { color: 1, special: Special::Rainbow }));
        assert!(is_useful_swap(&board, &rules, Pos::new(0, 0), Pos::new(0, 1)));
    }

    #[test]
    fn find_move_reports_none_on_a_locked_board() {
        // A Latin square has no run available, and no 2x2 either.
        let board = board_of(&["1234", "2341", "3412", "4123"]);
        assert!(find_move(&board, &rules_for(&board)).is_none());
    }

    #[test]
    fn a_line_gem_takes_its_whole_row() {
        let mut board = board_of(&["1234", "5678", "1234", "5678"]);
        board.set_gem(Pos::new(1, 1), Some(Gem { color: 6, special: Special::LineH }));
        let mut rng = Rng::new(1);
        let result = detonate(&board, &[Pos::new(1, 1)], &[], &[], &mut rng, 0.0);
        assert_eq!(result.cleared.len(), 4);
        assert!(result.cleared.iter().all(|p| p.r == 1));
        assert_eq!(result.fired.len(), 1);
    }

    #[test]
    fn a_line_clear_sweeps_outward_rather_than_popping_at_once() {
        let mut board = board_of(&["1234", "5678", "1234", "5678"]);
        board.set_gem(Pos::new(1, 0), Some(Gem { color: 5, special: Special::LineH }));
        let mut rng = Rng::new(1);
        let result = detonate(&board, &[Pos::new(1, 0)], &[], &[], &mut rng, 0.0);

        for (cell, delay) in result.cleared.iter().zip(result.delays.iter()) {
            let expected = (cell.c - 1 + 1) as f32 * SPREAD_STEP_MS;
            let _ = expected;
            assert_eq!(
                *delay,
                (cell.c as f32) * SPREAD_STEP_MS,
                "cell {cell:?} should pop in step with its distance from the gem"
            );
        }
        assert_eq!(result.delays[0], 0.0, "the gem that fired goes first");
    }

    #[test]
    fn a_cross_takes_a_row_and_a_column() {
        let mut board = board_of(&["1234", "5678", "1234", "5678"]);
        board.set_gem(Pos::new(1, 1), Some(Gem { color: 6, special: Special::Cross }));
        let mut rng = Rng::new(1);
        let result = detonate(&board, &[Pos::new(1, 1)], &[], &[], &mut rng, 0.0);
        // Four across plus four down, sharing the middle.
        assert_eq!(result.cleared.len(), 7);
    }

    #[test]
    fn specials_set_each_other_off() {
        let mut board = board_of(&["1234", "5678", "1234", "5678"]);
        board.set_gem(Pos::new(1, 1), Some(Gem { color: 6, special: Special::LineH }));
        board.set_gem(Pos::new(1, 3), Some(Gem { color: 8, special: Special::LineV }));
        let mut rng = Rng::new(1);
        let result = detonate(&board, &[Pos::new(1, 1)], &[], &[], &mut rng, 0.0);
        assert_eq!(result.fired.len(), 2);
        assert!(result.cleared.contains(&Pos::new(0, 3)));
        assert!(result.cleared.contains(&Pos::new(3, 3)));
    }

    #[test]
    fn a_beam_passes_a_waiting_rocket_by_without_taking_it() {
        // A rocket is holding its cell until it launches. A beam crossing the
        // row goes straight through it, clearing what is on the far side, and
        // leaves the rocket standing: it neither goes off early nor is taken
        // away before it ever fires.
        let mut board = board_of(&["1234", "5678", "1234", "5678"]);
        board.set_gem(Pos::new(1, 0), Some(Gem { color: 5, special: Special::LineH }));
        board.set_gem(Pos::new(1, 2), Some(Gem { color: 7, special: Special::Rocket }));
        let mut rng = Rng::new(1);
        let result = detonate(&board, &[Pos::new(1, 0)], &[], &[], &mut rng, 0.0);

        assert!(!result.cleared.contains(&Pos::new(1, 2)), "the rocket should have survived");
        assert!(
            result.cleared.contains(&Pos::new(1, 3)),
            "and the beam should have carried on past it",
        );
        assert_eq!(result.cleared.len(), 3, "the rest of the row, and no further");
        assert_eq!(result.fired.len(), 1, "only the line gem fired");
    }

    #[test]
    fn a_beam_crosses_a_wall_rather_than_stopping_at_it() {
        // A wall divides where gems can fall, not where a beam can reach. So a
        // line gem fired in a walled-off pocket still takes its whole row, and
        // a level drawn in separate chambers is not separate to a special.
        //
        // Deliberate rather than incidental, and asserted because nothing else
        // would notice it changing: the wall holds no gem, so it contributes
        // nothing of its own, and the cells beyond it go with the rest.
        let mut board = board_of(&["1#34", "5678", "1234", "5678"]);
        board.set_gem(Pos::new(0, 0), Some(Gem { color: 1, special: Special::LineH }));
        let mut rng = Rng::new(1);
        let result = detonate(&board, &[Pos::new(0, 0)], &[], &[], &mut rng, 0.0);

        assert!(
            result.cleared.contains(&Pos::new(0, 3)),
            "the beam stopped at the wall instead of crossing it",
        );
        assert_eq!(result.cleared.len(), 3, "the row, less the wall itself");
    }

    #[test]
    fn a_rainbow_caught_in_a_blast_takes_the_commonest_color() {
        let mut board = board_of(&["1111", "1111", "1123", "4567"]);
        board.set_gem(Pos::new(3, 0), Some(Gem { color: 4, special: Special::Rainbow }));
        let mut rng = Rng::new(1);
        let result = detonate(&board, &[Pos::new(3, 0)], &[], &[], &mut rng, 0.0);
        assert_eq!(result.cleared.len(), 11);
    }
}
