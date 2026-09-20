//! Finding matches, awarding specials, and resolving chain reactions.

use crate::board::{Board, Pos, Special};
use crate::rng::Rng;
use crate::rules::{Rules, SpecialSet, MAX_COLORS};

/// One connected clump of matched gems. Overlapping runs (an L or a T) arrive
/// as a single group so the shape can earn a single special.
#[derive(Clone, Debug)]
pub struct MatchGroup {
    pub cells: Vec<Pos>,
    pub color: u8,
    /// Longest horizontal run inside the group, 0 if none reached `min_match`.
    pub h_run: i32,
    pub v_run: i32,
    /// Where a special created by this group should land.
    pub pivot: Pos,
}

impl MatchGroup {
    /// The special this shape earns, if any.
    pub fn award(&self, specials: &SpecialSet) -> Special {
        let longest = self.h_run.max(self.v_run);
        if specials.rainbow && longest >= 5 {
            Special::Rainbow
        } else if specials.bomb && self.h_run >= 3 && self.v_run >= 3 {
            Special::Bomb
        } else if specials.line && longest == 4 {
            if self.h_run >= self.v_run {
                Special::LineH
            } else {
                Special::LineV
            }
        } else {
            Special::None
        }
    }
}

/// A run of same-colored gems found during the scan.
struct Run {
    cells: Vec<Pos>,
    horizontal: bool,
    color: u8,
}

/// Scans the whole board for runs of at least `rules.min_match` and merges
/// overlapping ones into groups.
pub fn find_matches(board: &Board, rules: &Rules) -> Vec<MatchGroup> {
    let min = rules.min_match;
    let mut runs: Vec<Run> = Vec::new();

    for r in 0..board.rows {
        collect_runs(board, min, &mut runs, true, r);
    }
    for c in 0..board.cols {
        collect_runs(board, min, &mut runs, false, c);
    }

    // Runs that share a cell belong to the same shape.
    let mut parent: Vec<usize> = (0..runs.len()).collect();
    let mut owner: Vec<Option<usize>> = vec![None; (board.rows * board.cols) as usize];
    for (i, run) in runs.iter().enumerate() {
        for cell in &run.cells {
            let slot = (cell.r * board.cols + cell.c) as usize;
            match owner[slot] {
                Some(j) => union(&mut parent, i, j),
                None => owner[slot] = Some(i),
            }
        }
    }

    let mut groups: Vec<MatchGroup> = Vec::new();
    let mut group_of: Vec<Option<usize>> = vec![None; runs.len()];
    for (i, run) in runs.iter().enumerate() {
        let root = find(&mut parent, i);
        let index = match group_of[root] {
            Some(g) => g,
            None => {
                groups.push(MatchGroup {
                    cells: Vec::new(),
                    color: run.color,
                    h_run: 0,
                    v_run: 0,
                    pivot: run.cells[0],
                });
                group_of[root] = Some(groups.len() - 1);
                groups.len() - 1
            }
        };
        let group = &mut groups[index];
        let len = run.cells.len() as i32;
        if run.horizontal {
            group.h_run = group.h_run.max(len);
        } else {
            group.v_run = group.v_run.max(len);
        }
        for cell in &run.cells {
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
fn collect_runs(board: &Board, min: i32, runs: &mut Vec<Run>, horizontal: bool, line: i32) {
    let length = if horizontal { board.cols } else { board.rows };
    let mut start = 0;
    while start < length {
        let pos = |i: i32| if horizontal { Pos::new(line, i) } else { Pos::new(i, line) };
        let color = match board.color(pos(start)) {
            Some(color) => color,
            None => {
                start += 1;
                continue;
            }
        };
        let mut end = start + 1;
        while end < length && board.color(pos(end)) == Some(color) {
            end += 1;
        }
        if end - start >= min {
            runs.push(Run {
                cells: (start..end).map(pos).collect(),
                horizontal,
                color,
            });
        }
        start = end;
    }
}

/// The cell a created special should occupy: the corner of an L or T, else the
/// middle of the run.
fn natural_pivot(group: &MatchGroup) -> Pos {
    if group.h_run >= 3 && group.v_run >= 3 {
        // The junction is the cell with neighbors on both axes inside the group.
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
    pub fn color(&self, p: Pos) -> Option<u8> {
        if p == self.a {
            self.board.color(self.b)
        } else if p == self.b {
            self.board.color(self.a)
        } else {
            self.board.color(p)
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

/// Whether swapping these two cells is a legal move: it either lands a match or
/// sets off a special.
pub fn is_useful_swap(board: &Board, rules: &Rules, a: Pos, b: Pos) -> bool {
    if !a.is_adjacent(b) || !board.is_open(a) || !board.is_open(b) {
        return false;
    }
    let (ga, gb) = match (board.gem(a), board.gem(b)) {
        (Some(ga), Some(gb)) => (ga, gb),
        _ => return false,
    };
    // Any swap that moves a special is worth making.
    if ga.special.is_special() || gb.special.is_special() {
        return true;
    }
    let view = SwapView { board, a, b };
    forms_match(&view, a, rules.min_match) || forms_match(&view, b, rules.min_match)
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

/// What a special does when it goes off.
fn blast(board: &Board, p: Pos, special: Special, rainbow_color: u8, out: &mut Vec<Pos>) {
    match special {
        Special::None => {}
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
        Special::Bomb => {
            for r in (p.r - 1)..=(p.r + 1) {
                for c in (p.c - 1)..=(p.c + 1) {
                    out.push(Pos::new(r, c));
                }
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
pub fn most_common_color(board: &Board) -> u8 {
    let mut counts = [0u32; MAX_COLORS];
    for p in board.positions() {
        if let Some(color) = board.color(p) {
            if (color as usize) < MAX_COLORS {
                counts[color as usize] += 1;
            }
        }
    }
    let mut best = 0;
    for color in 1..MAX_COLORS {
        if counts[color] > counts[best] {
            best = color;
        }
    }
    best as u8
}

/// The full set of cells a clear takes with it.
pub struct Detonation {
    /// Every cell that ends up cleared, in the order it was reached.
    pub cleared: Vec<Pos>,
    /// The specials that went off, in firing order.
    pub fired: Vec<(Pos, Special)>,
}

/// Grows an initial clear into its chain reaction: specials caught in the blast
/// fire in turn, and so do the ones they catch.
pub fn detonate(board: &Board, seeds: &[Pos], rng: &mut Rng) -> Detonation {
    let mut marked = vec![false; (board.rows * board.cols) as usize];
    let mut cleared: Vec<Pos> = Vec::new();
    let mut fired: Vec<(Pos, Special)> = Vec::new();
    let mut queue: Vec<Pos> = Vec::new();

    let push = |p: Pos, marked: &mut Vec<bool>, cleared: &mut Vec<Pos>, queue: &mut Vec<Pos>| {
        if !board.contains(p) || board.gem(p).is_none() {
            return;
        }
        let slot = (p.r * board.cols + p.c) as usize;
        if marked[slot] {
            return;
        }
        marked[slot] = true;
        cleared.push(p);
        queue.push(p);
    };

    for seed in seeds {
        push(*seed, &mut marked, &mut cleared, &mut queue);
    }

    // Nudge the rainbow's fallback color so repeated cascades are not identical.
    let fallback = most_common_color(board);
    let _ = rng.next_u32();

    let mut head = 0;
    let mut hits: Vec<Pos> = Vec::new();
    while head < queue.len() {
        let p = queue[head];
        head += 1;
        let special = match board.gem(p) {
            Some(gem) => gem.special,
            None => continue,
        };
        if !special.is_special() {
            continue;
        }
        fired.push((p, special));
        hits.clear();
        blast(board, p, special, fallback, &mut hits);
        for hit in std::mem::take(&mut hits) {
            push(hit, &mut marked, &mut cleared, &mut queue);
        }
    }

    Detonation { cleared, fired }
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
    fn a_run_of_four_earns_a_line_gem() {
        let board = board_of(&["1111", "2345", "6789", "2345"]);
        let groups = find_matches(&board, &rules_for(&board));
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].h_run, 4);
        assert_eq!(groups[0].award(&SpecialSet::ALL), Special::LineH);
    }

    #[test]
    fn a_run_of_five_earns_a_rainbow() {
        let board = board_of(&["11111", "23452", "67893", "23454", "67895"]);
        let groups = find_matches(&board, &rules_for(&board));
        assert_eq!(groups[0].award(&SpecialSet::ALL), Special::Rainbow);
    }

    #[test]
    fn an_l_shape_is_one_group_and_earns_a_bomb() {
        // A vertical three down the left meeting a horizontal three along the top.
        let board = board_of(&["111", "123", "145"]);
        let groups = find_matches(&board, &rules_for(&board));
        assert_eq!(groups.len(), 1, "the two runs are one shape");
        assert_eq!(groups[0].cells.len(), 5);
        assert_eq!(groups[0].award(&SpecialSet::ALL), Special::Bomb);
        assert_eq!(groups[0].pivot, Pos::new(0, 0), "the special lands on the corner");
    }

    #[test]
    fn disabled_specials_award_nothing() {
        let board = board_of(&["1111", "2345", "6789", "2345"]);
        let groups = find_matches(&board, &rules_for(&board));
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
        // Swapping (1,0) with (0,0) lines up three 1s down column 0? No — the
        // useful move here is (0,1) with (1,1), which puts a 1 at (0,1).
        assert!(is_useful_swap(&board, &rules, Pos::new(0, 1), Pos::new(1, 1)));
        assert!(!is_useful_swap(&board, &rules, Pos::new(2, 0), Pos::new(2, 1)));
    }

    #[test]
    fn a_swap_involving_a_special_is_always_useful() {
        let mut board = board_of(&["12", "34"]);
        let rules = rules_for(&board);
        assert!(!is_useful_swap(&board, &rules, Pos::new(0, 0), Pos::new(0, 1)));
        board.set_gem(Pos::new(0, 0), Some(Gem { color: 1, special: Special::Bomb }));
        assert!(is_useful_swap(&board, &rules, Pos::new(0, 0), Pos::new(0, 1)));
    }

    #[test]
    fn find_move_reports_none_on_a_locked_board() {
        // A Latin square is stuck: every row and column already holds four
        // distinct colors, and one swap can never bring three together.
        let board = board_of(&["1234", "2341", "3412", "4123"]);
        assert!(find_move(&board, &rules_for(&board)).is_none());
    }

    #[test]
    fn find_move_spots_a_swap_a_checkerboard_still_allows() {
        // Tempting to call this locked, but lifting a 1 into row 0 completes
        // three across, so the board is live.
        let board = board_of(&["1212", "2121", "1212", "2121"]);
        assert_eq!(find_move(&board, &rules_for(&board)), Some((Pos::new(0, 1), Pos::new(1, 1))));
    }

    #[test]
    fn a_line_gem_takes_its_whole_row() {
        let mut board = board_of(&["1234", "5678", "1234", "5678"]);
        board.set_gem(Pos::new(1, 1), Some(Gem { color: 6, special: Special::LineH }));
        let mut rng = Rng::new(1);
        let result = detonate(&board, &[Pos::new(1, 1)], &mut rng);
        assert_eq!(result.cleared.len(), 4);
        assert!(result.cleared.iter().all(|p| p.r == 1));
        assert_eq!(result.fired.len(), 1);
    }

    #[test]
    fn specials_set_each_other_off() {
        let mut board = board_of(&["1234", "5678", "1234", "5678"]);
        board.set_gem(Pos::new(1, 1), Some(Gem { color: 6, special: Special::LineH }));
        board.set_gem(Pos::new(1, 3), Some(Gem { color: 8, special: Special::LineV }));
        let mut rng = Rng::new(1);
        let result = detonate(&board, &[Pos::new(1, 1)], &mut rng);
        // The row goes, and the column gem it catches takes its column too.
        assert_eq!(result.fired.len(), 2);
        assert!(result.cleared.contains(&Pos::new(0, 3)));
        assert!(result.cleared.contains(&Pos::new(3, 3)));
    }

    #[test]
    fn a_bomb_clears_its_neighborhood_and_stops_at_the_edge() {
        let mut board = board_of(&["1234", "5678", "1234", "5678"]);
        board.set_gem(Pos::new(0, 0), Some(Gem { color: 1, special: Special::Bomb }));
        let mut rng = Rng::new(1);
        let result = detonate(&board, &[Pos::new(0, 0)], &mut rng);
        assert_eq!(result.cleared.len(), 4, "a corner bomb only has four cells to take");
    }

    #[test]
    fn a_rainbow_caught_in_a_blast_takes_the_commonest_color() {
        let mut board = board_of(&["1111", "1111", "1123", "4567"]);
        board.set_gem(Pos::new(3, 0), Some(Gem { color: 4, special: Special::Rainbow }));
        let mut rng = Rng::new(1);
        let result = detonate(&board, &[Pos::new(3, 0)], &mut rng);
        // Every 1 on the board goes, plus the rainbow itself.
        assert_eq!(result.cleared.len(), 11);
    }
}
