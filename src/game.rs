//! The playable game: input, the clock, and the resolve loop.
//!
//! The engine owns time. A front end calls [`Game::update`] with the elapsed
//! milliseconds and then reads a snapshot; every decision about when a swap
//! lands, when a cascade fires and when a level ends is made here, so the same
//! rules hold whether the front end is a browser, a test, or a headless
//! simulation.

use crate::board::{Board, Gem, Pos, Special};
use crate::level::{LevelSpec, Objective, Progress};
use crate::matching::{self, MatchGroup};
use crate::rng::Rng;
use crate::rules::{Rules, MAX_COLORS};

/// How long each animated phase lasts, in milliseconds.
pub const SWAP_MS: f32 = 130.0;
pub const CLEAR_MS: f32 = 190.0;
pub const FALL_MS: f32 = 240.0;
pub const SHUFFLE_MS: f32 = 420.0;

const SCORE_PER_GEM: u64 = 50;
const SCORE_PER_SPECIAL_FIRED: u64 = 120;
const SCORE_PER_SPECIAL_MADE: u64 = 200;

/// Event tags shared with the front end for sound and particles.
pub const EV_CLEAR: u8 = 1;
pub const EV_SPECIAL_MADE: u8 = 2;
pub const EV_SPECIAL_FIRED: u8 = 3;
pub const EV_SWAP: u8 = 4;
pub const EV_REVERT: u8 = 5;
pub const EV_CASCADE: u8 = 6;
pub const EV_SHUFFLE: u8 = 7;
pub const EV_WON: u8 = 8;
pub const EV_LOST: u8 = 9;

/// Something worth seeing or hearing. Positions are 255 when the event is not
/// about one cell.
#[derive(Clone, Copy, Debug)]
pub struct Event {
    pub kind: u8,
    pub r: u8,
    pub c: u8,
    pub color: u8,
    pub special: u8,
    pub cascade: u8,
    pub value: u16,
}

impl Event {
    fn at(kind: u8, p: Pos, color: u8, special: Special, cascade: u32) -> Self {
        Event {
            kind,
            r: p.r as u8,
            c: p.c as u8,
            color,
            special: special.code(),
            cascade: cascade.min(255) as u8,
            value: 0,
        }
    }

    fn plain(kind: u8, value: u16) -> Self {
        Event { kind, r: 255, c: 255, color: 255, special: 0, cascade: 0, value }
    }
}

/// What the board is doing right now.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Phase {
    /// Waiting for the player.
    Idle,
    Swapping { elapsed: f32, reverting: bool },
    Clearing { elapsed: f32 },
    Falling { elapsed: f32 },
    Shuffling { elapsed: f32 },
    /// The level is over; nothing advances until it is reloaded.
    Finished,
}

impl Phase {
    pub fn code(self) -> u32 {
        match self {
            Phase::Idle => 0,
            Phase::Swapping { .. } => 1,
            Phase::Clearing { .. } => 2,
            Phase::Falling { .. } => 3,
            Phase::Shuffling { .. } => 4,
            Phase::Finished => 5,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Playing,
    Won,
    Lost,
}

/// What a tap did, so the front end can respond without re-deriving it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tap {
    /// The tap did nothing: mid-animation, or an empty cell.
    Ignored,
    Selected,
    Deselected,
    /// A swap was started between the selection and the tapped cell.
    Swapped,
}

/// A clear that is about to happen: what sets it off, and what it leaves behind.
struct Resolution {
    seeds: Vec<Pos>,
    creations: Vec<(Pos, Gem)>,
}

#[derive(Clone)]
pub struct Game {
    pub spec: LevelSpec,
    pub board: Board,
    pub progress: Progress,
    pub moves_left: u32,
    seed: u64,
    rng: Rng,
    phase: Phase,
    status: Status,
    /// Cells popping during the current clear.
    clearing: Vec<Pos>,
    /// Specials to drop in once the clear finishes.
    pending: Vec<(Pos, Gem)>,
    /// Per-cell row a gem started falling from; see [`Board::collapse`].
    origin: Vec<f32>,
    cascade: u32,
    swap: Option<(Pos, Pos)>,
    selected: Option<Pos>,
    events: Vec<Event>,
    cells_buf: Vec<u8>,
    offs_buf: Vec<f32>,
}

impl Game {
    pub fn new(spec: LevelSpec, seed: u64) -> Self {
        let mut game = Game {
            board: Board::new(spec.rules.rows, spec.rules.cols),
            progress: Progress::default(),
            moves_left: spec.moves,
            seed,
            rng: Rng::new(seed),
            phase: Phase::Idle,
            status: Status::Playing,
            clearing: Vec::new(),
            pending: Vec::new(),
            origin: Vec::new(),
            cascade: 0,
            swap: None,
            selected: None,
            events: Vec::new(),
            cells_buf: Vec::new(),
            offs_buf: Vec::new(),
            spec,
        };
        game.restart();
        game
    }

    /// Rebuilds the level from its seed. The same seed gives the same board.
    pub fn restart(&mut self) {
        self.board = match self.spec.layout {
            Some(layout) => Board::from_layout(layout),
            None => Board::new(self.spec.rules.rows, self.spec.rules.cols),
        };
        self.rng = Rng::new(self.seed);
        self.progress = Progress::default();
        self.progress.jelly_total = self.board.jelly_remaining();
        self.progress.jelly_left = self.progress.jelly_total;
        self.moves_left = self.spec.moves;
        self.phase = Phase::Idle;
        self.status = Status::Playing;
        self.clearing.clear();
        self.pending.clear();
        self.cascade = 0;
        self.swap = None;
        self.selected = None;
        self.events.clear();
        self.deal();
        self.origin = self.board.positions().map(|p| p.r as f32).collect();
        self.refresh_snapshot();
    }

    pub fn rules(&self) -> &Rules {
        &self.spec.rules
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn status(&self) -> Status {
        self.status
    }

    pub fn cascade(&self) -> u32 {
        self.cascade
    }

    pub fn selected(&self) -> Option<Pos> {
        self.selected
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    /// True when the player may act.
    pub fn accepts_input(&self) -> bool {
        self.phase == Phase::Idle && self.status == Status::Playing
    }

    /// A legal move, for the hint button and for the idle nudge.
    pub fn hint(&self) -> Option<(Pos, Pos)> {
        matching::find_move(&self.board, &self.spec.rules)
    }

    // ---- input -----------------------------------------------------------

    /// Tap-to-select input: tapping a neighbor of the selection swaps.
    pub fn tap(&mut self, p: Pos) -> Tap {
        if !self.accepts_input() || self.board.gem(p).is_none() {
            return Tap::Ignored;
        }
        match self.selected {
            Some(sel) if sel == p => {
                self.selected = None;
                Tap::Deselected
            }
            Some(sel) if sel.is_adjacent(p) => {
                if self.try_swap(sel, p) {
                    Tap::Swapped
                } else {
                    self.selected = Some(p);
                    Tap::Selected
                }
            }
            _ => {
                self.selected = Some(p);
                Tap::Selected
            }
        }
    }

    pub fn clear_selection(&mut self) {
        self.selected = None;
    }

    /// Starts a swap. Returns false if the swap is not one the player may make;
    /// a swap that turns out to match nothing is still accepted here and slides
    /// back on its own.
    pub fn try_swap(&mut self, a: Pos, b: Pos) -> bool {
        if !self.accepts_input() || !a.is_adjacent(b) {
            return false;
        }
        if self.board.gem(a).is_none() || self.board.gem(b).is_none() {
            return false;
        }
        self.selected = None;
        self.board.swap_gems(a, b);
        self.swap = Some((a, b));
        self.phase = Phase::Swapping { elapsed: 0.0, reverting: false };
        self.events.push(Event::at(EV_SWAP, a, 255, Special::None, 0));
        true
    }

    // ---- the clock -------------------------------------------------------

    /// Advances the board by `dt_ms`, running as many phase transitions as that
    /// covers. Events from this step are readable afterwards via [`Game::events`].
    pub fn update(&mut self, dt_ms: f32) {
        self.events.clear();
        let mut remaining = if dt_ms.is_finite() { dt_ms.clamp(0.0, 250.0) } else { 0.0 };

        // A long frame may span several phases; the cap stops a pathological
        // dt from grinding through an unbounded number of cascades at once.
        for _ in 0..24 {
            let (duration, elapsed) = match self.phase {
                Phase::Idle | Phase::Finished => break,
                Phase::Swapping { elapsed, .. } => (SWAP_MS, elapsed),
                Phase::Clearing { elapsed } => (CLEAR_MS, elapsed),
                Phase::Falling { elapsed } => (FALL_MS, elapsed),
                Phase::Shuffling { elapsed } => (SHUFFLE_MS, elapsed),
            };
            let advanced = elapsed + remaining;
            if advanced < duration {
                self.set_elapsed(advanced);
                break;
            }
            remaining = advanced - duration;
            self.finish_phase();
            if remaining <= 0.0 {
                break;
            }
        }

        self.refresh_snapshot();
    }

    fn set_elapsed(&mut self, elapsed: f32) {
        self.phase = match self.phase {
            Phase::Swapping { reverting, .. } => Phase::Swapping { elapsed, reverting },
            Phase::Clearing { .. } => Phase::Clearing { elapsed },
            Phase::Falling { .. } => Phase::Falling { elapsed },
            Phase::Shuffling { .. } => Phase::Shuffling { elapsed },
            other => other,
        };
    }

    fn finish_phase(&mut self) {
        match self.phase {
            Phase::Swapping { reverting, .. } => self.finish_swap(reverting),
            Phase::Clearing { .. } => self.finish_clear(),
            Phase::Falling { .. } => self.finish_fall(),
            Phase::Shuffling { .. } => self.finish_shuffle(),
            Phase::Idle | Phase::Finished => {}
        }
    }

    fn finish_swap(&mut self, reverting: bool) {
        let (a, b) = match self.swap {
            Some(pair) => pair,
            None => {
                self.phase = Phase::Idle;
                return;
            }
        };
        if reverting {
            self.swap = None;
            self.settle();
            return;
        }

        match self.plan_resolution(Some((a, b))) {
            Some(resolution) => {
                self.spend_move();
                self.cascade = 1;
                self.swap = None;
                self.begin_clear(resolution);
            }
            None if self.spec.rules.revert_invalid => {
                // Nothing came of it: put the gems back and slide them home.
                self.board.swap_gems(a, b);
                self.phase = Phase::Swapping { elapsed: 0.0, reverting: true };
                self.events.push(Event::at(EV_REVERT, a, 255, Special::None, 0));
            }
            None => {
                self.spend_move();
                self.swap = None;
                self.settle();
            }
        }
    }

    fn finish_clear(&mut self) {
        for p in std::mem::take(&mut self.clearing) {
            self.board.set_gem(p, None);
        }
        for (p, gem) in std::mem::take(&mut self.pending) {
            self.board.set_gem(p, Some(gem));
        }
        self.origin = self.board.collapse(&self.spec.rules, &mut self.rng);
        self.phase = Phase::Falling { elapsed: 0.0 };
    }

    fn finish_fall(&mut self) {
        self.cascade += 1;
        match self.plan_resolution(None) {
            Some(resolution) => {
                self.events.push(Event::plain(EV_CASCADE, self.cascade.min(65535) as u16));
                self.begin_clear(resolution);
            }
            None => self.settle(),
        }
    }

    fn finish_shuffle(&mut self) {
        self.shuffle_board();
        self.origin = self.board.positions().map(|p| p.r as f32).collect();
        self.settle();
    }

    // ---- resolving -------------------------------------------------------

    /// Works out what this board state clears, and what specials it leaves
    /// behind. `swap` is the pair the player just moved, if any, which decides
    /// where a new special lands and whether a special combo fires.
    fn plan_resolution(&mut self, swap: Option<(Pos, Pos)>) -> Option<Resolution> {
        let groups = matching::find_matches(&self.board, &self.spec.rules);
        let mut seeds: Vec<Pos> = Vec::new();
        let mut creations: Vec<(Pos, Gem)> = Vec::new();

        for group in &groups {
            let special = group.award(&self.spec.rules.specials);
            if special != Special::None {
                let pivot = self.pivot_for(group, swap);
                if !creations.iter().any(|(p, _)| *p == pivot) {
                    creations.push((pivot, Gem { color: group.color, special }));
                }
            }
            seeds.extend_from_slice(&group.cells);
        }

        if let Some((a, b)) = swap {
            seeds.extend(self.combo_seeds(a, b));
        }

        if seeds.is_empty() {
            None
        } else {
            Some(Resolution { seeds, creations })
        }
    }

    /// New specials land under the player's finger when the match they made
    /// includes it, which is what makes them feel aimed.
    fn pivot_for(&self, group: &MatchGroup, swap: Option<(Pos, Pos)>) -> Pos {
        if let Some((a, b)) = swap {
            for candidate in [a, b] {
                if group.cells.contains(&candidate) {
                    return candidate;
                }
            }
        }
        group.pivot
    }

    /// Extra cells cleared because the player swapped two specials together,
    /// or swapped a special with anything at all.
    fn combo_seeds(&mut self, a: Pos, b: Pos) -> Vec<Pos> {
        let (ga, gb) = match (self.board.gem(a), self.board.gem(b)) {
            (Some(ga), Some(gb)) => (ga, gb),
            _ => return Vec::new(),
        };
        let mut seeds: Vec<Pos> = Vec::new();

        match (ga.special, gb.special) {
            (Special::None, Special::None) => {}

            // Two rainbows wipe the board.
            (Special::Rainbow, Special::Rainbow) => seeds.extend(self.board.occupied()),

            // A rainbow adopts its partner: every gem of that color goes, and
            // if the partner was itself special, they all go off as one.
            (Special::Rainbow, _) | (_, Special::Rainbow) => {
                let (rainbow, partner_pos) =
                    if ga.special == Special::Rainbow { (a, b) } else { (b, a) };
                let partner = self.board.gem(partner_pos).expect("checked above");
                let targets: Vec<Pos> = self
                    .board
                    .positions()
                    .filter(|p| self.board.color(*p) == Some(partner.color))
                    .collect();
                if partner.special.is_special() {
                    for p in &targets {
                        if let Some(gem) = self.board.gem(*p) {
                            self.board.set_gem(*p, Some(Gem { special: partner.special, ..gem }));
                        }
                    }
                }
                seeds.push(rainbow);
                seeds.extend(targets);
            }

            // Two line gems cross, whichever way each was facing.
            (Special::LineH | Special::LineV, Special::LineH | Special::LineV) => {
                self.board.set_gem(a, Some(Gem { special: Special::LineH, ..ga }));
                self.board.set_gem(b, Some(Gem { special: Special::LineV, ..gb }));
                seeds.push(a);
                seeds.push(b);
            }

            // Bomb meets bomb: a wider crater than either alone.
            (Special::Bomb, Special::Bomb) => {
                seeds.extend(self.block_around(b, 2));
                seeds.push(a);
            }

            // Bomb meets line: a three-wide cross through the bomb.
            (Special::Bomb, Special::LineH | Special::LineV)
            | (Special::LineH | Special::LineV, Special::Bomb) => {
                let center = if ga.special == Special::Bomb { a } else { b };
                for offset in -1..=1 {
                    for c in 0..self.board.cols {
                        seeds.push(Pos::new(center.r + offset, c));
                    }
                    for r in 0..self.board.rows {
                        seeds.push(Pos::new(r, center.c + offset));
                    }
                }
                seeds.push(a);
                seeds.push(b);
            }

            // One special, one ordinary gem: the special simply goes off.
            (special, Special::None) if special.is_special() => seeds.push(a),
            (Special::None, special) if special.is_special() => seeds.push(b),

            _ => {}
        }

        seeds.retain(|p| self.board.gem(*p).is_some());
        seeds
    }

    fn block_around(&self, center: Pos, radius: i32) -> Vec<Pos> {
        let mut cells = Vec::new();
        for r in (center.r - radius)..=(center.r + radius) {
            for c in (center.c - radius)..=(center.c + radius) {
                let p = Pos::new(r, c);
                if self.board.contains(p) {
                    cells.push(p);
                }
            }
        }
        cells
    }

    /// Fires the clear: scores it, tallies it against the objectives, and puts
    /// the board into its popping animation.
    fn begin_clear(&mut self, resolution: Resolution) {
        let blast = matching::detonate(&self.board, &resolution.seeds, &mut self.rng);
        if blast.cleared.is_empty() {
            self.settle();
            return;
        }

        let cascade = self.cascade.max(1);
        let points = (blast.cleared.len() as u64 * SCORE_PER_GEM
            + blast.fired.len() as u64 * SCORE_PER_SPECIAL_FIRED
            + resolution.creations.len() as u64 * SCORE_PER_SPECIAL_MADE)
            * cascade as u64;
        self.progress.score += points;

        for p in &blast.cleared {
            let gem = match self.board.gem(*p) {
                Some(gem) => gem,
                None => continue,
            };
            if (gem.color as usize) < MAX_COLORS {
                self.progress.cleared[gem.color as usize] += 1;
            }
            self.board.peel_jelly(*p);
            self.events.push(Event::at(EV_CLEAR, *p, gem.color, gem.special, cascade));
        }
        self.progress.jelly_left = self.board.jelly_remaining();

        for (p, special) in &blast.fired {
            self.events.push(Event::at(EV_SPECIAL_FIRED, *p, 255, *special, cascade));
        }
        for (p, gem) in &resolution.creations {
            self.events.push(Event::at(EV_SPECIAL_MADE, *p, gem.color, gem.special, cascade));
        }

        self.clearing = blast.cleared;
        self.pending = resolution.creations;
        self.phase = Phase::Clearing { elapsed: 0.0 };
    }

    /// The board has come to rest: decide whether the level is over, the board
    /// is stuck, or the player is up.
    fn settle(&mut self) {
        self.cascade = 0;
        self.swap = None;
        self.progress.jelly_left = self.board.jelly_remaining();

        if self.objectives_met() {
            self.status = Status::Won;
            self.phase = Phase::Finished;
            self.events.push(Event::plain(EV_WON, 0));
            return;
        }
        if self.spec.moves > 0 && self.moves_left == 0 {
            self.status = Status::Lost;
            self.phase = Phase::Finished;
            self.events.push(Event::plain(EV_LOST, 0));
            return;
        }
        if matching::find_move(&self.board, &self.spec.rules).is_none() {
            if self.spec.rules.shuffle_when_stuck {
                self.phase = Phase::Shuffling { elapsed: 0.0 };
                self.events.push(Event::plain(EV_SHUFFLE, 0));
            } else {
                self.status = Status::Lost;
                self.phase = Phase::Finished;
                self.events.push(Event::plain(EV_LOST, 0));
            }
            return;
        }
        self.phase = Phase::Idle;
    }

    pub fn objectives_met(&self) -> bool {
        self.spec.objectives.iter().all(|o| o.is_met(&self.progress))
    }

    pub fn objectives(&self) -> &[Objective] {
        &self.spec.objectives
    }

    fn spend_move(&mut self) {
        if self.spec.moves > 0 {
            self.moves_left = self.moves_left.saturating_sub(1);
        }
    }

    // ---- board generation ------------------------------------------------

    /// Fills every open cell, avoiding matches that would resolve before the
    /// player has touched anything, and guaranteeing at least one legal move.
    fn deal(&mut self) {
        let colors = self.spec.rules.colors.max(1) as u32;
        for _ in 0..64 {
            let cells: Vec<Pos> = self.board.positions().filter(|p| self.board.is_open(*p)).collect();
            for p in cells {
                let mut color = 0;
                // A handful of tries is enough to dodge a starting match; if
                // the colors run out we accept it and let the shuffle catch it.
                for _ in 0..12 {
                    color = self.rng.below(colors) as u8;
                    if !self.would_start_a_run(p, color) {
                        break;
                    }
                }
                self.board.set_gem(p, Some(Gem::plain(color)));
            }
            if matching::find_matches(&self.board, &self.spec.rules).is_empty()
                && matching::find_move(&self.board, &self.spec.rules).is_some()
            {
                return;
            }
        }
    }

    /// Whether placing `color` here completes a run with the cells already
    /// dealt above and to the left.
    fn would_start_a_run(&self, p: Pos, color: u8) -> bool {
        let min = self.spec.rules.min_match;
        let run_back = |dr: i32, dc: i32| {
            let mut n = 1;
            let mut cur = Pos::new(p.r + dr, p.c + dc);
            while self.board.color(cur) == Some(color) {
                n += 1;
                cur = Pos::new(cur.r + dr, cur.c + dc);
            }
            n
        };
        run_back(0, -1) >= min || run_back(-1, 0) >= min
    }

    /// Rearranges the gems already on the board into a position that has a move.
    fn shuffle_board(&mut self) {
        let cells = self.board.occupied();
        let mut gems: Vec<Gem> = cells.iter().filter_map(|p| self.board.gem(*p)).collect();
        for _ in 0..80 {
            self.rng.shuffle(&mut gems);
            for (p, gem) in cells.iter().zip(gems.iter()) {
                self.board.set_gem(*p, Some(*gem));
            }
            if matching::find_matches(&self.board, &self.spec.rules).is_empty()
                && matching::find_move(&self.board, &self.spec.rules).is_some()
            {
                return;
            }
        }
        // Shuffling this set of gems cannot produce a playable board, so deal
        // a fresh one rather than leaving the player stuck.
        self.deal();
    }

    // ---- snapshot --------------------------------------------------------

    /// Four bytes per cell: color (255 when empty), special, jelly, flags.
    pub fn cells_bytes(&self) -> &[u8] {
        &self.cells_buf
    }

    /// Three floats per cell: x offset, y offset (both in cell widths), scale.
    pub fn offsets(&self) -> &[f32] {
        &self.offs_buf
    }

    pub const FLAG_WALL: u8 = 1;
    pub const FLAG_CLEARING: u8 = 2;
    pub const FLAG_SELECTED: u8 = 4;

    fn refresh_snapshot(&mut self) {
        let count = (self.board.rows * self.board.cols) as usize;
        self.cells_buf.clear();
        self.cells_buf.resize(count * 4, 0);
        self.offs_buf.clear();
        self.offs_buf.resize(count * 3, 0.0);

        for p in self.board.positions() {
            let i = (p.r * self.board.cols + p.c) as usize;
            let cell = self.board.cell(p).expect("position came from the board");
            let (color, special) = match cell.gem {
                Some(gem) => (gem.color, gem.special.code()),
                None => (255, 0),
            };
            let mut flags = 0;
            if !self.board.is_open(p) {
                flags |= Self::FLAG_WALL;
            }
            if self.selected == Some(p) {
                flags |= Self::FLAG_SELECTED;
            }
            self.cells_buf[i * 4] = color;
            self.cells_buf[i * 4 + 1] = special;
            self.cells_buf[i * 4 + 2] = cell.jelly;
            self.cells_buf[i * 4 + 3] = flags;
            self.offs_buf[i * 3] = 0.0;
            self.offs_buf[i * 3 + 1] = 0.0;
            self.offs_buf[i * 3 + 2] = 1.0;
        }

        match self.phase {
            Phase::Swapping { elapsed, .. } => {
                if let Some((a, b)) = self.swap {
                    // The board already holds the swapped state, so each gem
                    // slides in from where its partner used to be.
                    let travel = 1.0 - ease_out(elapsed / SWAP_MS);
                    self.set_offset(a, (b.c - a.c) as f32 * travel, (b.r - a.r) as f32 * travel);
                    self.set_offset(b, (a.c - b.c) as f32 * travel, (a.r - b.r) as f32 * travel);
                }
            }
            Phase::Clearing { elapsed } => {
                let t = (elapsed / CLEAR_MS).clamp(0.0, 1.0);
                // A brief swell, then the gem shrinks out.
                let scale = if t < 0.25 { 1.0 + t * 0.6 } else { (1.15 - (t - 0.25) * 1.5).max(0.0) };
                for p in self.clearing.clone() {
                    let i = (p.r * self.board.cols + p.c) as usize;
                    if i * 3 + 2 < self.offs_buf.len() {
                        self.offs_buf[i * 3 + 2] = scale;
                        self.cells_buf[i * 4 + 3] |= Self::FLAG_CLEARING;
                    }
                }
            }
            Phase::Falling { elapsed } => {
                let travel = 1.0 - ease_out(elapsed / FALL_MS);
                for p in self.board.positions() {
                    let i = (p.r * self.board.cols + p.c) as usize;
                    let from = self.origin.get(i).copied().unwrap_or(p.r as f32);
                    let dy = (from - p.r as f32) * travel;
                    self.offs_buf[i * 3 + 1] = dy;
                }
            }
            Phase::Shuffling { elapsed } => {
                // Gems tuck in and pop back out while they are rearranged.
                let t = (elapsed / SHUFFLE_MS).clamp(0.0, 1.0);
                let scale = 1.0 - 0.8 * (t * std::f32::consts::PI).sin();
                for i in 0..(self.board.rows * self.board.cols) as usize {
                    self.offs_buf[i * 3 + 2] = scale;
                }
            }
            Phase::Idle | Phase::Finished => {}
        }
    }

    fn set_offset(&mut self, p: Pos, dx: f32, dy: f32) {
        let i = (p.r * self.board.cols + p.c) as usize;
        if i * 3 + 2 < self.offs_buf.len() {
            self.offs_buf[i * 3] = dx;
            self.offs_buf[i * 3 + 1] = dy;
        }
    }
}

fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::levels;
    use crate::rules::SpecialSet;

    fn spec(rows: i32, cols: i32, colors: u8, moves: u32) -> LevelSpec {
        LevelSpec {
            name: "test",
            rules: Rules { rows, cols, colors, ..Rules::default() },
            moves,
            objectives: vec![Objective::Score(1_000_000)],
            layout: None,
        }
    }

    /// Runs the clock until the board is idle again or the level ends,
    /// gathering the events raised along the way. `update` clears its event
    /// list every frame, so anything a test wants to see has to be collected
    /// as it goes.
    fn settle(game: &mut Game) -> Vec<Event> {
        let mut seen = Vec::new();
        for _ in 0..4000 {
            if game.phase() == Phase::Idle || game.phase() == Phase::Finished {
                return seen;
            }
            game.update(16.0);
            seen.extend_from_slice(game.events());
        }
        panic!("board never settled");
    }

    fn paint(game: &mut Game, rows: &[&str]) {
        for (r, row) in rows.iter().enumerate() {
            for (c, ch) in row.chars().enumerate() {
                if let Some(color) = ch.to_digit(10) {
                    game.board
                        .set_gem(Pos::new(r as i32, c as i32), Some(Gem::plain(color as u8)));
                }
            }
        }
    }

    #[test]
    fn a_fresh_board_is_full_playable_and_quiet() {
        for seed in 0..40 {
            let game = Game::new(spec(8, 8, 6, 20), seed);
            assert!(
                game.board.positions().all(|p| game.board.gem(p).is_some()),
                "seed {seed} left a hole"
            );
            assert!(
                matching::find_matches(&game.board, game.rules()).is_empty(),
                "seed {seed} dealt a board that was already matching"
            );
            assert!(game.hint().is_some(), "seed {seed} dealt a dead board");
        }
    }

    #[test]
    fn the_same_seed_deals_the_same_board() {
        let a = Game::new(spec(8, 8, 6, 20), 4242);
        let b = Game::new(spec(8, 8, 6, 20), 4242);
        assert!(a.board.positions().all(|p| a.board.gem(p) == b.board.gem(p)));
    }

    #[test]
    fn a_matching_swap_scores_and_costs_a_move() {
        let mut game = Game::new(spec(4, 4, 6, 10), 1);
        // Column 0 reads 1,1,2,0; swapping (2,0) with (2,1) makes it 1,1,1,0.
        paint(&mut game, &["1200", "1300", "2100", "0000"]);
        assert!(game.try_swap(Pos::new(2, 0), Pos::new(2, 1)));
        let _ = settle(&mut game);
        assert!(game.progress.score > 0, "a match should score");
        assert_eq!(game.moves_left, 9);
    }

    #[test]
    fn a_pointless_swap_slides_back_and_is_free() {
        let mut game = Game::new(spec(4, 4, 6, 10), 2);
        // Three quarters of a Latin square, so nothing matches, with a live
        // move parked in the bottom row so the settle does not shuffle.
        paint(&mut game, &["0123", "1230", "2301", "1101"]);
        let before = (game.board.gem(Pos::new(0, 0)), game.board.gem(Pos::new(0, 1)));
        assert!(game.try_swap(Pos::new(0, 0), Pos::new(0, 1)));
        let _ = settle(&mut game);
        assert_eq!(
            (game.board.gem(Pos::new(0, 0)), game.board.gem(Pos::new(0, 1))),
            before,
            "the gems should be back where they started"
        );
        assert_eq!(game.moves_left, 10, "a reverted swap costs nothing");
        assert_eq!(game.progress.score, 0);
    }

    #[test]
    fn input_is_refused_while_the_board_is_busy() {
        let mut game = Game::new(spec(4, 4, 6, 10), 3);
        paint(&mut game, &["1200", "1300", "2100", "0000"]);
        assert!(game.try_swap(Pos::new(2, 0), Pos::new(2, 1)));
        assert!(!game.accepts_input());
        assert!(!game.try_swap(Pos::new(0, 0), Pos::new(0, 1)));
        assert_eq!(game.tap(Pos::new(0, 0)), Tap::Ignored);
    }

    #[test]
    fn tapping_selects_then_swaps() {
        let mut game = Game::new(spec(4, 4, 6, 10), 4);
        paint(&mut game, &["1200", "1300", "2100", "0000"]);
        assert_eq!(game.tap(Pos::new(2, 0)), Tap::Selected);
        assert_eq!(game.selected(), Some(Pos::new(2, 0)));
        assert_eq!(game.tap(Pos::new(2, 0)), Tap::Deselected);
        assert_eq!(game.tap(Pos::new(2, 0)), Tap::Selected);
        assert_eq!(game.tap(Pos::new(2, 1)), Tap::Swapped);
    }

    #[test]
    fn tapping_a_distant_cell_moves_the_selection() {
        let mut game = Game::new(spec(4, 4, 6, 10), 5);
        assert_eq!(game.tap(Pos::new(0, 0)), Tap::Selected);
        assert_eq!(game.tap(Pos::new(3, 3)), Tap::Selected);
        assert_eq!(game.selected(), Some(Pos::new(3, 3)));
    }

    #[test]
    fn a_run_of_four_leaves_a_line_gem_under_the_swap() {
        let mut game = Game::new(spec(5, 5, 6, 10), 6);
        // Row 0 reads 1,1,2,1,3 — no match yet. Lifting the 1 at (1,2) into the
        // gap completes four across, and the special should land on the cell
        // the player moved.
        paint(&mut game, &["11213", "30120", "23401", "12340", "34012"]);
        assert!(matching::find_matches(&game.board, game.rules()).is_empty());
        assert!(game.try_swap(Pos::new(0, 2), Pos::new(1, 2)));
        let events = settle(&mut game);
        let made: Vec<&Event> = events.iter().filter(|e| e.kind == EV_SPECIAL_MADE).collect();
        assert_eq!(made.len(), 1, "a four-run makes exactly one special");
        assert_eq!(made[0].special, Special::LineH.code());
        assert_eq!((made[0].r, made[0].c), (0, 2), "it lands under the swap");
    }

    #[test]
    fn a_cascade_is_worth_more_than_the_same_clear_alone() {
        // The multiplier is what makes a chain feel different from two clears,
        // so check it against the plain rate rather than against itself.
        let mut game = Game::new(spec(8, 8, 6, 200), 11);
        let mut best_cascade = 0u32;
        let mut cascade_clear: Option<(u32, usize, u64)> = None;
        let mut score_before;

        for _ in 0..4000 {
            if game.status() != Status::Playing {
                break;
            }
            if game.phase() == Phase::Idle {
                match game.hint() {
                    Some((a, b)) => {
                        game.try_swap(a, b);
                    }
                    None => break,
                }
                continue;
            }
            score_before = game.progress.score;
            game.update(16.0);
            let cleared = game.events().iter().filter(|e| e.kind == EV_CLEAR).count();
            if let Some(event) = game.events().iter().find(|e| e.kind == EV_CLEAR) {
                if u32::from(event.cascade) > best_cascade {
                    best_cascade = u32::from(event.cascade);
                    cascade_clear = Some((
                        best_cascade,
                        cleared,
                        game.progress.score - score_before,
                    ));
                }
            }
        }

        let (cascade, cleared, gained) =
            cascade_clear.expect("a long bot session should clear something");
        assert!(cascade >= 2, "a long bot session should chain at least once");
        let flat_rate = cleared as u64 * SCORE_PER_GEM;
        assert!(
            gained >= flat_rate * cascade as u64,
            "a {cascade}x chain clearing {cleared} gems scored {gained}, \
             no better than the flat rate of {flat_rate}"
        );
    }

    #[test]
    fn running_out_of_moves_loses_the_level() {
        let mut level = spec(6, 6, 6, 1);
        level.objectives = vec![Objective::Score(u32::MAX)];
        let mut game = Game::new(level, 21);
        let (a, b) = game.hint().expect("a fresh board has a move");
        game.try_swap(a, b);
        let _ = settle(&mut game);
        assert_eq!(game.status(), Status::Lost);
        assert_eq!(game.phase(), Phase::Finished);
        assert!(!game.accepts_input());
    }

    #[test]
    fn meeting_the_objective_wins_immediately() {
        let mut level = spec(6, 6, 6, 30);
        level.objectives = vec![Objective::Score(1)];
        let mut game = Game::new(level, 22);
        let (a, b) = game.hint().expect("a fresh board has a move");
        game.try_swap(a, b);
        let _ = settle(&mut game);
        assert_eq!(game.status(), Status::Won);
        assert!(game.events().iter().any(|e| e.kind == EV_WON));
    }

    #[test]
    fn clearing_a_jelly_cell_peels_it() {
        let mut level = spec(4, 4, 6, 10);
        level.layout = Some(&["oooo", "oooo", "oooo", "oooo"]);
        level.objectives = vec![Objective::Jelly];
        let mut game = Game::new(level, 23);
        assert_eq!(game.progress.jelly_total, 16);
        assert_eq!(game.progress.jelly_left, 16);
        paint(&mut game, &["1200", "1300", "2100", "0000"]);
        game.try_swap(Pos::new(2, 0), Pos::new(2, 1));
        let _ = settle(&mut game);
        assert!(game.progress.jelly_left < 16, "the cleared cells should have lost a layer");
    }

    #[test]
    fn a_stuck_board_shuffles_rather_than_ending_the_level() {
        let mut level = spec(4, 4, 4, 10);
        level.rules.specials = SpecialSet::NONE;
        let mut game = Game::new(level, 24);
        paint(&mut game, &["1234", "2341", "3412", "4123"]);
        assert!(game.hint().is_none(), "the painted board is a locked Latin square");
        // Any swap settles the board, which notices there is nothing to do.
        game.try_swap(Pos::new(0, 0), Pos::new(0, 1));
        let _ = settle(&mut game);
        assert_eq!(game.status(), Status::Playing);
        assert!(game.hint().is_some(), "the shuffle should leave a playable board");
    }

    #[test]
    fn restart_rewinds_everything() {
        let mut game = Game::new(spec(6, 6, 6, 5), 31);
        let (a, b) = game.hint().unwrap();
        game.try_swap(a, b);
        let _ = settle(&mut game);
        let after_play = game.progress.score;
        game.restart();
        assert_eq!(game.progress.score, 0);
        assert_eq!(game.moves_left, 5);
        assert_eq!(game.status(), Status::Playing);
        assert_eq!(game.phase(), Phase::Idle);
        assert!(after_play > 0 || game.hint().is_some());
    }

    #[test]
    fn the_snapshot_describes_every_cell() {
        let mut level = spec(5, 5, 6, 10);
        level.layout = Some(&["..#..", ".....", "..o..", ".....", "....."]);
        let game = Game::new(level, 41);
        let cells = game.cells_bytes();
        let offs = game.offsets();
        assert_eq!(cells.len(), 25 * 4);
        assert_eq!(offs.len(), 25 * 3);
        // The wall is flagged and holds no gem.
        assert_eq!(cells[2 * 4] , 255);
        assert_eq!(cells[2 * 4 + 3] & Game::FLAG_WALL, Game::FLAG_WALL);
        // The jelly cell reports its layer.
        assert_eq!(cells[(2 * 5 + 2) * 4 + 2], 1);
        // At rest everything sits at its own cell, full size.
        assert!(offs.chunks(3).all(|o| o[0] == 0.0 && o[1] == 0.0 && o[2] == 1.0));
    }

    #[test]
    fn play_never_corrupts_the_board() {
        // Play every built-in level with a bot that always takes the hint, and
        // assert the invariants that make the board playable at all.
        for (index, level) in levels().into_iter().enumerate() {
            let jelly_level = level.objectives.contains(&Objective::Jelly);
            let mut game = Game::new(level, 900 + index as u64);
            for _ in 0..6000 {
                if game.status() != Status::Playing {
                    break;
                }
                if game.phase() == Phase::Idle {
                    match game.hint() {
                        Some((a, b)) => {
                            assert!(game.try_swap(a, b));
                        }
                        None => panic!("level {index} idled with no legal move"),
                    }
                } else {
                    game.update(16.0);
                }

                if game.phase() == Phase::Idle {
                    for p in game.board.positions() {
                        if game.board.is_open(p) {
                            assert!(
                                game.board.gem(p).is_some(),
                                "level {index} idled with a hole at {p:?}"
                            );
                        } else {
                            assert!(
                                game.board.gem(p).is_none(),
                                "level {index} put a gem inside a wall at {p:?}"
                            );
                        }
                    }
                    assert!(
                        matching::find_matches(&game.board, game.rules()).is_empty(),
                        "level {index} idled with an unresolved match"
                    );
                }
            }
            if jelly_level {
                assert!(
                    game.progress.jelly_left <= game.progress.jelly_total,
                    "level {index} peeled more jelly than it had"
                );
            }
        }
    }

    #[test]
    fn a_long_frame_does_not_skip_the_resolve_loop() {
        let mut game = Game::new(spec(6, 6, 6, 10), 55);
        let (a, b) = game.hint().unwrap();
        game.try_swap(a, b);
        // One absurdly long frame, as if the tab had been backgrounded.
        game.update(5_000.0);
        for _ in 0..200 {
            game.update(16.0);
        }
        assert!(matching::find_matches(&game.board, game.rules()).is_empty());
        assert!(game.board.positions().all(|p| !game.board.is_open(p) || game.board.gem(p).is_some()));
    }
}
