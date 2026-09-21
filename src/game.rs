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

/// How long each animated phase lasts, in milliseconds. These are paced to be
/// followed by eye rather than to get out of the way.
pub const SWAP_MS: f32 = 190.0;
/// How long one gem takes to swell and vanish. A clear lasts this plus however
/// long its blast takes to spread; see [`matching::SPREAD_STEP_MS`].
pub const POP_MS: f32 = 300.0;
pub const SHUFFLE_MS: f32 = 700.0;

/// Gems accelerate as they fall and then stop gaining speed, so a gem dropping
/// the height of the board takes longer than one dropping a single row rather
/// than both arriving together.
pub const FALL_ACCEL_MS: f32 = 160.0;
/// Terminal velocity, in cells per millisecond: about fourteen cells a second.
pub const FALL_SPEED: f32 = 0.014;

/// A rocket eases up to a top speed and then holds it.
///
/// Speed is capped rather than the flight being given a fixed duration: a
/// rocket crossing the whole board takes longer than one going next door,
/// instead of covering the extra ground faster. Without the cap a long shot
/// moves so quickly it is hard to see what it did.
pub const LAUNCH_RAMP_MS: f32 = 400.0;
/// Top speed, in cells per millisecond: a little over three cells a second.
pub const LAUNCH_SPEED: f32 = 0.00325;
/// A beat of stillness between gems landing and the clear that lands sets off.
///
/// Shorter than the rocket's hold below, because this one happens on every link
/// of a chain rather than once: long enough to separate the thump from the pop
/// that answers it, short enough that a six-deep cascade does not drag.
///
/// Only spent when a clear is actually waiting. A fall that ends the chain
/// hands the board straight back, since there is nothing to separate and a
/// pause there is just input the player cannot give yet.
pub const FALL_HOLD_MS: f32 = 80.0;

/// A beat of stillness between the last rocket striking and the board
/// collapsing into the holes.
///
/// Without it the strike and the gravity that answers it happen in the same
/// breath and read as one event: the target is taken and the column above it is
/// already moving. The pause lets the impact land as its own thing and gives
/// the fall something to be a consequence of.
pub const LAUNCH_HOLD_MS: f32 = 260.0;

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
/// A rocket reaching its target. Carried alongside the ordinary clear so the
/// front end can make more of it than a gem simply going away.
pub const EV_ROCKET_HIT: u8 = 10;
/// A match resolving, once per step of a chain, carrying that step's number.
///
/// This is the step itself rather than the gems it took, which is what a front
/// end wants for anything that belongs to the chain as a whole, the rising
/// chord being the one here. Gems also go away for reasons that are not a step
/// in a chain: a rocket takes one when it lands, and that is not a beat of the
/// music.
pub const EV_MATCH: u8 = 11;
/// Gems touching down after a fall, carrying the milliseconds until they do.
///
/// One per column per wave rather than one per gem: everything in a column
/// that drops the same distance arrives at the same instant and lands as a
/// single thump. A row clear drops most of the board by one row, and that is
/// three columns settling, not fifteen separate impacts.
pub const EV_LAND: u8 = 12;
/// The move budget running short, carrying how many are left.
///
/// Raised once, when the count first reaches [`LOW_MOVES`], and again only if
/// the count climbs back above the line and falls to it a second time. Under
/// Archipelago that will happen for real: moves are an item, and a level can
/// be handed more of them part way through.
pub const EV_LOW_MOVES: u8 = 13;

/// Where "running short" begins. A level that starts at or under this says so
/// before the player has touched anything, because with three moves to spend
/// the fact is part of the puzzle rather than a warning about it.
pub const LOW_MOVES: u32 = 5;

/// A brick taking a hit, carrying what is left of it: 1 cracked, 0 broken.
///
/// A brick takes at most one hit per clear, however many gems went off beside
/// it. Taking one per neighbor would mean a single ordinary match wiped a whole
/// brick out, which is not much of an obstacle.
pub const EV_BRICK: u8 = 14;

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
    /// Rockets are in the air. Nothing falls until they land, so that the cells
    /// they came from and the cells they hit collapse together, and then not
    /// for a beat longer, so the strike is not swallowed by its own aftermath.
    Launching { elapsed: f32 },
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
            Phase::Launching { .. } => 3,
            Phase::Falling { .. } => 4,
            Phase::Shuffling { .. } => 5,
            Phase::Finished => 6,
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

/// One rocket on its way somewhere.
#[derive(Clone, Copy, Debug)]
struct Launch {
    from: Pos,
    to: Pos,
    /// How long this one is in the air, which depends on how far it is going.
    flight_ms: f32,
    landed: bool,
}

/// A clear that is about to happen: what sets it off, and what it leaves behind.
struct Resolution {
    seeds: Vec<Pos>,
    creations: Vec<(Pos, Gem)>,
    /// Specials the swap has already cashed in; see [`Activation::spent`].
    spent: Vec<Pos>,
    /// Scatters the starting cells in time. A rainbow wants this; an ordinary
    /// match does not.
    jitter_ms: f32,
}

/// What swapping two specials together sets off.
struct Activation {
    seeds: Vec<Pos>,
    /// Rainbows this swap has already spent.
    ///
    /// A rainbow swapped deliberately has its power accounted for by the swap
    /// itself, and firing it again when the wave reaches it would take a
    /// second color as well. It stays a rainbow on the board, though, because
    /// it is about to pop in front of the player and popping as the gem it
    /// used to be is not what they did.
    spent: Vec<Pos>,
    jitter_ms: f32,
}

impl Activation {
    fn none() -> Self {
        Activation { seeds: Vec::new(), spent: Vec::new(), jitter_ms: 0.0 }
    }

    fn of(seeds: Vec<Pos>) -> Self {
        Activation { seeds, spent: Vec::new(), jitter_ms: 0.0 }
    }
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
    /// Cells popping during the current clear, each with the moment it pops,
    /// so a blast travels outward instead of taking everything at once.
    clearing: Vec<(Pos, f32)>,
    /// How long the current clear runs for, pops and spread together.
    clear_ms: f32,
    /// How long the current volley of rockets needs to reach its targets.
    launch_ms: f32,
    /// How long the current fall needs, set by whichever gem has furthest to go.
    fall_ms: f32,
    /// Specials to drop in once the clear finishes.
    pending: Vec<(Pos, Gem)>,
    /// Rockets in flight. Each keeps its own flight time and lands on it.
    launches: Vec<Launch>,
    /// Per-cell row and column a gem started falling from, which is not always
    /// its own column: see [`Board::collapse`].
    origin: Vec<(f32, f32)>,
    cascade: u32,
    swap: Option<(Pos, Pos)>,
    selected: Option<Pos>,
    /// Whether the short-on-moves warning has already gone out for this stretch
    /// of the level; see [`EV_LOW_MOVES`].
    warned_low_moves: bool,
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
            clear_ms: POP_MS,
            launch_ms: LAUNCH_RAMP_MS,
            fall_ms: FALL_ACCEL_MS,
            pending: Vec::new(),
            launches: Vec::new(),
            origin: Vec::new(),
            cascade: 0,
            swap: None,
            selected: None,
            warned_low_moves: false,
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
        self.clear_ms = POP_MS;
        self.launch_ms = LAUNCH_RAMP_MS;
        self.fall_ms = FALL_ACCEL_MS;
        self.pending.clear();
        self.launches.clear();
        self.cascade = 0;
        self.swap = None;
        self.selected = None;
        self.warned_low_moves = false;
        self.events.clear();
        self.deal();
        self.origin = self.settled_origin();
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
        // Before the phase loop, because an idle board runs none of it and a
        // level that opens on its last few moves has to say so anyway.
        self.announce_low_moves();
        let mut remaining = if dt_ms.is_finite() { dt_ms.clamp(0.0, 250.0) } else { 0.0 };

        // A long frame may span several phases; the cap stops a pathological
        // dt from grinding through an unbounded number of cascades at once.
        for _ in 0..24 {
            let (duration, elapsed) = match self.phase {
                Phase::Idle | Phase::Finished => break,
                Phase::Swapping { elapsed, .. } => (SWAP_MS, elapsed),
                Phase::Clearing { elapsed } => (self.clear_ms, elapsed),
                Phase::Launching { elapsed } => (self.launch_ms, elapsed),
                Phase::Falling { elapsed } => (self.fall_ms, elapsed),
                Phase::Shuffling { elapsed } => (SHUFFLE_MS, elapsed),
            };
            let advanced = elapsed + remaining;
            if let Phase::Launching { .. } = self.phase {
                self.land_arrivals(advanced);
            }
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
            Phase::Launching { .. } => Phase::Launching { elapsed },
            Phase::Falling { .. } => Phase::Falling { elapsed },
            Phase::Shuffling { .. } => Phase::Shuffling { elapsed },
            other => other,
        };
    }

    fn finish_phase(&mut self) {
        match self.phase {
            Phase::Swapping { reverting, .. } => self.finish_swap(reverting),
            Phase::Clearing { .. } => self.finish_clear(),
            Phase::Launching { .. } => self.finish_launch(),
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
        for (p, _) in std::mem::take(&mut self.clearing) {
            self.board.set_gem(p, None);
        }
        for (p, gem) in std::mem::take(&mut self.pending) {
            self.board.set_gem(p, Some(gem));
        }

        // A rocket falls like anything else and takes no part in the matches
        // going on around it. It flies once the cascade has run itself out.
        if !self.begin_settle_stage() {
            self.origin = self.settled_origin();
            self.begin_fall();
        }
    }

    /// Sends every rocket on the board at a target. Nothing falls while they
    /// are in the air, so the cells they leave and the cells they hit collapse
    /// in the same drop.
    fn begin_launch(&mut self, rockets: &[Pos]) -> bool {
        self.launches = self
            .pick_targets(rockets)
            .into_iter()
            .map(|(from, to)| Launch {
                from,
                to,
                flight_ms: flight_time(cells_between(from, to)),
                landed: false,
            })
            .collect();
        if self.launches.is_empty() {
            // Nowhere worth aiming: drop them rather than stall the board.
            for p in rockets {
                self.board.set_gem(*p, None);
            }
            return false;
        }

        // The phase runs until the last one lands, and then holds a beat before
        // gravity answers; each rocket lands on its own clock.
        let last = self.launches.iter().map(|l| l.flight_ms).fold(0.0_f32, f32::max);
        self.launch_ms = last + LAUNCH_HOLD_MS;

        for launch in self.launches.clone() {
            let mut event = Event::at(
                EV_SPECIAL_FIRED,
                launch.from,
                255,
                Special::Rocket,
                self.cascade.max(1),
            );
            // How long this one is in the air, so the front end can make its
            // whistle last exactly as far as it flies.
            event.value = launch.flight_ms.clamp(0.0, 65_535.0) as u16;
            self.events.push(event);
        }
        self.phase = Phase::Launching { elapsed: 0.0 };
        true
    }

    /// Lands every rocket whose flight has run out by `elapsed`.
    ///
    /// They arrive independently: a rocket going next door is gone long before
    /// one crossing the board, and should not hover on its target waiting for
    /// it. Gravity still holds off until the last one is down, so that nothing
    /// shifts under a rocket still in the air.
    fn land_arrivals(&mut self, elapsed: f32) {
        let cascade = self.cascade.max(1);
        for index in 0..self.launches.len() {
            let launch = self.launches[index];
            if launch.landed || elapsed < launch.flight_ms {
                continue;
            }
            self.launches[index].landed = true;
            self.board.set_gem(launch.from, None);

            let gem = match self.board.gem(launch.to) {
                Some(gem) => gem,
                None => continue,
            };
            if (gem.color as usize) < MAX_COLORS {
                self.progress.cleared[gem.color as usize] += 1;
            }
            self.board.peel_jelly(launch.to);
            self.events.push(Event::at(EV_CLEAR, launch.to, gem.color, gem.special, cascade));
            self.events.push(Event::at(
                EV_ROCKET_HIT,
                launch.to,
                gem.color,
                Special::Rocket,
                cascade,
            ));
            self.board.set_gem(launch.to, None);
            self.progress.score +=
                (SCORE_PER_GEM + SCORE_PER_SPECIAL_FIRED) * cascade as u64;
        }
        self.progress.jelly_left = self.board.jelly_remaining();
    }

    /// The last rocket is down and its beat has passed, so the board may
    /// finally settle.
    fn finish_launch(&mut self) {
        self.land_arrivals(f32::INFINITY);
        self.launches.clear();
        if !self.begin_settle_stage() {
            self.origin = self.settled_origin();
            self.begin_fall();
        }
    }

    /// Starts a fall, lasting as long as the gem with furthest to travel needs,
    /// and announces when each column touches down.
    ///
    /// Gems fall on one shared clock, so two that drop the same distance land
    /// together however far apart they are. Gathering them by column and by
    /// distance turns a collapse into a handful of landings. See [`EV_LAND`].
    fn begin_fall(&mut self) {
        let mut furthest = 0.0_f32;
        // (column, rows dropped, where that group touches down).
        let mut landings: Vec<(i32, i32, Pos)> = Vec::new();

        for p in self.board.positions() {
            let i = (p.r * self.board.cols + p.c) as usize;
            let from = self.origin.get(i).copied().unwrap_or((p.r as f32, p.c as f32));
            let drop = p.r as f32 - from.0;
            if drop <= 0.0 {
                continue;
            }
            furthest = furthest.max(drop);

            let rows = drop.round() as i32;
            match landings.iter_mut().find(|(c, d, _)| *c == p.c && *d == rows) {
                // Positions run down the board, so a later one is lower: the
                // gem of the group that actually meets what it is landing on.
                Some(entry) => entry.2 = p,
                None => landings.push((p.c, rows, p)),
            }
        }

        for (_, rows, p) in &landings {
            let color = self.board.gem(*p).map_or(255, |gem| gem.color);
            let mut event = Event::at(EV_LAND, *p, color, Special::None, self.cascade);
            event.value = fall_time(*rows as f32).min(65_535.0) as u16;
            self.events.push(event);
        }

        // The beat belongs at the end of the whole settle, not between its
        // stages: a gem that is about to slide off a shelf has not landed on
        // anything yet. So it is spent only on a stage with nothing following
        // it, and then only when a clear is waiting on the other side.
        let last = !self.board.will_move(&self.spec.rules);
        let waiting =
            last && !matching::find_matches(&self.board, &self.spec.rules).is_empty();
        let hold = if waiting { FALL_HOLD_MS } else { 0.0 };

        self.fall_ms = fall_time(furthest).max(1.0) + hold;
        self.phase = Phase::Falling { elapsed: 0.0 };
    }

    /// Knocks a hit off every brick this clear reached.
    ///
    /// A brick is reached two ways: a beam went through it, or something
    /// cleared in one of the four cells around it. Either way it takes exactly
    /// one hit per clear, so the same brick beside three gems of one match is
    /// cracked rather than demolished.
    fn strike_bricks(&mut self, blast: &matching::Detonation, cascade: u32) {
        let mut hit = blast.struck.clone();
        for p in &blast.cleared {
            for side in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                let neighbor = Pos::new(p.r + side.0, p.c + side.1);
                if self.board.brick(neighbor) > 0 && !hit.contains(&neighbor) {
                    hit.push(neighbor);
                }
            }
        }

        for p in hit {
            if let Some(left) = self.board.damage_brick(p) {
                let mut event = Event::at(EV_BRICK, p, 255, Special::None, cascade);
                event.value = left as u16;
                self.events.push(event);
            }
        }
    }

    /// Runs the next stage of a settle and puts the board into the fall that
    /// animates it, reporting whether there was a stage to run.
    fn begin_settle_stage(&mut self) -> bool {
        match self.board.settle_stage(&self.spec.rules, &mut self.rng) {
            Some(origin) => {
                self.origin = origin;
                self.begin_fall();
                true
            }
            None => false,
        }
    }

    /// Origins for a board that is already where it belongs: every gem starts
    /// from its own cell, so nothing animates.
    fn settled_origin(&self) -> Vec<(f32, f32)> {
        self.board.positions().map(|p| (p.r as f32, p.c as f32)).collect()
    }

    fn rockets_on_board(&self) -> Vec<Pos> {
        self.board
            .positions()
            .filter(|p| {
                self.board.gem(*p).map_or(false, |gem| gem.special == Special::Rocket)
            })
            .collect()
    }

    /// Picks what each rocket flies at. For now that is any other live gem,
    /// chosen at random; preference rules come later. Rockets are never aimed
    /// at each other, and no two pick the same gem.
    fn pick_targets(&mut self, rockets: &[Pos]) -> Vec<(Pos, Pos)> {
        let mut candidates: Vec<Pos> = self
            .board
            .positions()
            .filter(|p| {
                self.board.gem(*p).map_or(false, |gem| gem.special != Special::Rocket)
            })
            .collect();

        let mut launches = Vec::new();
        for from in rockets {
            if candidates.is_empty() {
                break;
            }
            let pick = self.rng.below(candidates.len() as u32) as usize;
            launches.push((*from, candidates.swap_remove(pick)));
        }
        launches
    }

    fn finish_fall(&mut self) {
        // A settle can take several stages: gems drop, then whatever is left
        // perched slides off, then what slid drops again. Nothing is matched
        // until all of that is over, because a board part way through settling
        // is not a board anybody has finished looking at.
        if self.begin_settle_stage() {
            return;
        }

        // Counted per match resolved, not per fall. A fall that turns up
        // nothing is not a step of the chain: rockets fire from exactly that
        // fall, so counting it would spend a step on the rocket and land the
        // clear its impact sets off a step higher than it earned.
        if let Some(resolution) = self.plan_resolution(None) {
            self.cascade += 1;
            self.events.push(Event::plain(EV_CASCADE, self.cascade.min(65535) as u16));
            self.begin_clear(resolution);
            return;
        }
        // The chain is spent, so any rockets waiting on the board go now.
        let rockets = self.rockets_on_board();
        if !rockets.is_empty() && self.begin_launch(&rockets) {
            return;
        }
        self.settle();
    }

    fn finish_shuffle(&mut self) {
        self.shuffle_board();
        self.origin = self.settled_origin();
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
        let mut jitter_ms = 0.0_f32;

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

        let mut spent: Vec<Pos> = Vec::new();
        if let Some((a, b)) = swap {
            let activation = self.swap_activation(a, b);
            seeds.extend(activation.seeds);
            spent.extend(activation.spent);
            jitter_ms = jitter_ms.max(activation.jitter_ms);
        }

        if seeds.is_empty() {
            None
        } else {
            Some(Resolution { seeds, creations, spent, jitter_ms })
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
        if group.squares > 0 {
            // No swap to pin it to, so the rocket takes the place of whichever
            // gem arrived last: the one that fell furthest, and of those the
            // lowest and furthest right.
            return group
                .cells
                .iter()
                .copied()
                .max_by_key(|cell| {
                    let i = (cell.r * self.board.cols + cell.c) as usize;
                    let fell = self
                        .origin
                        .get(i)
                        .map_or(0.0, |from| cell.r as f32 - from.0);
                    ((fell * 64.0) as i32, cell.r, cell.c)
                })
                .unwrap_or(group.pivot);
        }
        group.pivot
    }

    /// Extra cells cleared because of what the player swapped together.
    ///
    /// A special pushed against an ordinary gem does nothing: it waits for a
    /// match of its color. Two specials swapped together always set each other
    /// off, and the rainbow answers to anything.
    ///
    /// The rainbow against an ordinary gem takes that whole color, and any
    /// clearing gems standing in that color go off as they are swept up.
    /// Against a clearing gem it goes further: every gem of that color becomes
    /// a copy of it and they all fire at once. Against another rainbow it takes
    /// the board.
    ///
    /// Two ordinary specials simply both fire, with one wrinkle: two gems of
    /// the *same* orientation would clear the same line twice, so the gem the
    /// player actually moved turns the other way and clears across its own
    /// grain instead.
    fn swap_activation(&mut self, a: Pos, b: Pos) -> Activation {
        let (ga, gb) = match (self.board.gem(a), self.board.gem(b)) {
            (Some(ga), Some(gb)) => (ga, gb),
            _ => return Activation::none(),
        };

        let (rainbow, partner) = match (ga.special, gb.special) {
            (Special::Rainbow, Special::Rainbow) => {
                // Both are spent, so neither fires again on its own color.
                return Activation {
                    seeds: self.board.occupied(),
                    spent: vec![a, b],
                    jitter_ms: matching::RAINBOW_SPREAD_MS,
                };
            }
            (Special::Rainbow, _) => (a, gb),
            (_, Special::Rainbow) => (b, ga),
            (resting, moved) if resting.is_special() && moved.is_special() => {
                return Activation::of(self.pair_activation(a, b, resting, gb));
            }
            _ => return Activation::none(),
        };

        // The rainbow itself is not one of the color's gems, whatever color it
        // is carrying underneath: it is the thing doing the taking, and it is
        // seeded separately below.
        let color = partner.color;
        let targets: Vec<Pos> = self
            .board
            .positions()
            .filter(|p| *p != rainbow && self.board.color(*p) == Some(color))
            .collect();

        // A rainbow swapped against a clearing gem hands that gem's power to
        // the whole color before setting the lot off.
        if matches!(partner.special, Special::LineH | Special::LineV | Special::Cross) {
            for p in &targets {
                if let Some(gem) = self.board.gem(*p) {
                    self.board.set_gem(*p, Some(Gem { special: partner.special, ..gem }));
                }
            }
        }

        // The rainbow is spent on the color the player chose. Left unmarked, it
        // would be swept up as an unfired special and go off a second time
        // against whatever color happened to be commonest.
        let mut seeds = vec![rainbow];
        seeds.extend(targets);
        seeds.retain(|p| self.board.gem(*p).is_some());
        Activation { seeds, spent: vec![rainbow], jitter_ms: matching::RAINBOW_SPREAD_MS }
    }

    /// Two ordinary specials, set off against each other.
    ///
    /// The board has already been swapped, so the gem the player picked up now
    /// sits at `b`. When both face the same way that is the one that turns, so
    /// the pair clears a row and a column rather than the same line twice.
    fn pair_activation(&mut self, a: Pos, b: Pos, resting: Special, moved: Gem) -> Vec<Pos> {
        let turned = match (resting, moved.special) {
            (Special::LineH, Special::LineH) => Some(Special::LineV),
            (Special::LineV, Special::LineV) => Some(Special::LineH),
            _ => None,
        };
        if let Some(special) = turned {
            self.board.set_gem(b, Some(Gem { special, ..moved }));
        }
        vec![a, b]
    }


    /// Fires the clear: scores it, tallies it against the objectives, and puts
    /// the board into its popping animation.
    fn begin_clear(&mut self, resolution: Resolution) {
        let blast = matching::detonate(
            &self.board,
            &resolution.seeds,
            &resolution.spent,
            &mut self.rng,
            resolution.jitter_ms,
        );
        if blast.cleared.is_empty() {
            self.settle();
            return;
        }

        let cascade = self.cascade.max(1);
        self.events.push(Event::plain(EV_MATCH, cascade.min(65_535) as u16));
        let points = (blast.cleared.len() as u64 * SCORE_PER_GEM
            + blast.fired.len() as u64 * SCORE_PER_SPECIAL_FIRED
            + resolution.creations.len() as u64 * SCORE_PER_SPECIAL_MADE)
            * cascade as u64;
        self.progress.score += points;

        for (p, delay) in blast.cleared.iter().zip(blast.delays.iter()) {
            let gem = match self.board.gem(*p) {
                Some(gem) => gem,
                None => continue,
            };
            if (gem.color as usize) < MAX_COLORS {
                self.progress.cleared[gem.color as usize] += 1;
            }
            self.board.peel_jelly(*p);
            let mut event = Event::at(EV_CLEAR, *p, gem.color, gem.special, cascade);
            // The front end reads this to hold each cell's particles back until
            // the blast actually reaches it.
            event.value = delay.clamp(0.0, 65_535.0) as u16;
            self.events.push(event);
        }
        self.progress.jelly_left = self.board.jelly_remaining();
        self.strike_bricks(&blast, cascade);

        for (p, special) in &blast.fired {
            self.events.push(Event::at(EV_SPECIAL_FIRED, *p, 255, *special, cascade));
        }
        for (p, gem) in &resolution.creations {
            self.events.push(Event::at(EV_SPECIAL_MADE, *p, gem.color, gem.special, cascade));
        }

        // The clear runs until the furthest cell has finished popping.
        let last = blast.delays.iter().copied().fold(0.0_f32, f32::max);
        self.clear_ms = POP_MS + last;
        self.clearing = blast
            .cleared
            .iter()
            .copied()
            .zip(blast.delays.iter().copied())
            .collect();
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

    /// Says so, once, when the move budget gets short.
    ///
    /// Re-arms whenever the count climbs back over the line, so a level that is
    /// handed more moves part way through can warn again when it runs down a
    /// second time. A level with no move limit at all never warns, and neither
    /// does the last move running out, which ends the level and says that
    /// instead.
    fn announce_low_moves(&mut self) {
        if self.spec.moves == 0 || self.status != Status::Playing {
            return;
        }
        if self.moves_left > LOW_MOVES {
            self.warned_low_moves = false;
            return;
        }
        if self.warned_low_moves || self.moves_left == 0 {
            return;
        }
        self.warned_low_moves = true;
        self.events.push(Event::plain(EV_LOW_MOVES, self.moves_left.min(65_535) as u16));
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
            // Brick cells are open ground with something already standing on
            // it, so they are dealt around rather than into.
            let cells: Vec<Pos> =
                self.board.positions().filter(|p| self.board.brick(*p) == 0 && self.board.is_open(*p)).collect();
            for p in cells {
                let mut color = 0;
                // A handful of tries is enough to dodge a starting match; if
                // the colors run out we accept it and let the shuffle catch it.
                for _ in 0..12 {
                    color = self.rng.below(colors) as u8;
                    if !self.would_start_a_shape(p, color) {
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

    /// Whether placing `color` here completes a run or a 2x2 with the cells
    /// already dealt above and to the left.
    fn would_start_a_shape(&self, p: Pos, color: u8) -> bool {
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
        if run_back(0, -1) >= min || run_back(-1, 0) >= min {
            return true;
        }
        // The deal fills top to bottom, left to right, so the only square this
        // gem can close is the one above and to its left.
        self.spec.rules.square_match
            && self.board.color(Pos::new(p.r - 1, p.c)) == Some(color)
            && self.board.color(Pos::new(p.r, p.c - 1)) == Some(color)
            && self.board.color(Pos::new(p.r - 1, p.c - 1)) == Some(color)
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
    /// A brick stands here. With [`Game::FLAG_CRACKED`] as well, it is the
    /// cracked one, which the next hit breaks.
    pub const FLAG_BRICK: u8 = 8;
    pub const FLAG_CRACKED: u8 = 16;

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
            if cell.brick > 0 {
                flags |= Self::FLAG_BRICK;
                if cell.brick == 1 {
                    flags |= Self::FLAG_CRACKED;
                }
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
                for (p, delay) in self.clearing.clone() {
                    if elapsed < delay {
                        // Its turn has not come round yet.
                        continue;
                    }
                    let t = ((elapsed - delay) / POP_MS).clamp(0.0, 1.0);
                    // A brief swell, then the gem shrinks out.
                    let scale =
                        if t < 0.25 { 1.0 + t * 0.6 } else { (1.15 - (t - 0.25) * 1.5).max(0.0) };
                    let i = (p.r * self.board.cols + p.c) as usize;
                    if i * 3 + 2 < self.offs_buf.len() {
                        self.offs_buf[i * 3 + 2] = scale;
                        self.cells_buf[i * 4 + 3] |= Self::FLAG_CLEARING;
                    }
                }
            }
            Phase::Launching { elapsed } => {
                for launch in self.launches.clone() {
                    if launch.landed {
                        continue;
                    }
                    let (from, to) = (launch.from, launch.to);
                    let distance = cells_between(from, to).max(0.001);
                    let flown = (flown_cells(elapsed) / distance).clamp(0.0, 1.0);
                    self.set_offset(
                        from,
                        (to.c - from.c) as f32 * flown,
                        (to.r - from.r) as f32 * flown,
                    );
                    // The target is left entirely alone. It used to start
                    // shrinking once the rocket was most of the way there,
                    // which on a long flight meant it was visibly cringing for
                    // half a second before anything reached it, and told the
                    // player where the rocket was going before it arrived.
                    // Nothing happens to it until it is hit.
                }
            }
            Phase::Falling { elapsed } => {
                let fallen = fallen_cells(elapsed);
                for p in self.board.positions() {
                    let i = (p.r * self.board.cols + p.c) as usize;
                    let from = self.origin.get(i).copied().unwrap_or((p.r as f32, p.c as f32));
                    let drop = p.r as f32 - from.0;
                    if drop <= 0.0 {
                        continue;
                    }
                    // Still above its cell by whatever it has left to fall.
                    let left = (drop - fallen).max(0.0);
                    self.offs_buf[i * 3 + 1] = -left;
                    // A gem that spilled sideways slides across as it drops,
                    // in step with the fall rather than on a clock of its own,
                    // so the two read as one movement.
                    let across = from.1 - p.c as f32;
                    if across != 0.0 {
                        self.offs_buf[i * 3] = across * (left / drop);
                    }
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

/// Straight-line distance between two cells, in cells.
fn cells_between(a: Pos, b: Pos) -> f32 {
    let dr = (b.r - a.r) as f32;
    let dc = (b.c - a.c) as f32;
    (dr * dr + dc * dc).sqrt()
}

/// Distance covered after `elapsed` milliseconds by something that accelerates
/// evenly for `ramp_ms` and then holds `speed`.
fn travelled(elapsed: f32, ramp_ms: f32, speed: f32) -> f32 {
    if elapsed <= 0.0 {
        0.0
    } else if elapsed < ramp_ms {
        0.5 * speed * elapsed * elapsed / ramp_ms
    } else {
        0.5 * speed * ramp_ms + speed * (elapsed - ramp_ms)
    }
}

/// How long that takes to cover `distance`. A short hop is over before the
/// ramp finishes and never reaches full speed.
fn travel_time(distance: f32, ramp_ms: f32, speed: f32) -> f32 {
    let ramp_distance = 0.5 * speed * ramp_ms;
    if distance <= ramp_distance {
        (2.0 * distance * ramp_ms / speed).sqrt()
    } else {
        ramp_ms + (distance - ramp_distance) / speed
    }
}

/// How far a rocket has flown after `elapsed` milliseconds.
fn flown_cells(elapsed: f32) -> f32 {
    travelled(elapsed, LAUNCH_RAMP_MS, LAUNCH_SPEED)
}

/// How long a rocket needs to cover `distance` cells.
fn flight_time(distance: f32) -> f32 {
    travel_time(distance, LAUNCH_RAMP_MS, LAUNCH_SPEED)
}

/// How far a gem has fallen after `elapsed` milliseconds.
fn fallen_cells(elapsed: f32) -> f32 {
    travelled(elapsed, FALL_ACCEL_MS, FALL_SPEED)
}

/// How long a gem needs to drop `distance` rows.
fn fall_time(distance: f32) -> f32 {
    travel_time(distance, FALL_ACCEL_MS, FALL_SPEED)
}

fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::levels;
    use crate::rules::{RefillMode, SpecialSet};

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

    /// Steps until the first batch of clears and hands back just that batch, so
    /// a test can weigh one clear rather than everything the cascade went on to
    /// do afterwards.
    fn first_clear(game: &mut Game) -> Vec<Event> {
        for _ in 0..400 {
            game.update(16.0);
            let batch: Vec<Event> =
                game.events().iter().filter(|e| e.kind == EV_CLEAR).copied().collect();
            if !batch.is_empty() {
                return batch;
            }
        }
        panic!("nothing ever cleared");
    }

    /// The same for the first fall, so a test weighs one collapse settling
    /// rather than every landing the cascade went on to make.
    fn first_landing(game: &mut Game) -> Vec<Event> {
        for _ in 0..400 {
            game.update(16.0);
            let batch: Vec<Event> =
                game.events().iter().filter(|e| e.kind == EV_LAND).copied().collect();
            if !batch.is_empty() {
                return batch;
            }
        }
        panic!("nothing ever landed");
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
    fn a_row_of_four_leaves_a_downward_clearer_under_the_swap() {
        let mut game = Game::new(spec(5, 5, 6, 10), 6);
        // Row 0 reads 1,1,2,1,3, no match yet. Lifting the 1 at (1,2) into the
        // gap completes four across, and the special should land on the cell
        // the player moved.
        paint(&mut game, &["11213", "30120", "23401", "12340", "34012"]);
        assert!(matching::find_matches(&game.board, game.rules()).is_empty());
        assert!(game.try_swap(Pos::new(0, 2), Pos::new(1, 2)));
        let events = settle(&mut game);
        // Later cascades may well earn their own gems, so only the first one
        // says anything about the run the player made.
        let made: Vec<&Event> = events.iter().filter(|e| e.kind == EV_SPECIAL_MADE).collect();
        assert!(!made.is_empty(), "a four-run should leave a gem behind");
        assert_eq!(
            made[0].special,
            Special::LineV.code(),
            "a row of four is finished with a vertical slide, so it clears downward"
        );
        assert_eq!((made[0].r, made[0].c), (0, 2), "it lands under the swap");
    }

    /// A board where swapping (1,1) with (1,2) closes a 2x2 of color 1 in the
    /// corner and forms nothing else.
    fn square_board() -> [&'static str; 4] {
        ["1123", "1214", "4505", "5430"]
    }

    #[test]
    fn a_two_by_two_leaves_a_rocket_where_the_player_swapped() {
        let mut game = Game::new(spec(4, 4, 6, 10), 71);
        paint(&mut game, &square_board());
        assert!(matching::find_matches(&game.board, game.rules()).is_empty());
        assert!(game.try_swap(Pos::new(1, 1), Pos::new(1, 2)));

        let events = settle(&mut game);
        let made: Vec<&Event> = events.iter().filter(|e| e.kind == EV_SPECIAL_MADE).collect();
        assert!(!made.is_empty(), "a 2x2 should leave a rocket");
        assert_eq!(made[0].special, Special::Rocket.code());
        assert_eq!((made[0].r, made[0].c), (1, 1), "it takes the swapped gem's place");
    }

    #[test]
    fn the_board_settles_before_the_rocket_flies() {
        // The rocket rides out the cascade like an ordinary gem and only
        // launches once there is nothing left to clear.
        let mut game = Game::new(spec(4, 4, 6, 10), 72);
        paint(&mut game, &square_board());
        assert!(game.try_swap(Pos::new(1, 1), Pos::new(1, 2)));

        let mut saw_launch = false;
        for _ in 0..400 {
            if let Phase::Launching { .. } = game.phase() {
                saw_launch = true;
                break;
            }
            game.update(16.0);
        }
        assert!(saw_launch, "the board never entered its launch phase");

        assert_eq!(
            game.rockets_on_board().len(),
            1,
            "the rocket should have survived the cascade to fly"
        );
        assert!(
            game.board.positions().all(|p| !game.board.is_open(p) || game.board.gem(p).is_some()),
            "and the board should have filled in before it left"
        );
        assert!(
            matching::find_matches(&game.board, game.rules()).is_empty(),
            "nothing should still be waiting to clear"
        );
    }

    #[test]
    fn a_rocket_falls_like_any_other_gem() {
        let mut game = Game::new(spec(4, 4, 6, 78), 78);
        let perch = Pos::new(1, 0);
        let gem = game.board.gem(perch).expect("the board is full");
        game.board.set_gem(perch, Some(Gem { special: Special::Rocket, ..gem }));
        game.board.set_gem(Pos::new(2, 0), None);
        game.board.set_gem(Pos::new(3, 0), None);

        let rules = *game.rules();
        while game.board.settle_stage(&rules, &mut game.rng).is_some() {}

        assert_eq!(
            game.board.gem(Pos::new(3, 0)).map(|g| g.special),
            Some(Special::Rocket),
            "it should have dropped to the floor with everything else"
        );
    }

    #[test]
    fn a_rocket_is_not_swept_up_by_a_match() {
        // Left matchable, a rocket could be cleared by a cascade before it ever
        // fired, quietly costing the player what they earned.
        let mut game = Game::new(spec(4, 4, 6, 79), 79);
        paint(&mut game, &["1112", "2345", "3456", "4567"]);
        assert_eq!(
            matching::find_matches(&game.board, game.rules()).len(),
            1,
            "three ones across the top is a match"
        );

        game.board.set_gem(Pos::new(0, 1), Some(Gem { color: 1, special: Special::Rocket }));
        assert!(
            matching::find_matches(&game.board, game.rules()).is_empty(),
            "with a rocket in the middle of them, it is not"
        );
    }

    #[test]
    fn neither_end_of_the_flight_moves_until_impact() {
        let mut game = Game::new(spec(4, 4, 6, 10), 77);
        paint(&mut game, &square_board());
        assert!(game.try_swap(Pos::new(1, 1), Pos::new(1, 2)));

        for _ in 0..400 {
            if let Phase::Launching { .. } = game.phase() {
                break;
            }
            game.update(16.0);
        }
        let launch = *game.launches.first().expect("a rocket should be in the air");
        let (from, to) = (launch.from, launch.to);
        let target_before = game.board.gem(to);
        assert!(target_before.is_some());

        // Part way through the flight, nothing at either end has shifted.
        game.update(16.0 * 6.0);
        assert!(matches!(game.phase(), Phase::Launching { .. }), "still in the air");
        assert_eq!(game.board.gem(to), target_before, "the target waited to be hit");
        assert_eq!(
            game.board.gem(from).map(|g| g.special),
            Some(Special::Rocket),
            "and the rocket has not been dropped from its cell"
        );

        settle(&mut game);
        assert!(
            game.board.positions().all(|p| !game.board.is_open(p) || game.board.gem(p).is_some()),
            "everything fills in once it lands"
        );
    }

    #[test]
    fn a_rocket_never_exceeds_its_top_speed() {
        // The complaint that started this: without a cap, a long shot simply
        // moves faster and is over before you can see it.
        let step = 16.0;
        let mut previous = 0.0;
        let mut t = 0.0;
        while t < 4_000.0 {
            let speed = (flown_cells(t + step) - flown_cells(t)) / step;
            assert!(
                speed <= LAUNCH_SPEED + 1e-6,
                "at {t}ms the rocket was doing {speed} cells/ms"
            );
            let flown = flown_cells(t);
            assert!(flown >= previous, "a rocket never goes backwards");
            previous = flown;
            t += step;
        }
        assert!(flown_cells(20.0) < flown_cells(LAUNCH_RAMP_MS) * 0.05, "it starts slowly");
    }

    #[test]
    fn a_longer_shot_takes_longer_instead_of_flying_faster() {
        let near = flight_time(1.0);
        let far = flight_time(8.0);
        assert!(far > near, "crossing the board should take longer than going next door");
        // Once up to speed the extra ground is covered at exactly the cap.
        let extra_time = flight_time(8.0) - flight_time(7.0);
        assert!((extra_time - 1.0 / LAUNCH_SPEED).abs() < 1e-3);
    }

    #[test]
    fn a_rocket_takes_exactly_one_gem_somewhere_else() {
        let mut game = Game::new(spec(4, 4, 6, 10), 73);
        paint(&mut game, &square_board());
        game.try_swap(Pos::new(1, 1), Pos::new(1, 2));

        // Step to the moment of impact and note what it was aimed at.
        let mut target = None;
        for _ in 0..400 {
            if let Phase::Launching { .. } = game.phase() {
                target = game.launches.first().map(|launch| launch.to);
                break;
            }
            game.update(16.0);
        }
        let target = target.expect("a rocket should have been launched");
        assert_ne!(target, Pos::new(1, 1), "a rocket does not target its own cell");
    }

    #[test]
    fn rockets_never_aim_at_each_other() {
        let mut game = Game::new(spec(4, 4, 6, 10), 74);
        let rockets = [Pos::new(0, 0), Pos::new(3, 3)];
        for p in rockets {
            let gem = game.board.gem(p).expect("the board is full");
            game.board.set_gem(p, Some(Gem { special: Special::Rocket, ..gem }));
        }
        let launches = game.pick_targets(&rockets);
        assert_eq!(launches.len(), 2, "both rockets should find something");
        for (from, to) in &launches {
            assert!(!rockets.contains(to), "a rocket was aimed at another rocket");
            assert_ne!(from, to);
        }
        assert_ne!(launches[0].1, launches[1].1, "two rockets should not share a target");
    }

    /// A 6x6 Latin square: no runs, no 2x2s, and exactly six cells of each
    /// color, which makes rainbow effects easy to count.
    fn latin_board() -> [&'static str; 6] {
        ["012345", "123450", "234501", "345012", "450123", "501234"]
    }

    #[test]
    fn a_rainbow_swapped_against_a_gem_takes_that_color_and_nothing_else() {
        let mut game = Game::new(spec(6, 6, 6, 10), 81);
        paint(&mut game, &latin_board());
        assert!(matching::find_matches(&game.board, game.rules()).is_empty());
        // (0,0) is color 0; make it a rainbow and swap it onto the 1 beside it.
        game.board.set_gem(Pos::new(0, 0), Some(Gem { color: 0, special: Special::Rainbow }));

        assert!(game.try_swap(Pos::new(0, 0), Pos::new(0, 1)));
        let cleared = first_clear(&mut game);

        // Six 1s plus the spent rainbow itself. Anything more in this first
        // clear would mean it fired a second time against another color.
        let ones = cleared.iter().filter(|e| e.color == 1).count();
        assert_eq!(ones, 6, "every gem of the chosen color should go");
        assert_eq!(cleared.len(), 7, "the chosen color and the rainbow, nothing else");
    }

    #[test]
    fn a_spent_rainbow_still_pops_as_a_rainbow() {
        // Spending it used to mean turning it back into the gem underneath,
        // which is correct for the rules and wrong for the eye: it is about to
        // go off in front of the player, and for the length of the animation
        // they would watch a plain gem that was never there.
        let mut game = Game::new(spec(6, 6, 6, 10), 83);
        paint(&mut game, &latin_board());
        game.board.set_gem(Pos::new(0, 0), Some(Gem { color: 0, special: Special::Rainbow }));

        assert!(game.try_swap(Pos::new(0, 0), Pos::new(0, 1)));
        // The swap carries it across, and it clears from there.
        let slot = (0 * game.board.cols + 1) as usize;

        let mut frames = 0;
        for _ in 0..400 {
            game.update(16.0);
            if !matches!(game.phase(), Phase::Clearing { .. }) {
                if frames > 0 {
                    break;
                }
                continue;
            }
            frames += 1;
            assert_eq!(
                game.cells_bytes()[slot * 4 + 1],
                Special::Rainbow.code(),
                "at frame {frames} it was showing as a plain gem while it popped"
            );
        }
        assert!(frames > 3, "the clear should have lasted several frames");
    }

    #[test]
    fn a_rainbow_sets_off_clearing_gems_it_sweeps_up() {
        let mut game = Game::new(spec(6, 6, 6, 10), 82);
        paint(&mut game, &latin_board());
        game.board.set_gem(Pos::new(0, 0), Some(Gem { color: 0, special: Special::Rainbow }));
        // One of the 1s is a row clearer, and it should go off as it is taken.
        game.board.set_gem(Pos::new(2, 5), Some(Gem { color: 1, special: Special::LineH }));

        assert!(game.try_swap(Pos::new(0, 0), Pos::new(0, 1)));
        let cleared = first_clear(&mut game);
        assert!(
            cleared.len() > 7,
            "the row clearer should have taken its row along with the color, saw {}",
            cleared.len()
        );
    }

    #[test]
    fn a_rainbow_swapped_against_a_clearing_gem_spreads_it_to_the_whole_color() {
        let mut game = Game::new(spec(6, 6, 6, 10), 83);
        paint(&mut game, &latin_board());
        game.board.set_gem(Pos::new(0, 0), Some(Gem { color: 0, special: Special::Rainbow }));
        game.board.set_gem(Pos::new(0, 1), Some(Gem { color: 1, special: Special::Cross }));

        assert!(game.try_swap(Pos::new(0, 0), Pos::new(0, 1)));
        let events = settle(&mut game);

        let crosses = events
            .iter()
            .filter(|e| e.kind == EV_SPECIAL_FIRED && e.special == Special::Cross.code())
            .count();
        assert!(
            crosses >= 6,
            "every gem of that color should have become a cross and fired, saw {crosses}"
        );
        // Six crosses sitting on a Latin square cover every row and column.
        let cleared = events.iter().filter(|e| e.kind == EV_CLEAR).count();
        assert!(cleared >= 30, "that should take almost the whole board, saw {cleared}");
    }

    #[test]
    fn an_ordinary_match_pops_as_one() {
        let mut game = Game::new(spec(4, 4, 6, 10), 101);
        paint(&mut game, &["1200", "1300", "2100", "0000"]);
        assert!(game.try_swap(Pos::new(2, 0), Pos::new(2, 1)));
        let cleared = first_clear(&mut game);
        assert!(
            cleared.iter().all(|e| e.value == 0),
            "three in a row should go together, not in sequence"
        );
    }

    #[test]
    fn a_chain_of_specials_propagates_across_the_board() {
        // Each blast starts when the gem carrying it pops, so a chain of them
        // accumulates delay and the clear really does travel. This pins the
        // arithmetic: 7 cells across, then 4 down, then 7 back across.
        let mut game = Game::new(spec(8, 8, 6, 10), 104);
        game.board.set_gem(Pos::new(3, 0), Some(Gem { color: 0, special: Special::LineH }));
        game.board.set_gem(Pos::new(3, 7), Some(Gem { color: 1, special: Special::LineV }));
        game.board.set_gem(Pos::new(7, 7), Some(Gem { color: 2, special: Special::LineH }));

        let blast = matching::detonate(&game.board, &[Pos::new(3, 0)], &[], &mut game.rng, 0.0);
        let step = matching::SPREAD_STEP_MS;

        let delay_at = |p: Pos| {
            blast
                .cleared
                .iter()
                .position(|cell| *cell == p)
                .map(|i| blast.delays[i])
                .expect("cell should have been cleared")
        };

        assert_eq!(delay_at(Pos::new(3, 0)), 0.0, "the first gem goes immediately");
        assert_eq!(delay_at(Pos::new(3, 7)), 7.0 * step, "seven cells along the row");
        assert_eq!(delay_at(Pos::new(7, 7)), 11.0 * step, "then four more down the column");
        assert_eq!(
            delay_at(Pos::new(7, 0)),
            18.0 * step,
            "and seven back along the bottom row"
        );
        let widest = blast.delays.iter().copied().fold(0.0_f32, f32::max);
        assert_eq!(widest, 18.0 * step);
    }

    #[test]
    fn a_rainbow_is_not_swept_up_by_a_match_either() {
        // It answers to every color, so it belongs to none and lines up with
        // nothing. Its own way out is a swap, or another special catching it.
        let mut game = Game::new(spec(4, 4, 6, 113), 113);
        paint(&mut game, &["1112", "2345", "3456", "4567"]);
        assert_eq!(matching::find_matches(&game.board, game.rules()).len(), 1);

        game.board.set_gem(Pos::new(0, 1), Some(Gem { color: 1, special: Special::Rainbow }));
        assert!(
            matching::find_matches(&game.board, game.rules()).is_empty(),
            "a rainbow in the middle of them breaks the run"
        );
    }

    #[test]
    fn a_rainbow_scatters_its_clears_in_time() {
        let mut game = Game::new(spec(6, 6, 6, 10), 102);
        paint(&mut game, &latin_board());
        game.board.set_gem(Pos::new(0, 0), Some(Gem { color: 0, special: Special::Rainbow }));

        assert!(game.try_swap(Pos::new(0, 0), Pos::new(0, 1)));
        let cleared = first_clear(&mut game);
        let delays: Vec<u16> = cleared.iter().map(|e| e.value).collect();
        let distinct: std::collections::BTreeSet<u16> = delays.iter().copied().collect();
        assert!(
            distinct.len() > 2,
            "a rainbow's cells should go off at different moments, saw {delays:?}"
        );
        assert!(
            delays.iter().all(|d| (*d as f32) < matching::RAINBOW_SPREAD_MS),
            "and all within its window"
        );
    }

    #[test]
    fn the_target_is_untouched_until_the_rocket_arrives() {
        // Nothing may happen to the gem being aimed at until it is actually
        // hit: not a shrink, not a flag. On a long flight an early flinch is
        // both wrong to look at and a giveaway of where the rocket is headed.
        let mut game = Game::new(spec(8, 8, 6, 10), 114);
        let from = Pos::new(0, 0);
        let to = Pos::new(7, 7);
        let gem = game.board.gem(from).expect("the board is full");
        game.board.set_gem(from, Some(Gem { special: Special::Rocket, ..gem }));

        let launch = Launch {
            from,
            to,
            flight_ms: flight_time(cells_between(from, to)),
            landed: false,
        };
        game.launches = vec![launch];
        game.launch_ms = launch.flight_ms;
        game.phase = Phase::Launching { elapsed: 0.0 };

        let slot = (to.r * game.board.cols + to.c) as usize;
        let mut frames = 0;
        while matches!(game.phase(), Phase::Launching { .. }) && frames < 400 {
            game.update(16.0);
            frames += 1;
            if !matches!(game.phase(), Phase::Launching { .. }) {
                break;
            }
            assert_eq!(
                game.offsets()[slot * 3 + 2],
                1.0,
                "the target shrank at frame {frames}, with the rocket still in the air"
            );
            assert_eq!(
                game.cells_bytes()[slot * 4 + 3] & Game::FLAG_CLEARING,
                0,
                "the target was marked as clearing before it was hit"
            );
        }
        assert!(frames > 20, "a flight across the board should last many frames");
    }

    #[test]
    fn a_match_announces_its_place_in_the_chain() {
        let mut game = Game::new(spec(4, 4, 6, 10), 115);
        paint(&mut game, &["1200", "1300", "2100", "0000"]);
        assert!(game.try_swap(Pos::new(2, 0), Pos::new(2, 1)));
        let events = settle(&mut game);

        let steps: Vec<&Event> = events.iter().filter(|e| e.kind == EV_MATCH).collect();
        assert!(!steps.is_empty(), "a match should announce itself");
        assert_eq!(steps[0].value, 1, "the first step of a chain is the first chord");
        for pair in steps.windows(2) {
            assert!(pair[1].value > pair[0].value, "a chain only climbs");
        }
    }

    #[test]
    fn a_rocket_landing_is_not_a_step_in_the_chain() {
        // It takes a gem with it, but that is not a beat of the music: the
        // chord should neither advance nor sound again.
        let mut game = Game::new(spec(8, 8, 6, 10), 116);
        let from = Pos::new(0, 0);
        let to = Pos::new(4, 4);
        let gem = game.board.gem(from).expect("the board is full");
        game.board.set_gem(from, Some(Gem { special: Special::Rocket, ..gem }));
        let launch = Launch {
            from,
            to,
            flight_ms: flight_time(cells_between(from, to)),
            landed: false,
        };
        game.launches = vec![launch];
        game.launch_ms = launch.flight_ms;
        game.phase = Phase::Launching { elapsed: 0.0 };

        // Watch only the flight, so nothing the later collapse does can muddy it.
        let mut hits = 0;
        let mut steps = 0;
        for _ in 0..400 {
            if !matches!(game.phase(), Phase::Launching { .. }) {
                break;
            }
            game.update(16.0);
            hits += game.events().iter().filter(|e| e.kind == EV_ROCKET_HIT).count();
            steps += game.events().iter().filter(|e| e.kind == EV_MATCH).count();
        }

        assert_eq!(hits, 1, "the rocket should have landed");
        assert_eq!(steps, 0, "and raised no step of the chain doing it");
    }

    #[test]
    fn rockets_arrive_and_go_off_independently() {
        // A rocket going next door should be gone long before one crossing the
        // board, not hovering on its target waiting for it.
        let mut game = Game::new(spec(8, 8, 6, 10), 112);
        let near = Launch {
            from: Pos::new(0, 0),
            to: Pos::new(0, 1),
            flight_ms: flight_time(1.0),
            landed: false,
        };
        let far = Launch {
            from: Pos::new(7, 0),
            to: Pos::new(7, 7),
            flight_ms: flight_time(7.0),
            landed: false,
        };
        assert!(far.flight_ms > near.flight_ms * 2.0, "the flights should differ plainly");

        for p in [near.from, far.from] {
            let gem = game.board.gem(p).expect("the board is full");
            game.board.set_gem(p, Some(Gem { special: Special::Rocket, ..gem }));
        }
        game.launches = vec![near, far];
        game.launch_ms = far.flight_ms + LAUNCH_HOLD_MS;
        game.phase = Phase::Launching { elapsed: 0.0 };

        // Run the clock to just past the short flight.
        let mut hits = 0;
        for _ in 0..(((near.flight_ms + 40.0) / 16.0).ceil() as usize) {
            game.update(16.0);
            hits += game.events().iter().filter(|e| e.kind == EV_ROCKET_HIT).count();
        }

        assert_eq!(hits, 1, "only the near rocket should have gone off");
        assert!(game.launches[0].landed, "the short flight should be down");
        assert!(!game.launches[1].landed, "the long one should still be in the air");
        assert!(game.board.gem(near.to).is_none(), "its target should have gone");
        assert!(game.board.gem(far.to).is_some(), "the far target is not hit yet");
        assert!(
            matches!(game.phase(), Phase::Launching { .. }),
            "and the board should still be waiting on the second"
        );

        for _ in 0..400 {
            if !matches!(game.phase(), Phase::Launching { .. }) {
                break;
            }
            game.update(16.0);
            hits += game.events().iter().filter(|e| e.kind == EV_ROCKET_HIT).count();
        }
        assert_eq!(hits, 2, "both should have gone off by the end");

        let _ = settle(&mut game);
        assert!(
            game.board.positions().all(|p| !game.board.is_open(p) || game.board.gem(p).is_some()),
            "and the board fills in once they are all down"
        );
    }

    #[test]
    fn a_rocket_does_not_advance_the_chain() {
        // The chord climbs one step per match resolved. A rocket firing is not
        // a match, so the clear its impact sets off is the next step of the
        // chain rather than the one after that.
        //
        // Refill is off so the collapse is entirely determined: nothing random
        // drops in, and the board after the strike is the board below.
        let mut game = Game::new(spec(4, 3, 6, 10), 130);
        game.spec.rules.refill = RefillMode::None;
        //  R 1 2      . 1 .
        //  0 1 1  ->  0 1 2   once the rocket leaves (0,0) and takes (3,2),
        //  1 0 2      1 0 1   column 2 drops by one and the bottom row reads
        //  2 2 0      2 2 2   three alike.
        paint(&mut game, &["012", "011", "102", "220"]);
        let rocket = game.board.gem(Pos::new(0, 0)).expect("the board is full");
        game.board.set_gem(Pos::new(0, 0), Some(Gem { special: Special::Rocket, ..rocket }));
        assert!(matching::find_matches(&game.board, game.rules()).is_empty());

        // As though one match has already resolved and left this rocket
        // behind: the board is mid-chain at step one, falling.
        game.cascade = 1;
        game.origin = game.settled_origin();
        game.phase = Phase::Falling { elapsed: 0.0 };

        // Run the fall out through the real path: the board finds no match and
        // sends the rocket. Only its target is then replaced, because it is
        // picked at random and this board is built around one in particular.
        for _ in 0..40 {
            if matches!(game.phase(), Phase::Launching { .. }) {
                break;
            }
            game.update(16.0);
        }
        assert!(matches!(game.phase(), Phase::Launching { .. }), "the rocket should have fired");
        let from = Pos::new(0, 0);
        let to = Pos::new(3, 2);
        let shot =
            Launch { from, to, flight_ms: flight_time(cells_between(from, to)), landed: false };
        game.launches = vec![shot];
        game.launch_ms = shot.flight_ms + LAUNCH_HOLD_MS;
        game.phase = Phase::Launching { elapsed: 0.0 };

        let events = settle(&mut game);
        let steps: Vec<u16> =
            events.iter().filter(|e| e.kind == EV_MATCH).map(|e| e.value).collect();
        assert_eq!(
            steps,
            vec![2],
            "one match resolved after the strike, so it is step two of the chain",
        );
    }

    #[test]
    fn a_landing_and_the_clear_it_sets_off_are_separate_events() {
        // Refill off, so the collapse is entirely determined. The middle column
        // holds three alike in its middle; clearing them drops the 7 at its top
        // down to row three, where two more 7s are already waiting.
        //
        //   1 7 2                 . . 2
        //   2 5 1                 . . 1
        //   3 5 3      ->         . . 3
        //   7 5 7                 7 7 7   <- the clear this beat comes before
        //   2 1 2                 2 1 2
        //   3 4 3                 3 4 3
        let mut game = Game::new(spec(6, 3, 8, 10), 140);
        game.spec.rules.refill = RefillMode::None;
        paint(&mut game, &["172", "251", "353", "757", "212", "343"]);

        game.cascade = 1;
        let resolution = game.plan_resolution(None).expect("the painted board has a match");
        game.begin_clear(resolution);

        // Walked in small steps: what is measured is the gap between two
        // moments, not what the board looks like at the end. The first clear's
        // own announcement is discarded by the first update, so the match seen
        // here is the one the landing set off.
        let step = 4.0;
        let (mut now, mut landed, mut matched) = (0.0_f32, None, None);
        for _ in 0..600 {
            game.update(step);
            now += step;
            for event in game.events() {
                // Raised as the fall begins, carrying how long until it lands.
                if event.kind == EV_LAND {
                    let at = now + event.value as f32;
                    landed = Some(landed.map_or(at, |seen: f32| seen.max(at)));
                }
                if event.kind == EV_MATCH {
                    matched = Some(now);
                }
            }
            if matched.is_some() {
                break;
            }
        }

        let landed = landed.expect("nothing ever fell");
        let matched = matched.expect("the drop never set anything off");
        let beat = matched - landed;
        // Against a floor of its own as well as the constant, because a check
        // that only compares the two agrees just as happily with a hold of zero.
        assert!(beat > 60.0, "only {beat:.0}ms between the landing and the clear");
        assert!(
            beat >= FALL_HOLD_MS - step * 2.0,
            "the beat is {beat:.0}ms, short of the {FALL_HOLD_MS} it is set to",
        );
    }

    /// A board with a brick in the middle of the bottom row and a match lined
    /// up beside it.
    fn bricked_game(seed: u64) -> Game {
        let mut game = Game::new(spec(4, 5, 8, 10), seed);
        game.spec.rules.refill = RefillMode::None;
        game.board = Board::from_layout(&[".....", ".....", ".....", "..B.."]);
        game.cascade = 1;
        game
    }

    #[test]
    fn a_brick_cracks_before_it_breaks() {
        let mut game = bricked_game(160);
        // Three alike immediately left of the brick at (3,2).
        for c in 0..3 {
            game.board.set_gem(Pos::new(2, c), Some(Gem::plain(1)));
        }
        assert_eq!(game.board.brick(Pos::new(3, 2)), 2, "it starts whole");

        let resolution = game.plan_resolution(None).expect("the row should match");
        game.begin_clear(resolution);
        let broke: Vec<&Event> = game.events().iter().filter(|e| e.kind == EV_BRICK).collect();

        // One hit, not three, though three gems went off around it.
        assert_eq!(broke.len(), 1, "a brick takes one hit per clear");
        assert_eq!(broke[0].value, 1, "and is left cracked, not gone");
        assert_eq!(game.board.brick(Pos::new(3, 2)), 1);
    }

    #[test]
    fn a_cracked_brick_is_cleared_away_by_the_next_hit() {
        let mut game = bricked_game(161);
        game.board.damage_brick(Pos::new(3, 2));
        assert_eq!(game.board.brick(Pos::new(3, 2)), 1, "starting from cracked");

        for c in 0..3 {
            game.board.set_gem(Pos::new(2, c), Some(Gem::plain(1)));
        }
        let resolution = game.plan_resolution(None).expect("the row should match");
        game.begin_clear(resolution);

        assert_eq!(game.board.brick(Pos::new(3, 2)), 0, "the second hit takes it");
        let broke: Vec<&Event> = game.events().iter().filter(|e| e.kind == EV_BRICK).collect();
        assert_eq!(broke[0].value, 0, "and it says so");
    }

    #[test]
    fn a_brick_nobody_clears_beside_is_left_alone() {
        let mut game = bricked_game(162);
        // The match is two rows up, with nothing of it touching the brick.
        for c in 0..3 {
            game.board.set_gem(Pos::new(1, c), Some(Gem::plain(1)));
        }
        let resolution = game.plan_resolution(None).expect("the row should match");
        game.begin_clear(resolution);

        assert_eq!(game.board.brick(Pos::new(3, 2)), 2, "nothing went off beside it");
        assert!(game.events().iter().all(|e| e.kind != EV_BRICK));
    }

    #[test]
    fn a_beam_goes_through_a_brick_rather_than_stopping_at_it() {
        let mut game = bricked_game(163);
        // A row clearer on the brick's own row, with the brick between it and
        // the far side. Nothing is adjacent to the brick but the beam.
        game.board.set_gem(Pos::new(3, 0), Some(Gem { color: 1, special: Special::LineH }));
        game.board.set_gem(Pos::new(3, 4), Some(Gem::plain(2)));

        let blast = matching::detonate(
            &game.board,
            &[Pos::new(3, 0)],
            &[],
            &mut game.rng,
            0.0,
        );
        assert!(blast.struck.contains(&Pos::new(3, 2)), "the beam should have marked the brick");
        assert!(
            blast.cleared.contains(&Pos::new(3, 4)),
            "and carried on past it to the far side",
        );
    }

    #[test]
    fn a_brick_cannot_be_swapped_with() {
        let mut game = bricked_game(164);
        for p in game.board.positions().collect::<Vec<_>>() {
            if game.board.brick(p) == 0 {
                game.board.set_gem(p, Some(Gem::plain((p.r + p.c) as u8 % 4)));
            }
        }
        game.phase = Phase::Idle;

        // Its neighbor is an ordinary gem, so the refusal is about the brick.
        assert!(game.board.gem(Pos::new(3, 1)).is_some());
        assert!(!game.try_swap(Pos::new(3, 1), Pos::new(3, 2)), "a brick does not move");
        assert_eq!(game.board.brick(Pos::new(3, 2)), 2, "and is still there");
    }

    #[test]
    fn a_brick_holds_its_cell_against_gravity() {
        let mut game = bricked_game(165);
        game.board.set_gem(Pos::new(0, 2), Some(Gem::plain(1)));
        game.origin = game.settled_origin();

        while game.board.settle_stage(&game.spec.rules, &mut game.rng).is_some() {}

        assert_eq!(game.board.brick(Pos::new(3, 2)), 2, "the brick stayed put");
        assert!(
            game.board.gem(Pos::new(3, 2)).is_none(),
            "and nothing fell into the cell it is holding",
        );
        // The gem came to rest on top of it rather than passing through.
        assert_eq!(game.board.gem(Pos::new(2, 2)).map(|g| g.color), Some(1));
    }

    #[test]
    fn a_spill_is_its_own_fall_with_its_own_landing() {
        // The board: a wall at (1,1) with a gap under it, and a gem perched at
        // (1,0) on top of another. Nothing can drop, so the only thing left to
        // do is slide, and that slide is a stage of its own rather than part of
        // the fall that came before it.
        let mut game = Game::new(spec(3, 3, 6, 10), 150);
        game.spec.rules.refill = RefillMode::None;
        game.board = Board::from_layout(&["...", ".#.", "..."]);
        game.board.set_gem(Pos::new(1, 0), Some(Gem::plain(1)));
        game.board.set_gem(Pos::new(2, 0), Some(Gem::plain(2)));
        game.origin = game.settled_origin();
        game.events.clear();

        assert!(game.begin_settle_stage(), "the perched gem should have somewhere to go");
        assert!(matches!(game.phase(), Phase::Falling { .. }), "and it should animate getting there");
        let landings: Vec<&Event> = game.events().iter().filter(|e| e.kind == EV_LAND).collect();
        assert_eq!(landings.len(), 1, "a slide lands, so it thuds like any other landing");
        assert_eq!((landings[0].r, landings[0].c), (2, 1), "where it came to rest");

        // And it really was a separate stage: the board it left is the board
        // the fall before it ended on.
        assert_eq!(game.board.gem(Pos::new(2, 1)).map(|g| g.color), Some(1));
        assert!(game.board.gem(Pos::new(1, 0)).is_none());
    }

    #[test]
    fn nothing_is_matched_until_the_spilling_has_finished() {
        // Three alike only line up once a gem has slid off a shelf. A board
        // checked for matches before the slide finds nothing and hands the turn
        // back mid-settle, which is the thing being ruled out.
        //
        //   . # . .        . # . .
        //   1 # . .   ->   . # . .
        //   1 . 1 3        1 1 1 3
        let mut game = Game::new(spec(3, 4, 6, 10), 151);
        game.spec.rules.refill = RefillMode::None;
        game.board = Board::from_layout(&[".#..", ".#..", "...."]);
        game.board.set_gem(Pos::new(1, 0), Some(Gem::plain(1)));
        game.board.set_gem(Pos::new(2, 0), Some(Gem::plain(1)));
        game.board.set_gem(Pos::new(2, 2), Some(Gem::plain(1)));
        game.board.set_gem(Pos::new(2, 3), Some(Gem::plain(3)));
        game.cascade = 1;

        // Nothing matches yet: the three 1s are not in a line.
        assert!(matching::find_matches(&game.board, game.rules()).is_empty());

        game.origin = game.settled_origin();
        game.phase = Phase::Falling { elapsed: 0.0 };
        game.fall_ms = 1.0;
        let events = settle(&mut game);

        assert!(
            events.iter().any(|e| e.kind == EV_MATCH),
            "the slide should have completed a row and set it off",
        );
    }

    #[test]
    fn a_fall_that_ends_the_chain_hands_the_board_straight_back() {
        // The beat is for separating a landing from the clear it causes. With
        // no clear coming it would only be input the player cannot give yet.
        let mut game = Game::new(spec(6, 6, 6, 10), 141);
        paint(&mut game, &latin_board());
        game.origin = game.settled_origin();
        game.origin[1] = (-1.0, 1.0);
        game.events.clear();
        game.begin_fall();

        let landing = game
            .events()
            .iter()
            .find(|e| e.kind == EV_LAND)
            .map(|e| e.value as f32)
            .expect("the painted board should have something to drop");
        assert!(
            (game.fall_ms - landing).abs() <= 1.0,
            "the fall runs {}ms for a {landing}ms drop, so it is holding on for nothing",
            game.fall_ms,
        );
    }

    #[test]
    fn a_strike_and_the_gravity_that_answers_it_are_separate_events() {
        let mut game = Game::new(spec(8, 8, 6, 10), 113);
        let from = Pos::new(0, 0);
        let to = Pos::new(4, 3);
        let shot =
            Launch { from, to, flight_ms: flight_time(cells_between(from, to)), landed: false };
        let gem = game.board.gem(from).expect("the board is full");
        game.board.set_gem(from, Some(Gem { special: Special::Rocket, ..gem }));
        game.launches = vec![shot];
        game.launch_ms = shot.flight_ms + LAUNCH_HOLD_MS;
        game.phase = Phase::Launching { elapsed: 0.0 };

        // Walked in small steps, because what is being measured is the gap
        // between two moments rather than what happened by the end.
        let step = 4.0;
        let (mut now, mut struck, mut fell) = (0.0_f32, None, None);
        for _ in 0..600 {
            game.update(step);
            now += step;
            if game.events().iter().any(|e| e.kind == EV_ROCKET_HIT) {
                struck = Some(now);
            }
            if game.events().iter().any(|e| e.kind == EV_LAND) {
                fell = Some(now);
                break;
            }
        }

        let struck = struck.expect("the rocket never hit anything");
        let fell = fell.expect("the board never collapsed");
        let beat = fell - struck;
        // Asserted against a floor of its own as well as against the constant.
        // A check that only compares the gap to LAUNCH_HOLD_MS agrees just as
        // happily when that is set to zero, which is the thing being ruled out:
        // two events closer together than this read as one.
        assert!(beat > 150.0, "only {beat:.0}ms between the strike and the fall");
        // Either moment can be seen up to a step late, so allow for both.
        assert!(
            beat >= LAUNCH_HOLD_MS - step * 2.0,
            "the hold is {beat:.0}ms, short of the {LAUNCH_HOLD_MS} it is set to",
        );
    }

    #[test]
    fn a_launch_says_how_long_it_will_be_in_the_air() {
        // The front end needs this to make the whistle last the whole flight.
        let mut game = Game::new(spec(4, 4, 6, 10), 105);
        paint(&mut game, &square_board());
        assert!(game.try_swap(Pos::new(1, 1), Pos::new(1, 2)));
        let events = settle(&mut game);

        let launch = events
            .iter()
            .find(|e| e.kind == EV_SPECIAL_FIRED && e.special == Special::Rocket.code())
            .expect("a rocket should have launched");
        let flown = f32::from(launch.value);
        assert!(flown > 0.0, "a flight takes some time");
        assert!(
            (flown - flight_time(1.0)).abs() < 1.0 || flown >= flight_time(1.0),
            "it should be at least as long as the shortest hop, got {flown}ms"
        );
        assert!(flown < 4_000.0, "and not absurd, got {flown}ms");
    }

    #[test]
    fn a_rocket_impact_announces_itself() {
        let mut game = Game::new(spec(4, 4, 6, 10), 103);
        paint(&mut game, &square_board());
        assert!(game.try_swap(Pos::new(1, 1), Pos::new(1, 2)));
        let events = settle(&mut game);

        let hits: Vec<&Event> = events.iter().filter(|e| e.kind == EV_ROCKET_HIT).collect();
        assert_eq!(hits.len(), 1, "one rocket, one strike");
        assert_eq!(hits[0].special, Special::Rocket.code());
        assert!(
            events.iter().any(|e| e.kind == EV_CLEAR && (e.r, e.c) == (hits[0].r, hits[0].c)),
            "the struck cell should also clear normally"
        );
    }

    #[test]
    fn two_line_gems_facing_the_same_way_clear_a_row_and_a_column() {
        // Both face across, so clearing "as normal" would take the same row
        // twice. The gem the player moved turns instead.
        let mut game = Game::new(spec(6, 6, 6, 91), 91);
        paint(&mut game, &latin_board());
        game.board.set_gem(Pos::new(2, 2), Some(Gem { color: 4, special: Special::LineH }));
        game.board.set_gem(Pos::new(2, 3), Some(Gem { color: 5, special: Special::LineH }));

        // The player picks up (2,3) and drags it onto (2,2).
        assert!(game.try_swap(Pos::new(2, 3), Pos::new(2, 2)));
        let cleared = first_clear(&mut game);
        let cells: Vec<(u8, u8)> = cleared.iter().map(|e| (e.r, e.c)).collect();

        for c in 0..6u8 {
            assert!(cells.contains(&(2, c)), "row 2 should have gone, missing column {c}");
        }
        for r in 0..6u8 {
            assert!(cells.contains(&(r, 2)), "column 2 should have gone, missing row {r}");
        }
        assert_eq!(cleared.len(), 11, "a row and a column, sharing one cell");
    }

    #[test]
    fn two_crosses_both_fire_as_they_are() {
        let mut game = Game::new(spec(6, 6, 6, 92), 92);
        paint(&mut game, &latin_board());
        game.board.set_gem(Pos::new(2, 2), Some(Gem { color: 4, special: Special::Cross }));
        game.board.set_gem(Pos::new(2, 3), Some(Gem { color: 5, special: Special::Cross }));

        assert!(game.try_swap(Pos::new(2, 3), Pos::new(2, 2)));
        let cleared = first_clear(&mut game);
        // Row 2 twice over, plus both columns.
        assert_eq!(cleared.len(), 16, "one row and two columns");
    }

    #[test]
    fn a_line_gem_and_a_cross_each_do_their_own_thing() {
        let mut game = Game::new(spec(6, 6, 6, 93), 93);
        paint(&mut game, &latin_board());
        game.board.set_gem(Pos::new(2, 2), Some(Gem { color: 4, special: Special::Cross }));
        game.board.set_gem(Pos::new(2, 3), Some(Gem { color: 5, special: Special::LineV }));

        assert!(game.try_swap(Pos::new(2, 3), Pos::new(2, 2)));
        let cleared = first_clear(&mut game);
        // The cross takes row 2 and column 2; the line gem takes column 3.
        assert_eq!(cleared.len(), 16);
    }

    #[test]
    fn a_special_against_an_ordinary_gem_still_does_nothing() {
        let mut game = Game::new(spec(6, 6, 6, 94), 94);
        paint(&mut game, &latin_board());
        game.board.set_gem(Pos::new(2, 2), Some(Gem { color: 4, special: Special::Cross }));

        assert!(game.try_swap(Pos::new(2, 2), Pos::new(2, 3)));
        let _ = settle(&mut game);
        assert_eq!(game.progress.score, 0, "it should simply have slid back");
        assert_eq!(game.moves_left, game.spec.moves, "and cost nothing");
    }

    #[test]
    fn a_gem_with_further_to_fall_takes_longer() {
        let near = fall_time(1.0);
        let far = fall_time(6.0);
        assert!(far > near, "a long drop should take longer than a short one");
        // Past the acceleration, each extra row costs exactly one row at
        // terminal velocity.
        let extra = fall_time(6.0) - fall_time(5.0);
        assert!((extra - 1.0 / FALL_SPEED).abs() < 1e-3, "terminal velocity is not capped");
    }

    #[test]
    fn a_column_settling_lands_once_however_many_gems_fall() {
        let mut game = Game::new(spec(5, 5, 6, 10), 21);
        // Lifting the 2 at (3,2) into the gap completes three across at row 2.
        paint(&mut game, &["01234", "13402", "22301", "30240", "41023"]);
        assert!(matching::find_matches(&game.board, game.rules()).is_empty());

        assert!(game.try_swap(Pos::new(3, 2), Pos::new(2, 2)));
        let lands = first_landing(&mut game);

        // Three gems come down each of those columns (two survivors and a
        // newcomer), but they arrive together and are heard once.
        assert_eq!(lands.len(), 3, "three columns emptied, three landings");
        let mut cols: Vec<u8> = lands.iter().map(|e| e.c).collect();
        cols.sort();
        assert_eq!(cols, vec![0, 1, 2]);
        for land in &lands {
            assert_eq!(land.r, 2, "a column lands where its hole was");
            assert_eq!(land.value, fall_time(1.0) as u16, "one row's worth of falling");
        }
    }

    #[test]
    fn a_column_landing_in_two_waves_is_heard_twice() {
        let mut game = Game::new(spec(5, 5, 6, 10), 22);
        // Straight at begin_fall, because a column that empties in two places
        // at once is fiddly to paint: into column 2 come one gem from a row up
        // and three from two rows up, which is what two holes leave behind.
        game.origin = game.settled_origin();
        for (r, from) in [(4usize, 3.0), (3, 2.0), (2, 0.0), (1, -1.0), (0, -2.0)] {
            game.origin[r * 5 + 2] = (from, 2.0);
        }
        game.events.clear();
        game.begin_fall();

        let lands: Vec<&Event> = game.events.iter().filter(|e| e.kind == EV_LAND).collect();
        assert_eq!(lands.len(), 2, "two distances, two landings");
        let near = lands.iter().find(|e| e.r == 4).expect("the lower wave lands at row 4");
        let far = lands.iter().find(|e| e.r == 2).expect("the upper wave lands at row 2");
        assert_eq!(near.value, fall_time(1.0) as u16);
        assert_eq!(far.value, fall_time(2.0) as u16);
        assert!(far.value > near.value, "the deeper drop is heard later");
    }

    #[test]
    fn a_board_that_does_not_move_makes_no_sound() {
        let mut game = Game::new(spec(5, 5, 6, 10), 23);
        game.origin = game.settled_origin();
        game.events.clear();
        game.begin_fall();
        assert!(game.events.iter().all(|e| e.kind != EV_LAND), "nothing fell, nothing landed");
    }

    #[test]
    fn falling_gems_never_exceed_terminal_velocity() {
        let step = 16.0;
        let mut t = 0.0;
        while t < 2_000.0 {
            let speed = (fallen_cells(t + step) - fallen_cells(t)) / step;
            assert!(speed <= FALL_SPEED + 1e-6, "at {t}ms gems were falling at {speed} cells/ms");
            t += step;
        }
    }

    #[test]
    fn two_rainbows_take_the_whole_board() {
        let mut game = Game::new(spec(6, 6, 6, 10), 84);
        paint(&mut game, &latin_board());
        game.board.set_gem(Pos::new(3, 3), Some(Gem { color: 0, special: Special::Rainbow }));
        game.board.set_gem(Pos::new(3, 4), Some(Gem { color: 1, special: Special::Rainbow }));

        assert!(game.try_swap(Pos::new(3, 3), Pos::new(3, 4)));
        let events = settle(&mut game);
        let cleared = events.iter().filter(|e| e.kind == EV_CLEAR).count();
        assert_eq!(cleared, 36, "every cell on the board");
    }

    #[test]
    fn a_special_sits_still_until_a_match_of_its_color_reaches_it() {
        let mut game = Game::new(spec(4, 4, 6, 10), 75);
        paint(&mut game, &["0234", "2340", "3112", "4021"]);
        game.board.set_gem(Pos::new(2, 1), Some(Gem { color: 1, special: Special::Cross }));

        // Pushing the cross around achieves nothing on its own.
        assert!(game.try_swap(Pos::new(2, 1), Pos::new(1, 1)));
        let _ = settle(&mut game);
        assert_eq!(game.progress.score, 0, "shoving a cross is not a move");
        assert_eq!(game.moves_left, 10, "and it costs nothing");
        assert_eq!(
            game.board.gem(Pos::new(2, 1)).map(|g| g.special),
            Some(Special::Cross),
            "the cross should have slid back"
        );
    }

    #[test]
    fn a_cross_goes_off_when_a_match_sweeps_it_up() {
        let mut game = Game::new(spec(4, 4, 6, 10), 76);
        paint(&mut game, &["0234", "2340", "3112", "4021"]);
        game.board.set_gem(Pos::new(2, 1), Some(Gem { color: 1, special: Special::Cross }));
        assert!(matching::find_matches(&game.board, game.rules()).is_empty());

        // Swapping brings a third 1 into row 2, which catches the cross.
        assert!(game.try_swap(Pos::new(2, 3), Pos::new(3, 3)));
        let events = settle(&mut game);

        assert!(
            events
                .iter()
                .any(|e| e.kind == EV_SPECIAL_FIRED && e.special == Special::Cross.code()),
            "the cross should have fired"
        );
        let cleared = events.iter().filter(|e| e.kind == EV_CLEAR).count();
        assert!(cleared >= 7, "a cross takes a row and a column, saw {cleared} cells");
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
    fn a_level_that_opens_short_of_moves_says_so_straight_away() {
        // Three moves is the puzzle, not a warning about it, and the player
        // should know before spending the first one. Archipelago makes this the
        // ordinary case: moves are an item, so an early level is a handful.
        let mut game = Game::new(spec(6, 6, 6, 3), 90);
        game.update(16.0);
        let warnings: Vec<&Event> =
            game.events().iter().filter(|e| e.kind == EV_LOW_MOVES).collect();
        assert_eq!(warnings.len(), 1, "it should say so on the first frame");
        assert_eq!(warnings[0].value, 3, "and say how many there actually are");

        // Once, though, not on every frame from here on.
        for _ in 0..20 {
            game.update(16.0);
            assert!(game.events().iter().all(|e| e.kind != EV_LOW_MOVES), "it repeated itself");
        }
    }

    #[test]
    fn a_comfortable_level_warns_only_once_it_runs_down() {
        let mut game = Game::new(spec(6, 6, 6, 20), 91);
        for _ in 0..10 {
            game.update(16.0);
            assert!(game.events().iter().all(|e| e.kind != EV_LOW_MOVES), "warned far too early");
        }

        // Straight to the edge of the budget rather than playing twenty moves
        // out: what is under test is the threshold, not the route to it.
        game.moves_left = LOW_MOVES + 1;
        game.update(16.0);
        assert!(game.events().iter().all(|e| e.kind != EV_LOW_MOVES), "one above the line is fine");

        game.moves_left = LOW_MOVES;
        game.update(16.0);
        let warnings: Vec<&Event> =
            game.events().iter().filter(|e| e.kind == EV_LOW_MOVES).collect();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].value, LOW_MOVES as u16);
    }

    #[test]
    fn being_handed_more_moves_re_arms_the_warning() {
        // Which is what receiving a moves item mid-level will look like.
        let mut game = Game::new(spec(6, 6, 6, 20), 92);
        game.moves_left = 2;
        game.update(16.0);
        assert!(game.events().iter().any(|e| e.kind == EV_LOW_MOVES));

        game.moves_left = 12;
        game.update(16.0);
        assert!(game.events().iter().all(|e| e.kind != EV_LOW_MOVES), "nothing to warn about yet");

        game.moves_left = 4;
        game.update(16.0);
        let warnings: Vec<&Event> =
            game.events().iter().filter(|e| e.kind == EV_LOW_MOVES).collect();
        assert_eq!(warnings.len(), 1, "running short a second time is worth saying again");
        assert_eq!(warnings[0].value, 4);
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
                        if !game.board.is_open(p) {
                            assert!(
                                game.board.gem(p).is_none(),
                                "level {index} put a gem inside a wall at {p:?}"
                            );
                        }
                        if game.board.brick(p) > 0 {
                            assert!(
                                game.board.gem(p).is_none(),
                                "level {index} put a gem inside a brick at {p:?}"
                            );
                        }
                    }
                    // A hole is legal now: a pocket under a brick shelf can be
                    // out of everything's reach. What is not legal is a hole
                    // something could still fall into, which is what asking the
                    // board whether it would move catches, and it catches a gem
                    // left hanging in the air besides.
                    assert!(
                        !game.board.will_move(game.rules()),
                        "level {index} idled with the board still able to move"
                    );
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
