//! The playable game: input, the clock, and the resolve loop.
//!
//! The engine owns time. A front end calls [`Game::update`] with the elapsed
//! milliseconds and then reads a snapshot; every decision about when a swap
//! lands, when a cascade fires and when a level ends is made here, so the same
//! rules hold whether the front end is a browser, a test, or a headless
//! simulation.

use crate::board::{Board, Gem, Pos, Special, ANY_COLOR};
use crate::level::{LevelSpec, Objective, Progress};
use crate::matching::{self, MatchGroup};
use crate::progression::Consumable;
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
/// Top speed, in cells per millisecond: a little over four cells a second.
pub const LAUNCH_SPEED: f32 = 0.004225;
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

/// How many boards to deal looking for one that opens quietly, with no match
/// already on it. Past this an opening match is accepted: it is untidy rather
/// than broken, and a level whose placed gems make one unavoidable should
/// still be playable.
const QUIET_DEAL_TRIES: u32 = 64;

/// How many more to deal looking for one with a legal move on it, which is not
/// optional. Bounded only so that a layout nothing can be played on cannot
/// hang the page; anything approaching this many is a level design fault, and
/// `every_level_opens_with_something_to_do` is what catches it.
const PLAYABLE_DEAL_TRIES: u32 = 4_096;

/// How far apart the specials minted at the end of a level go off.
///
/// Wider than a match's own spread: this is a dozen of them at once, and
/// firing them on the same frame reads as one white flash rather than as the
/// board being taken apart.
const FINALE_JITTER_MS: f32 = 700.0;

/// One leftover move spent per this long, so the counter can be watched
/// running down rather than dropping to zero between frames. Slow enough to
/// follow, brisk enough that a forty move surplus is not a minute of waiting.
const CASH_IN_STEP_MS: f32 = 300.0;

/// A beat between the board coming to rest and the level being declared over,
/// so the last thing the player sees is the board rather than a panel sliding
/// over it.
///
/// Both endings get it. A win wants a moment to look at what was cleared, and
/// a loss wants one more than that: the panel arriving on the same frame as
/// the last gem lands reads as though the game snatched the board away, and
/// the move that ran the counter out is never seen to finish.
const END_HOLD_MS: f32 = 1_000.0;

/// Splits the hint's own stream off the deal's. Any constant would do; what
/// matters is that the two streams differ. See [`Game::hint_rng`].
const HINT_STREAM: u64 = 0x4849_4e54_5f5f_5f5f;

/// How many rockets a cluster is worth, at each end.
const CLUSTER_LEAST: u32 = 3;
const CLUSTER_MOST: u32 = 5;

/// What a leftover move can be turned into.
///
/// The three that clear a line or a cross, and not the other two. A rainbow
/// takes a color off the whole board and a rocket flies somewhere else, so a
/// boardful of either is a wall of noise rather than a board coming apart in
/// front of you.
const CASH_IN_SPECIALS: [Special; 3] = [Special::LineH, Special::LineV, Special::Cross];

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
///
/// A rocket is the exception in both directions: it can be aimed at a brick,
/// which nothing else can, and its strike does nothing to the bricks around
/// what it hits. It takes the one cell it was pointed at, and that is all.
pub const EV_BRICK: u8 = 14;

/// An item reached the run.
///
/// Not raised by the board, which knows nothing about items: the session
/// raises it, and it rides the same stream so the page has one place to watch
/// for things worth telling the player about.
///
/// `value` is the item's place in the game's item table, and `color` and
/// `special` are the low and high bytes of the location's place in its own
/// table, or 65535 for an item that came from no location here, which is what
/// a multiworld handing one over looks like.
///
/// Indices rather than names because the stream carries no text, and indices
/// rather than a kind and a parameter because the names have to be the same
/// strings a tracker shows: the page reads them out of the engine's tables
/// instead of building its own and hoping they match.
pub const EV_ITEM: u8 = 15;

/// The level is won. `value` is how many moves were left over, which is what
/// the flourish that follows has to spend.
///
/// Raised once, the moment the goals are met, rather than again for every
/// round of the flourish: what it announces is the level being beaten, and
/// that happens once.
pub const EV_CLEARED: u8 = 16;

/// One leftover move was spent on this cell. `special` is what it left there,
/// or 0 when it left nothing.
///
/// Raised whether or not anything was placed, because the spending is the
/// event: a run holding none of the eligible unlocks still goes through the
/// motions, and a cell that flashes and stays a plain gem says that far better
/// than silence does. Distinct from [`EV_SPECIAL_MADE`], which is a match
/// earning its reward.
pub const EV_CASH_IN: u8 = 17;

/// An Archipelago gem was collected.
///
/// Separate from [`EV_CLEAR`], which says a gem left the board: one of these
/// can leave the board through a beam's path and raise both, and the two mean
/// different things. This one is the check being taken, which is what the
/// session turns into an item and what a multiworld is told about.
pub const EV_AP_CLEAR: u8 = 18;

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

    /// An event about the run rather than the board. See [`EV_ITEM`].
    pub fn about_item(kind: u8, from: u16, item: u16) -> Self {
        Event {
            kind,
            r: 255,
            c: 255,
            color: from as u8,
            special: (from >> 8) as u8,
            cascade: 0,
            value: item,
        }
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
    /// The goals are met and the board is holding still while the front end
    /// finishes saying so.
    ///
    /// A goal does not finish the moment its last gem goes: what the clear
    /// counted takes a moment to reach the goal on screen, and the count
    /// falling is the payoff. The flourish waits for that rather than starting
    /// over the top of it. How long is [`Game::goal_hold_ms`], which is the
    /// page's number because the page owns the animation.
    Tallying { elapsed: f32 },
    /// The goals are met and the moves left over are being spent, one at a
    /// time, so the counter can be watched running down.
    CashingIn { elapsed: f32 },
    /// The board has come to rest and the level is over, one way or the other.
    /// A beat to look at it before the result is declared and a panel covers
    /// it. Which way it went is settled by then; see [`Game::declare_over`].
    Finishing { elapsed: f32 },
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
            Phase::CashingIn { .. } => 7,
            Phase::Finishing { .. } => 8,
            Phase::Tallying { .. } => 9,
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
    /// Spent out of the inventory rather than fired off the board.
    ///
    /// Its `from` is a point below the bottom row, which is where the bar the
    /// inventory sits in is, so it flies up into the board. There is no gem
    /// there to take away when it goes, and no cell to hang an offset on;
    /// both of those are what this says.
    from_inventory: bool,
}

/// A clear that is about to happen: what sets it off, and what it leaves behind.
struct Resolution {
    seeds: Vec<Pos>,
    /// How many gems the matches themselves hold, which is the size of the
    /// match rather than the size of what it sets off.
    ///
    /// Only what lined up: a group is made of gems that share a color, so a
    /// plain gem counts and so does one carrying a beam, while a rocket, a
    /// rainbow and an Archipelago gem answer to no color and are never in one.
    /// Bricks and seals hold no gem at all; they are broken beside a match,
    /// not part of it.
    matched: u32,
    creations: Vec<(Pos, Gem)>,
    /// Specials the swap has already cashed in; see [`Activation::spent`].
    spent: Vec<Pos>,
    /// Specials to set off at cells that are not carrying one.
    ///
    /// What spending a Cross Clear out of the inventory is: the beam is real
    /// and does everything a beam does, but no gem on the board is holding it.
    /// Empty for everything the board sets off itself.
    fires: Vec<(Pos, Special)>,
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
    /// A stream of its own, for choosing which legal move to offer.
    ///
    /// Separate from `rng` on purpose. A hint is the one thing the player asks
    /// for that changes nothing about the board, and drawing from the deal to
    /// answer it would make every gem that falls afterwards depend on how
    /// often they asked. A run would stop being the same run.
    hint_rng: Rng,
    /// The move this board has already been offering, if it has been asked.
    ///
    /// Kept so that asking twice about the same board gets the same answer;
    /// see [`Game::hint`]. Not cleared when a move is made, because it does
    /// not need to be: a move that is no longer on the board is no longer a
    /// useful swap, and that is what retires it.
    hinted: Option<(Pos, Pos)>,
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
    /// Rockets in flight. Each keeps its own flight time and lands on it.
    launches: Vec<Launch>,
    /// Specials a strike landed on, waiting for the last rocket to be down
    /// before they all go off together.
    triggered: Vec<Pos>,
    /// Per-cell row and column a gem started falling from, which is not always
    /// its own column: see [`Board::collapse`].
    origin: Vec<(f32, f32)>,
    cascade: u32,
    swap: Option<(Pos, Pos)>,
    selected: Option<Pos>,
    /// Whether the short-on-moves warning has already gone out for this stretch
    /// of the level; see [`EV_LOW_MOVES`].
    warned_low_moves: bool,
    /// Whether the level has already said it was cleared; see [`EV_CLEARED`].
    /// The board settles several times during the flourish that follows, and
    /// only the first of those is news.
    announced_clear: bool,
    /// How many gems the player's last swap matched, or 0 for a swap that
    /// matched nothing.
    ///
    /// The player's own match and nothing else: what the clear went on to set
    /// off is not part of it, and neither is anything a cascade lined up
    /// afterwards, because the point of the number is what the player did.
    /// Set on every swap, so it is always the last one's.
    swap_match: u32,
    /// Whether the beat for the goals to finish showing themselves met has
    /// been taken; see [`Phase::Tallying`]. Once a level, even though the
    /// board settles several times during the flourish that follows.
    tallied: bool,
    /// How long that beat is, in milliseconds, or 0 for none.
    ///
    /// The page's number rather than the engine's, because the page owns the
    /// animation it is waiting for: an engine constant here would be a second
    /// copy of a duration only the front end knows, and it would go stale the
    /// first time that animation was retimed. Zero for a board opened without
    /// a page in front of it, which is every test.
    pub goal_hold_ms: f32,
    /// How many Archipelago gems this level still has checks waiting in, which
    /// is what caps how many the refill lets in.
    ///
    /// Set by whoever opened the level, because the board has no idea what a
    /// location is. Zero on a board opened without a run behind it, which is
    /// every board in these tests and the reason none of them sees one.
    pub ap_gems_wanted: u32,
    events: Vec<Event>,
    cells_buf: Vec<u8>,
    offs_buf: Vec<f32>,
    /// Where each rocket spent out of the inventory is: a column, a row, and
    /// whether it is still in the air.
    ///
    /// A rocket fired off the board needs none of this. It is a gem in a cell,
    /// and its cell's offset carries it wherever it goes. One spent out of the
    /// inventory comes up from under the bottom row, where there is no cell,
    /// so its position has nowhere else to be written down.
    flights_buf: Vec<f32>,
}

impl Game {
    pub fn new(spec: LevelSpec, seed: u64) -> Self {
        let mut game = Game {
            board: Board::new(spec.rules.rows, spec.rules.cols),
            progress: Progress::default(),
            moves_left: spec.moves,
            seed,
            rng: Rng::new(seed),
            // Off the same seed, so a run is still the same run, but not off
            // the same stream.
            hint_rng: Rng::new(seed ^ HINT_STREAM),
            hinted: None,
            phase: Phase::Idle,
            status: Status::Playing,
            clearing: Vec::new(),
            clear_ms: POP_MS,
            launch_ms: LAUNCH_RAMP_MS,
            fall_ms: FALL_ACCEL_MS,
            launches: Vec::new(),
            triggered: Vec::new(),
            origin: Vec::new(),
            cascade: 0,
            swap: None,
            selected: None,
            warned_low_moves: false,
            announced_clear: false,
            swap_match: 0,
            tallied: false,
            goal_hold_ms: 0.0,
            ap_gems_wanted: 0,
            events: Vec::new(),
            cells_buf: Vec::new(),
            offs_buf: Vec::new(),
            flights_buf: Vec::new(),
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
        self.hint_rng = Rng::new(self.seed ^ HINT_STREAM);
        self.hinted = None;
        self.progress = Progress::default();
        self.progress.jelly_total = self.board.jelly_cells();
        self.progress.jelly_left = self.progress.jelly_total;
        self.progress.brick_total = self.board.brick_cells();
        self.progress.brick_left = self.progress.brick_total;
        self.progress.seals_at_start = self.board.seal_cells();
        self.progress.seals_now = self.progress.seals_at_start;
        self.moves_left = self.spec.moves;
        self.phase = Phase::Idle;
        self.status = Status::Playing;
        self.clearing.clear();
        self.clear_ms = POP_MS;
        self.launch_ms = LAUNCH_RAMP_MS;
        self.fall_ms = FALL_ACCEL_MS;
        self.launches.clear();
        self.triggered.clear();
        self.cascade = 0;
        self.swap = None;
        self.selected = None;
        self.warned_low_moves = false;
        self.announced_clear = false;
        self.swap_match = 0;
        self.tallied = false;
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

    /// Whether the level's goals have been met, which is the moment it is
    /// cleared rather than the moment it is over.
    ///
    /// The flourish runs between the two, and it is still play: the score is
    /// still climbing, so a score mark can be crossed in there, and what
    /// crossing it pays can still change what the rest of the flourish does.
    /// Anything asking "has this level been beaten" wants this rather than
    /// [`Status::Won`], which does not arrive until the board has stopped.
    pub fn cleared(&self) -> bool {
        self.announced_clear
    }

    pub fn cascade(&self) -> u32 {
        self.cascade
    }

    /// How many gems the player's last swap matched. See [`Game::swap_match`].
    pub fn swap_match(&self) -> u32 {
        self.swap_match
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

    /// Spends something out of the inventory, reporting whether it happened.
    ///
    /// Refused rather than wasted when it cannot do anything: the board has to
    /// be the player's to touch, and an aimed one has to be aimed at something
    /// it can work on. The caller only takes the item out of the inventory
    /// when this says yes, so a refused tap costs nothing.
    ///
    /// None of these costs a move. They are a bonus rather than a turn, and a
    /// rocket that cost a move would be worth less than the move.
    pub fn use_consumable(&mut self, kind: Consumable, target: Option<Pos>) -> bool {
        if !self.accepts_input() {
            return false;
        }
        // A player action, so it opens a chain of its own the way a swap does.
        // Not a match, though: nothing was lined up, so it pays no match
        // location. See [`Game::swap_match`].
        match kind {
            Consumable::Rocket => {
                let Some(at) = target.filter(|p| self.board.is_open(*p)) else { return false };
                self.cascade = 1;
                self.swap_match = 0;
                self.fly(vec![(self.below_the_board(at.c), at)], true)
            }
            Consumable::RocketCluster => {
                let count = CLUSTER_LEAST + self.rng.below(CLUSTER_MOST - CLUSTER_LEAST + 1);
                // Each picks its own target the way any rocket does, so they
                // spread over what the level actually wants rather than
                // landing in a heap.
                let sources: Vec<Pos> =
                    (0..count).map(|i| self.below_the_board(i as i32 % self.board.cols)).collect();
                let pairs = self.pick_targets(&sources);
                self.cascade = 1;
                self.swap_match = 0;
                self.fly(pairs, true)
            }
            Consumable::Rainbow => {
                let Some(at) = target else { return false };
                let seeds = self.rainbow_sweep(at);
                if seeds.is_empty() {
                    return false;
                }
                self.cascade = 1;
                self.swap_match = 0;
                self.begin_clear(Resolution {
                    seeds,
                    // A rainbow's cells are the color it took, which are gems
                    // going away because the player spent it on them, so they
                    // are seeds and nothing here is fired.
                    fires: Vec::new(),
                    matched: 0,
                    creations: Vec::new(),
                    spent: Vec::new(),
                    jitter_ms: matching::RAINBOW_SPREAD_MS,
                });
                true
            }
            Consumable::CrossClear => {
                let Some(at) = target.filter(|p| self.board.is_open(*p)) else { return false };
                // Set off as a cross rather than handed over as the cells a
                // cross covers. The shape is the same either way; what is not
                // is everything else a beam does, which is worked out where a
                // special fires and nowhere else: the bricks it goes through,
                // the gems it runs over without cracking what they stand
                // beside, and the sweep outward from the middle.
                let mut reach = Vec::new();
                matching::blast(&self.board, at, Special::Cross, ANY_COLOR, &mut reach);
                // Refused rather than wasted when the row and the column hold
                // nothing at all: walls, and empty cells nothing has fallen
                // into yet.
                if reach
                    .iter()
                    .all(|p| self.board.gem(*p).is_none() && self.board.brick(*p) == 0)
                {
                    return false;
                }
                self.cascade = 1;
                self.swap_match = 0;
                self.begin_clear(Resolution {
                    seeds: Vec::new(),
                    fires: vec![(at, Special::Cross)],
                    matched: 0,
                    creations: Vec::new(),
                    spent: Vec::new(),
                    jitter_ms: 0.0,
                });
                true
            }
        }
    }

    /// What a rainbow spent on this cell takes with it, or nothing when there
    /// is nothing there it can answer to.
    ///
    /// A color it can name, and every gem wearing it. Against another rainbow
    /// it takes the board, the same as swapping two together. Against
    /// anything with no color of its own the tap is refused rather than spent:
    /// an Archipelago gem answers to nothing, and spending a rainbow to clear
    /// one cell would be the worst trade in the game.
    fn rainbow_sweep(&self, at: Pos) -> Vec<Pos> {
        let Some(gem) = self.board.gem(at) else { return Vec::new() };
        if gem.special == Special::Rainbow {
            return self.board.occupied();
        }
        let Some(color) = self.board.match_color(at) else { return Vec::new() };
        self.board.positions().filter(|p| self.board.match_color(*p) == Some(color)).collect()
    }

    /// A point below the bottom row, which is where the bar the inventory sits
    /// in is. Nothing is ever there; it is where a spent rocket flies in from.
    fn below_the_board(&self, column: i32) -> Pos {
        Pos::new(self.board.rows, column.clamp(0, self.board.cols - 1))
    }

    /// A legal move to nudge the player with, chosen from all of them.
    ///
    /// The whole board rather than the first move found. Walking the board in
    /// order and stopping at the first hit always points at the top left,
    /// which is both a tell and a poor suggestion: the move nearest the top
    /// of the board is rarely the interesting one, and a player who leans on
    /// hints is walked through the level in reading order.
    ///
    /// **One answer per board.** Having chosen, it keeps saying the same thing
    /// for as long as that move is still there. The nudge goes away when the
    /// player touches anything and comes back when they stop, so without this
    /// a player could tap twice and be dealt another suggestion, and then
    /// another, until they liked one: a reroll for free and a way to be shown
    /// every move on the board without making any of them.
    ///
    /// Takes the game rather than borrowing it, because choosing needs a
    /// draw. It comes from a stream of the hint's own so that asking changes
    /// nothing else about the run; see [`Game::hint_rng`].
    pub fn hint(&mut self) -> Option<(Pos, Pos)> {
        // Still there, so still the answer. This is also what retires it: a
        // move that has been played, or that the board has fallen away from,
        // stops being a useful swap and the next ask draws afresh.
        if let Some((a, b)) = self.hinted {
            if matching::is_useful_swap(&self.board, &self.spec.rules, a, b) {
                return Some((a, b));
            }
        }
        let moves = matching::legal_moves(&self.board, &self.spec.rules);
        let at = self.hint_rng.below(moves.len() as u32) as usize;
        self.hinted = moves.get(at).copied();
        self.hinted
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
                Phase::CashingIn { elapsed } => (CASH_IN_STEP_MS, elapsed),
                Phase::Finishing { elapsed } => (END_HOLD_MS, elapsed),
                Phase::Tallying { elapsed } => (self.goal_hold_ms, elapsed),
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
            Phase::CashingIn { .. } => Phase::CashingIn { elapsed },
            Phase::Finishing { .. } => Phase::Finishing { elapsed },
            Phase::Tallying { .. } => Phase::Tallying { elapsed },
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
            Phase::CashingIn { .. } => self.finish_cash_in_step(),
            Phase::Finishing { .. } => self.declare_over(),
            // The goals have finished showing themselves met, so the decision
            // that was held back is taken now. `tallied` is already set, so
            // this reaches the flourish rather than holding again.
            Phase::Tallying { .. } => self.settle(),
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
                // The player's own match, before anything it sets off. See
                // [`Game::swap_match`].
                self.swap_match = resolution.matched;
                self.begin_clear(resolution);
            }
            None if self.spec.rules.revert_invalid => {
                // Nothing came of it: put the gems back and slide them home.
                self.swap_match = 0;
                self.board.swap_gems(a, b);
                self.phase = Phase::Swapping { elapsed: 0.0, reverting: true };
                self.events.push(Event::at(EV_REVERT, a, 255, Special::None, 0));
            }
            None => {
                self.swap_match = 0;
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
        let pairs = self.pick_targets(rockets);
        self.fly(pairs, false)
    }

    /// Puts a set of rockets in the air and runs the phase until they are all
    /// down. `from_inventory` says they were spent rather than fired, which
    /// changes what there is to clean up behind them.
    fn fly(&mut self, pairs: Vec<(Pos, Pos)>, from_inventory: bool) -> bool {
        self.launches = pairs
            .into_iter()
            .map(|(from, to)| Launch {
                from,
                to,
                flight_ms: flight_time(cells_between(from, to)),
                landed: false,
                from_inventory,
            })
            .collect();
        if self.launches.is_empty() {
            // Nowhere worth aiming: drop them rather than stall the board.
            // Nothing to drop when they came out of the inventory, which is
            // also why one is refused before it is spent rather than here.
            if !from_inventory {
                for p in self.rockets_on_board() {
                    self.board.set_gem(p, None);
                }
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
            if !launch.from_inventory {
                self.board.set_gem(launch.from, None);
            }

            // A brick target takes the hit itself rather than being cleared.
            // The strike still lands as a strike, so it booms and throws
            // debris; the color is 255 because there is no gem behind it,
            // which is how the front end knows to throw brick and not gem.
            let sealed = self.board.brick_color(launch.to);
            if let Some(left) = self.board.damage_brick(launch.to) {
                let mut broke = Event::at(EV_BRICK, launch.to, sealed, Special::None, cascade);
                broke.value = left as u16;
                self.events.push(broke);
                self.events.push(Event::at(
                    EV_ROCKET_HIT,
                    launch.to,
                    255,
                    Special::Rocket,
                    cascade,
                ));
                self.progress.score += (SCORE_PER_GEM + SCORE_PER_SPECIAL_FIRED) * cascade as u64;
                self.progress.brick_left = self.board.brick_cells();
                self.progress.seals_now = self.board.seal_cells();
                continue;
            }

            let gem = match self.board.gem(launch.to) {
                Some(gem) => gem,
                None => continue,
            };

            // A special is set off rather than taken, and so, for its own
            // reasons, is an Archipelago gem. Both stay where they are for now
            // and go on a list; once every rocket is down, the lot of them go
            // through an ordinary clear together. Taking a special instead
            // would waste the best thing a rocket can land on.
            //
            // Taking a check instead would be worse than waste. Collecting one
            // happens in the clear, which is the only place that raises it and
            // the only place that tells the level it has one fewer to drop; a
            // rocket lifting the gem off the board here took the check away
            // with it and nothing anywhere said so.
            //
            // So the test is "not a plain gem" rather than `is_special`, which
            // an Archipelago gem is deliberately not: it is a check sitting on
            // the board, not a charge waiting to go off.
            if gem.special != Special::None {
                self.triggered.push(launch.to);
                self.events.push(Event::at(
                    EV_ROCKET_HIT,
                    launch.to,
                    gem.color,
                    Special::Rocket,
                    cascade,
                ));
                self.progress.score += SCORE_PER_SPECIAL_FIRED * cascade as u64;
                continue;
            }

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
        self.progress.jelly_left = self.board.jelly_cells();
    }

    /// The last rocket is down and its beat has passed, so the board may
    /// finally settle.
    fn finish_launch(&mut self) {
        self.land_arrivals(f32::INFINITY);
        self.launches.clear();

        // Specials the strikes landed on go off now, all together, as a clear
        // like any other. It has to wait until here: firing one while another
        // rocket was still in the air would collapse the board under it.
        if !self.triggered.is_empty() {
            let seeds = std::mem::take(&mut self.triggered);
            self.begin_clear(Resolution {
                seeds,
                // Struck rather than matched, so no shape the player lined up.
                matched: 0,
                creations: Vec::new(),
                spent: Vec::new(),
                // Whatever a rocket landed on is carrying its own special, so
                // the wave finds it where it stands.
                fires: Vec::new(),
                jitter_ms: 0.0,
            });
            return;
        }

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
    /// A brick is reached two ways: a beam went through it, or a gem the
    /// player actually matched went away in one of the four cells around it.
    /// A rainbow counts as matched, wherever on the board its color happened to
    /// be. A gem a beam merely ran over does not: the beam's business with
    /// bricks is the ones it passes through.
    ///
    /// Either way the brick takes exactly one hit per clear, so the same brick
    /// beside three gems of one match is cracked rather than demolished.
    fn strike_bricks(&mut self, blast: &matching::Detonation, cascade: u32) {
        // A beam counts against whatever it goes through, seal or brick alike:
        // being shot is not a question of color.
        let mut hit = blast.struck.clone();
        for (p, _) in blast.cleared.iter().zip(&blast.cracks).filter(|(_, hits)| **hits) {
            // Still on the board at this point: gems are not taken off it until
            // the pop animation finishes, so their colors are readable here.
            let Some(color) = self.board.color(*p) else { continue };
            for side in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                let neighbor = Pos::new(p.r + side.0, p.c + side.1);
                if self.board.brick(neighbor) > 0
                    && self.board.answers_to(neighbor, color)
                    && !hit.contains(&neighbor)
                {
                    hit.push(neighbor);
                }
            }
        }

        for p in hit {
            let color = self.board.brick_color(p);
            if let Some(left) = self.board.damage_brick(p) {
                let mut event = Event::at(EV_BRICK, p, color, Special::None, cascade);
                event.value = left as u16;
                self.events.push(event);
            }
        }
        self.progress.brick_left = self.board.brick_cells();
        self.progress.seals_now = self.board.seal_cells();
    }

    /// Every Archipelago gem this clear reached by going off beside it.
    ///
    /// The same reach a brick answers to, and for the same reason: an
    /// Archipelago gem is collected by clearing against it rather than by
    /// matching it. A beam that runs straight through one needs nothing here,
    /// because the gem is in the beam's path and is cleared with everything
    /// else in the row.
    ///
    /// Unlike a brick it takes no damage, only collection: one hit and it is
    /// gone, since there is nothing to wear down.
    fn ap_gems_struck(&self, blast: &matching::Detonation) -> Vec<Pos> {
        let mut struck: Vec<Pos> = Vec::new();
        for (p, _) in blast.cleared.iter().zip(&blast.cracks).filter(|(_, hits)| **hits) {
            for side in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                let neighbor = Pos::new(p.r + side.0, p.c + side.1);
                if self.board.is_ap_gem(neighbor)
                    && !struck.contains(&neighbor)
                    // Already going off in this same clear, so collecting it
                    // twice would announce one check as two.
                    && !blast.cleared.contains(&neighbor)
                {
                    struck.push(neighbor);
                }
            }
        }
        struck
    }

    /// Runs the next stage of a settle and puts the board into the fall that
    /// animates it, reporting whether there was a stage to run.
    fn begin_settle_stage(&mut self) -> bool {
        match self.board.settle_stage(&self.spec.rules, &mut self.rng, self.ap_gems_wanted) {
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

    /// Picks what each rocket flies at, best use first.
    ///
    /// Three tiers, and a rocket takes the best one on offer:
    ///
    /// 1. Anything that moves an objective along: a gem of a color still being
    ///    counted, a gem sitting on jelly, or a brick. Within the tier the
    ///    choice is even, so a rocket does not always go for the same kind of
    ///    progress.
    /// 2. Any ordinary gem.
    /// 3. A special, which the strike sets off rather than simply taking. That
    ///    is the last resort by weight but the best thing that can happen, so
    ///    it is worth the board being down to it.
    ///
    /// Score is not an objective for this purpose. Everything on the board
    /// advances a score target, so counting it would put every cell in the
    /// first tier and the tiers would mean nothing.
    ///
    /// Rockets are never aimed at each other: one waiting to launch is about to
    /// do this itself. No two rockets pick the same target.
    fn pick_targets(&mut self, rockets: &[Pos]) -> Vec<(Pos, Pos)> {
        let mut best: Vec<Pos> = Vec::new();
        let mut plain: Vec<Pos> = Vec::new();
        let mut specials: Vec<Pos> = Vec::new();

        let wants_jelly = self
            .spec
            .objectives
            .iter()
            .any(|o| matches!(o, Objective::Jelly) && !o.is_met(&self.progress));
        let wanted_colors: Vec<u8> = self
            .spec
            .objectives
            .iter()
            .filter(|o| !o.is_met(&self.progress))
            .filter_map(|o| match o {
                Objective::Color { color, .. } => Some(*color),
                _ => None,
            })
            .collect();

        for p in self.board.positions() {
            if self.board.brick(p) > 0 {
                best.push(p);
                continue;
            }
            // An empty cell is nothing to aim at. That matters most for jelly:
            // it is peeled by clearing the gem standing on it, so bare jelly
            // has nothing to clear, and orphaned jelly in a pocket nothing can
            // refill would otherwise swallow every rocket for the rest of the
            // level.
            let Some(gem) = self.board.gem(p) else { continue };
            if gem.special == Special::Rocket {
                continue;
            }
            // A check is worth as much as a brick, so it sits in the same tier:
            // the top one, beside the bricks and the cells that move a goal
            // along. Left to fall through as an ordinary gem it read as one of
            // the plain ones, which meant a rocket took it by accident and only
            // once everything that advanced an objective was gone.
            if gem.special == Special::Archipelago {
                best.push(p);
                continue;
            }
            if (wants_jelly && self.board.jelly(p) > 0) || wanted_colors.contains(&gem.color) {
                best.push(p);
            } else if gem.special.is_special() {
                specials.push(p);
            } else {
                plain.push(p);
            }
        }

        let mut launches = Vec::new();
        for from in rockets {
            // Re-chosen per rocket, because an earlier one may have taken the
            // last of a tier.
            let pool = if !best.is_empty() {
                &mut best
            } else if !plain.is_empty() {
                &mut plain
            } else if !specials.is_empty() {
                &mut specials
            } else {
                break;
            };
            let pick = self.rng.below(pool.len() as u32) as usize;
            launches.push((*from, pool.swap_remove(pick)));
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

        let mut matched = 0;
        for group in &groups {
            let special = group.award(&self.spec.rules.specials);
            if special != Special::None {
                let pivot = self.pivot_for(group, swap);
                if !creations.iter().any(|(p, _)| *p == pivot) {
                    creations.push((pivot, Gem { color: group.color, special }));
                }
            }
            // Cells rather than groups: one swap can line up two runs at
            // once, and both are the player's. Summing them is safe because a
            // group is already everything that touches it, so an L is one
            // group of five rather than two threes sharing a corner, and no
            // two groups hold the same cell. See `matching::find_matches`.
            matched += group.cells.len() as u32;
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
            // Nothing fired: a match is gems the player lined up, and any
            // special among them is on the board for the wave to find.
            Some(Resolution { seeds, matched, creations, spent, fires: Vec::new(), jitter_ms })
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

        // Before the rainbow's own cases, because a rainbow swapped against an
        // Archipelago gem must not go looking for that gem's color: it has
        // none, and the sweep below would come back empty.
        //
        // Only against a rainbow or another of its own kind. An Archipelago
        // gem swapped with an ordinary one is an ordinary move that happens to
        // shift a check around: what comes of it is whatever match the gem it
        // traded places with lands in, and nothing here.
        let takes_them_all = matches!(
            (ga.special, gb.special),
            (Special::Archipelago, Special::Archipelago)
                | (Special::Archipelago, Special::Rainbow)
                | (Special::Rainbow, Special::Archipelago)
        );
        if takes_them_all {
            let mut seeds = self.board.ap_gems();
            let mut spent = Vec::new();
            // The rainbow is spent on them and goes off with them. Two
            // Archipelago gems swapped together are seeds already.
            for (p, gem) in [(a, ga), (b, gb)] {
                if gem.special == Special::Rainbow {
                    seeds.push(p);
                    spent.push(p);
                }
            }
            return Activation { seeds, spent, jitter_ms: matching::RAINBOW_SPREAD_MS };
        }

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
            // By matching color rather than raw color, so a rocket carrying
            // this one underneath is left where it is. A rocket is an item
            // holding its cell, not a gem in the pool of colors, and a rainbow
            // sweeping it up would cost the player a reward already earned:
            // the same durability a beam crossing it now respects.
            .filter(|p| *p != rainbow && self.board.match_color(*p) == Some(color))
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
            &resolution.fires,
            &mut self.rng,
            resolution.jitter_ms,
        );
        // Bricks as well as gems, because a beam can reach a brick without
        // taking a single gem with it: a cross fired down a column of nothing
        // but bricks clears none of them and breaks all of them.
        if blast.cleared.is_empty() && blast.struck.is_empty() {
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
        self.progress.jelly_left = self.board.jelly_cells();
        self.strike_bricks(&blast, cascade);
        // Worked out before the board is touched, because it reads what is
        // standing beside the clear.
        let ap_struck = self.ap_gems_struck(&blast);
        let mut collected = 0;
        for p in blast
            .cleared
            .iter()
            .copied()
            .filter(|p| self.board.is_ap_gem(*p))
            .chain(ap_struck.iter().copied())
        {
            self.events.push(Event::at(EV_AP_CLEAR, p, 255, Special::Archipelago, cascade));
            collected += 1;
        }
        // A check taken is a check gone, so the level has one fewer worth
        // dropping. Without this the cap is only on how many sit on the board
        // at once: collect one, the board has room again, and another falls
        // for a check that is no longer there.
        self.ap_gems_wanted = self.ap_gems_wanted.saturating_sub(collected);

        for (p, special) in &blast.fired {
            self.events.push(Event::at(EV_SPECIAL_FIRED, *p, 255, *special, cascade));
        }
        // A new special belongs to the match that made it rather than to what
        // follows, so it is on the board from this moment: it is seen coming
        // out of the gems instead of out of the hole they leave behind. The
        // gem it stands in for still counts as cleared and still bursts, but
        // it does not pop, because the special is already in its place.
        for (p, gem) in &resolution.creations {
            self.events.push(Event::at(EV_SPECIAL_MADE, *p, gem.color, gem.special, cascade));
            self.board.set_gem(*p, Some(*gem));
        }

        // The clear runs until the furthest cell has finished popping.
        let last = blast.delays.iter().copied().fold(0.0_f32, f32::max);
        self.clear_ms = POP_MS + last;
        self.clearing = blast
            .cleared
            .iter()
            .copied()
            .zip(blast.delays.iter().copied())
            .filter(|(p, _)| !resolution.creations.iter().any(|(made, _)| made == p))
            .collect();
        // Struck from the side rather than caught in the blast, so they pop
        // with the clear that took them rather than instantly.
        self.clearing.extend(ap_struck.into_iter().map(|p| (p, 0.0)));
        self.phase = Phase::Clearing { elapsed: 0.0 };
    }

    /// The board has come to rest: decide whether the level is over, the board
    /// is stuck, or the player is up.
    fn settle(&mut self) {
        // A chain ends when the board comes to rest. The flourish at the end
        // of a level is one chain of its own: this is the settle that noticed
        // the goal was met, so it still ends the player's chain and the
        // flourish starts again from the bottom of the progression. Every
        // settle after it is between two rounds of the flourish, with nobody
        // moving in between, so those do not break the chain and the music
        // climbs through to the end.
        if !self.announced_clear {
            self.cascade = 0;
        }
        self.swap = None;
        self.progress.jelly_left = self.board.jelly_cells();

        if self.objectives_met() {
            // Before the flourish, which spends them: afterwards the counter
            // always reads zero and how briskly the level was beaten is gone.
            self.progress.moves_spare = self.progress.moves_spare.max(self.moves_left);
            // Said once, the moment the level is won, rather than again on
            // every round of what follows.
            if !self.announced_clear {
                self.announced_clear = true;
                self.events.push(Event::plain(EV_CLEARED, self.moves_left.min(65_535) as u16));
            }
            // Once, and before anything else happens: what the winning clear
            // counted is still crossing the screen toward the goals it
            // counted for, and watching them finish is the point of the win.
            // See [`Phase::Tallying`].
            if !self.tallied {
                self.tallied = true;
                if self.goal_hold_ms > 0.0 {
                    self.phase = Phase::Tallying { elapsed: 0.0 };
                    return;
                }
            }
            // Leftover moves are spent one at a time so the counter can be
            // watched running down; only once it reaches zero does the board
            // go off.
            if self.moves_left > 0 {
                self.phase = Phase::CashingIn { elapsed: 0.0 };
                return;
            }
            if self.begin_finale() {
                return;
            }
            self.phase = Phase::Finishing { elapsed: 0.0 };
            return;
        }
        // Out of moves and stuck both hold the same beat a win does, and say
        // nothing until it is up: the last move deserves to be watched land.
        if self.spec.moves > 0 && self.moves_left == 0 {
            self.phase = Phase::Finishing { elapsed: 0.0 };
            return;
        }
        if matching::find_move(&self.board, &self.spec.rules).is_none() {
            if self.spec.rules.shuffle_when_stuck {
                self.phase = Phase::Shuffling { elapsed: 0.0 };
                self.events.push(Event::plain(EV_SHUFFLE, 0));
            } else {
                self.phase = Phase::Finishing { elapsed: 0.0 };
            }
            return;
        }
        self.phase = Phase::Idle;
    }

    /// Cashes in whatever is left once the goals are met.
    ///
    /// Every move still in hand turns a gem into a special, and then
    /// everything inert on the board goes off at once. What that clears can
    /// leave more specials behind, and the board coming to rest brings it back
    /// here to set those off too, round after round until there is nothing
    /// left to fire. The score climbs the whole way, which is what makes
    /// finishing a level early worth more than merely finishing it.
    ///
    /// It reuses the ordinary clear and fall rather than adding a phase: a
    /// finale round is a detonation that nobody swapped for. Returns whether
    /// there was anything to cash in, so [`Game::settle`] knows whether the
    /// level is actually over.
    ///
    /// A run that has unlocked nothing has nothing to mint, so the finale
    /// quietly does not happen. That is the same rule as everywhere else: the
    /// board can only make what the run may make.
    fn begin_finale(&mut self) -> bool {
        let waiting = self.inert_specials();
        if !waiting.is_empty() {
            // Another link in the same chain, so the music takes its next
            // step rather than repeating the chord the last round ended on.
            // `finish_fall` does this for the links inside a round; a round
            // beginning is one too.
            self.cascade += 1;
            self.begin_clear(Resolution {
                seeds: waiting,
                // The flourish setting itself off, which nobody swapped. The
                // specials it fires are ones it just minted onto the board, so
                // the wave finds them the ordinary way.
                matched: 0,
                creations: Vec::new(),
                spent: Vec::new(),
                fires: Vec::new(),
                jitter_ms: FINALE_JITTER_MS,
            });
            return true;
        }

        // Nothing inert, but the board may still hold rockets, which fly at
        // something rather than going off where they stand.
        let rockets = self.rockets_on_board();
        !rockets.is_empty() && self.begin_launch(&rockets)
    }

    /// Spends one leftover move, then either takes the next or sets the board
    /// off.
    ///
    /// One per step rather than all at once so the counter visibly runs down.
    /// A step with nothing to place still costs its move and still takes its
    /// time: a run that has unlocked none of the eligible specials goes
    /// through the same motions and simply leaves the board alone, which is
    /// less confusing than the count vanishing.
    fn finish_cash_in_step(&mut self) {
        if self.moves_left > 0 {
            self.moves_left -= 1;
            self.mint_one();
        }
        if self.moves_left > 0 {
            self.phase = Phase::CashingIn { elapsed: 0.0 };
            return;
        }
        if self.begin_finale() {
            return;
        }
        self.phase = Phase::Finishing { elapsed: 0.0 };
    }

    /// Spends one leftover move on a plain gem somewhere on the board,
    /// turning it into a special if the run holds one to give.
    ///
    /// Only gems are picked: a cell already holding a special is worth more
    /// left alone, and a brick is not a gem. Only the three that clear a line
    /// or a cross are eligible, drawn from what the level allows, so this can
    /// neither hand out something the run has not earned nor fill the board
    /// with rainbows.
    ///
    /// A cell is picked and announced even when there is nothing to put on it,
    /// so the spending is seen and heard wherever it lands. Only a placement
    /// that actually happened is worth points.
    fn mint_one(&mut self) {
        // Carrying nothing at all, rather than not carrying a special.
        //
        // Those are not the same set, because an Archipelago gem carries no
        // special by that measure: it answers to no color and fires no beam,
        // so [`Special::is_special`] says no, and it used to land in here and
        // be minted over. A gem is worth a few hundred points; one of these
        // is worth an item, and overwriting it takes the check off the board
        // for good, with nothing to say it ever happened.
        let plain: Vec<Pos> = self
            .board
            .positions()
            .filter(|p| self.board.gem(*p).map_or(false, |gem| gem.special == Special::None))
            .collect();
        let Some(&at) = plain.get(self.rng.below(plain.len() as u32) as usize) else { return };
        let Some(gem) = self.board.gem(at) else { return };

        let allowed: Vec<Special> = self
            .spec
            .rules
            .specials
            .list()
            .into_iter()
            .filter(|special| CASH_IN_SPECIALS.contains(special))
            .collect();
        let placed = match allowed.get(self.rng.below(allowed.len() as u32) as usize) {
            Some(&special) => {
                self.board.set_gem(at, Some(Gem { special, ..gem }));
                self.progress.score += SCORE_PER_SPECIAL_MADE;
                special
            }
            None => Special::None,
        };
        self.events.push(Event::at(EV_CASH_IN, at, gem.color, placed, 1));
    }

    /// The level is over. The beat before this is [`Phase::Finishing`].
    ///
    /// Which way it went is read back from whether the goals were ever met
    /// rather than carried along on the phase, because that is the same
    /// question: a level ends won when its objectives are behind it and lost
    /// when the moves ran out or the board died with them still ahead.
    ///
    /// Both the status and the event land here rather than where the board
    /// came to rest, so nothing outside can tell the level is over until the
    /// beat has been held. The panel goes up on the status, so setting it
    /// early would be the same as having no beat at all.
    fn declare_over(&mut self) {
        self.phase = Phase::Finished;
        if self.announced_clear {
            self.status = Status::Won;
            self.events.push(Event::plain(EV_WON, 0));
        } else {
            self.status = Status::Lost;
            self.events.push(Event::plain(EV_LOST, 0));
        }
    }

    /// Specials sitting on the board waiting for something to set them off.
    ///
    /// Rockets are left out: they fly at a target rather than going off where
    /// they stand, and the launch path already knows how to send them.
    fn inert_specials(&self) -> Vec<Pos> {
        self.board
            .positions()
            .filter(|p| {
                self.board.gem(*p).map_or(false, |gem| {
                    gem.special.is_special() && gem.special != Special::Rocket
                })
            })
            .collect()
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

    /// Fills every empty cell, avoiding matches that would resolve before the
    /// player has touched anything, and never leaving a board with no move on
    /// it.
    ///
    /// Empty, not every cell: a layout can place gems, and those are the
    /// opening board rather than a property of their cells, so the deal fills
    /// around them and the first clear refills them like anywhere else. A
    /// caller that wants the whole board dealt again empties it first.
    ///
    /// The two failures are not the same failure. A board that opens mid-match
    /// plays a little of itself before the player has touched it, which is
    /// untidy; a board with no legal move on it is not a game at all. So the
    /// first is worth a budget of attempts and is accepted when that runs out,
    /// and the second is not accepted.
    fn deal(&mut self) {
        // Brick cells are open ground with something already standing on it,
        // so they are dealt around rather than into.
        let mine: Vec<Pos> = self
            .board
            .positions()
            .filter(|p| {
                self.board.brick(*p) == 0
                    && self.board.is_open(*p)
                    && self.board.gem(*p).is_none()
            })
            .collect();

        for _ in 0..QUIET_DEAL_TRIES {
            self.fill(&mine);
            if matching::find_matches(&self.board, &self.spec.rules).is_empty()
                && matching::find_move(&self.board, &self.spec.rules).is_some()
            {
                return;
            }
        }
        // The budget is spent, so an opening match is accepted: a level whose
        // placed gems make one unavoidable should still be playable, and the
        // board resolves it and carries on. A board with nothing to do is
        // dealt again until there is something, which takes a pathological
        // layout to need twice and is bounded so that one cannot hang the
        // page. `every_level_opens_with_something_to_do` is what would catch
        // such a layout, at build time rather than in front of a player.
        for _ in 0..PLAYABLE_DEAL_TRIES {
            if matching::find_move(&self.board, &self.spec.rules).is_some() {
                return;
            }
            self.fill(&mine);
        }
    }

    /// Deals a color into each of `cells`, in reading order.
    fn fill(&mut self, cells: &[Pos]) {
        for p in cells.iter().copied() {
            let mut color = 0;
            // A handful of tries is enough to dodge a starting match; if the
            // colors run out we accept it and let the caller decide.
            for _ in 0..12 {
                color = self.spec.rules.draw_color(&mut self.rng);
                if !self.would_start_a_shape(p, color) {
                    break;
                }
            }
            self.board.set_gem(p, Some(Gem::plain(color)));
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
        // a fresh one rather than leaving the player stuck. Emptied first,
        // because a deal fills what is empty: this is the whole board being
        // dealt again, and anything a layout placed belonged to the opening
        // board rather than to this one.
        for p in self.board.occupied() {
            self.board.set_gem(p, None);
        }
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

    /// Three floats per rocket spent out of the inventory: a column, a row,
    /// and 1 while it is still flying. See [`Game::flights_buf`].
    ///
    /// The column and the row are the board's own, so a row of `rows` is the
    /// one below the bottom, and both carry a fraction: this is a position on
    /// the grid rather than a cell in it. Empty except while such a rocket is
    /// in the air, which is only ever after one was spent.
    pub fn flights(&self) -> &[f32] {
        &self.flights_buf
    }

    pub const FLAG_WALL: u8 = 1;
    pub const FLAG_CLEARING: u8 = 2;
    pub const FLAG_SELECTED: u8 = 4;
    /// Something immovable stands here. With [`Game::FLAG_CRACKED`] as well it
    /// is the damaged one, which the next hit breaks.
    pub const FLAG_BRICK: u8 = 8;
    pub const FLAG_CRACKED: u8 = 16;
    /// And it is the kind keyed to a color, which the cell's color byte
    /// carries. Without this the cell holds a plain brick and the color byte
    /// is the empty marker.
    pub const FLAG_SEAL: u8 = 32;

    fn refresh_snapshot(&mut self) {
        let count = (self.board.rows * self.board.cols) as usize;
        self.cells_buf.clear();
        self.cells_buf.resize(count * 4, 0);
        self.offs_buf.clear();
        self.offs_buf.resize(count * 3, 0.0);
        self.flights_buf.clear();

        for p in self.board.positions() {
            let i = (p.r * self.board.cols + p.c) as usize;
            let cell = self.board.cell(p).expect("position came from the board");
            // A blocker holds no gem, so the color byte is free for the color a
            // seal answers to. A cell carrying one is flagged, and the front
            // end draws the seal rather than reading this as a gem.
            let (color, special) = match cell.gem {
                Some(gem) => (gem.color, gem.special.code()),
                None if cell.brick > 0 => (cell.brick_color, 0),
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
                if cell.brick_color != ANY_COLOR {
                    flags |= Self::FLAG_SEAL;
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
                    let (from, to) = (launch.from, launch.to);
                    let distance = cells_between(from, to).max(0.001);
                    let flown = (flown_cells(elapsed) / distance).clamp(0.0, 1.0);
                    let (dx, dy) = lob(from, to, flown);
                    if launch.from_inventory {
                        // No cell holds this one, so where it is goes in a
                        // list of its own. A landed one keeps its place in
                        // that list rather than being left out of it: the
                        // front end tells one rocket from the next by where
                        // it sits, and dropping one would hand every rocket
                        // behind it somebody else's heading.
                        self.flights_buf.extend_from_slice(&[
                            from.c as f32 + dx,
                            from.r as f32 + dy,
                            if launch.landed { 0.0 } else { 1.0 },
                        ]);
                        continue;
                    }
                    if launch.landed {
                        continue;
                    }
                    self.set_offset(from, dx, dy);
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
            // Nothing moves during these: the board sits where it is while the
            // counter runs down, or while a finished board is looked at.
            Phase::Idle
            | Phase::Tallying { .. }
            | Phase::CashingIn { .. }
            | Phase::Finishing { .. }
            | Phase::Finished => {}
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

/// How far off the straight line a rocket swings at the top of its arc, as a
/// share of how far it is going.
///
/// A quadratic curve sits half way to its control point at the middle, so the
/// widest the flight gets is half of this. More pronounced than the arc a
/// cleared gem takes to its goal, because a rocket is a thing being thrown
/// rather than a mote drifting: at a fifth of the distance it reads as a lob
/// from across the board, and anything much past that starts flying around
/// the board rather than over it.
const LAUNCH_BOW: f32 = 0.42;

/// Where a rocket is when it is `flown` of the way from `from` to `to`, as an
/// offset from `from` in cells.
///
/// A curve rather than a straight line, and bowed the same way every time: up
/// the board, so it reads as something lobbed over the gems in between rather
/// than fired through them. A rocket going straight up or down has no up to
/// bow toward, so those take their side from where they started, which keeps
/// two of them in the same column from tracing the same path.
///
/// Parameterized by the fraction of the *straight line* covered, not of the
/// curve, so the arc changes where the rocket appears without changing when it
/// gets there. The timing is the engine's promise to the sound and to the
/// clear that follows; the shape is not.
fn lob(from: Pos, to: Pos, flown: f32) -> (f32, f32) {
    let (dx, dy) = ((to.c - from.c) as f32, (to.r - from.r) as f32);
    let distance = (dx * dx + dy * dy).sqrt();
    if distance < 0.001 {
        return (0.0, 0.0);
    }

    // Square to the flight, and of the two ways to turn, the one that goes up.
    let (mut px, mut py) = (dy / distance, -dx / distance);
    if py > 0.0 || (py == 0.0 && (from.r + from.c) % 2 == 0) {
        px = -px;
        py = -py;
    }

    let bow = distance * LAUNCH_BOW;
    let (cx, cy) = (dx * 0.5 + px * bow, dy * 0.5 + py * bow);
    let t = flown.clamp(0.0, 1.0);
    let u = 1.0 - t;
    (2.0 * u * t * cx + t * t * dx, 2.0 * u * t * cy + t * t * dy)
}

/// Distance covered after `elapsed` milliseconds by something that accelerates
/// evenly for `ramp_ms` and then holds `speed`.
fn traveled(elapsed: f32, ramp_ms: f32, speed: f32) -> f32 {
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
    traveled(elapsed, LAUNCH_RAMP_MS, LAUNCH_SPEED)
}

/// How long a rocket needs to cover `distance` cells.
fn flight_time(distance: f32) -> f32 {
    travel_time(distance, LAUNCH_RAMP_MS, LAUNCH_SPEED)
}

/// How far a gem has fallen after `elapsed` milliseconds.
fn fallen_cells(elapsed: f32) -> f32 {
    traveled(elapsed, FALL_ACCEL_MS, FALL_SPEED)
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
            moves_upgrade: 0,
            objectives: vec![Objective::Score(1_000_000)],
            silver: 0,
            gold: 0,
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
            assert!(
                matching::find_move(&game.board, game.rules()).is_some(),
                "seed {seed} dealt a dead board",
            );
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
    fn a_new_special_is_there_while_the_match_that_made_it_is_still_popping() {
        // Waiting for the pops to finish made the gem look like it arrived
        // from nowhere. It belongs to the match, so it is on the board from
        // the moment the match resolves, standing still while the rest goes.
        let mut game = Game::new(spec(4, 4, 6, 10), 73);
        paint(&mut game, &square_board());
        assert!(game.try_swap(Pos::new(1, 1), Pos::new(1, 2)));

        let mut frames = 0;
        while !matches!(game.phase(), Phase::Clearing { .. }) && frames < 100 {
            game.update(16.0);
            frames += 1;
        }
        assert!(matches!(game.phase(), Phase::Clearing { .. }), "the swap never resolved");

        let made = Pos::new(1, 1);
        assert_eq!(
            game.board.gem(made).map(|gem| gem.special),
            Some(Special::Rocket),
            "the rocket should be on the board as soon as the match resolves"
        );

        let slot = (made.r * game.board.cols + made.c) as usize;
        let mut saw_a_neighbor_pop = false;
        while matches!(game.phase(), Phase::Clearing { .. }) && frames < 200 {
            game.update(16.0);
            frames += 1;
            if !matches!(game.phase(), Phase::Clearing { .. }) {
                break;
            }
            assert_eq!(
                game.cells_bytes()[slot * 4 + 3] & Game::FLAG_CLEARING,
                0,
                "the new rocket was marked as clearing at frame {frames}"
            );
            assert_eq!(
                game.offsets()[slot * 3 + 2],
                1.0,
                "the new rocket shrank with the match at frame {frames}"
            );
            // The corner above it is part of the same square and does pop,
            // which is what says this loop ran during a clear rather than
            // over an empty one.
            let corner = Pos::new(0, 1);
            let neighbor = (corner.r * game.board.cols + corner.c) as usize;
            saw_a_neighbor_pop |=
                game.cells_bytes()[neighbor * 4 + 3] & Game::FLAG_CLEARING != 0;
        }
        assert!(saw_a_neighbor_pop, "nothing was popping, so the check proved nothing");
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
        while game.board.settle_stage(&rules, &mut game.rng, 0).is_some() {}

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

        let blast = matching::detonate(&game.board, &[Pos::new(3, 0)], &[], &[], &mut game.rng, 0.0);
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
            from_inventory: false,
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
            from_inventory: false,
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
            from_inventory: false,
        };
        let far = Launch {
            from: Pos::new(7, 0),
            to: Pos::new(7, 7),
            flight_ms: flight_time(7.0),
            landed: false,
            from_inventory: false,
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
        let shot = Launch {
            from,
            to,
            flight_ms: flight_time(cells_between(from, to)),
            landed: false,
            from_inventory: false,
        };
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

    /// A still board with nothing matched on it yet, for weighing what one
    /// swap lines up.
    fn quiet_game(layout: &[&str]) -> Game {
        let rows = layout.len() as i32;
        let cols = layout[0].len() as i32;
        let mut game = Game::new(spec(rows, cols, 8, 10), 1);
        game.spec.rules.refill = RefillMode::None;
        game.board = Board::from_layout(layout);
        assert!(
            matching::find_matches(&game.board, &game.spec.rules).is_empty(),
            "the board already had a match on it before anything was swapped",
        );
        game
    }

    /// Swaps two cells, plays the board out, and says how many gems the
    /// player lined up along with everything that happened afterwards.
    ///
    /// The number is read the frame the swap resolves, which is before
    /// anything it sets off. The events have to be gathered as they go: the
    /// clear the swap starts is raised on that same frame, and `update`
    /// empties its list on the next one.
    fn swap_and_settle(game: &mut Game, a: Pos, b: Pos) -> (u32, Vec<Event>) {
        assert!(game.try_swap(a, b), "the swap was refused");
        let mut lined_up = None;
        let mut seen = Vec::new();
        for _ in 0..4000 {
            game.update(16.0);
            seen.extend_from_slice(game.events());
            if lined_up.is_none() && !matches!(game.phase(), Phase::Swapping { .. }) {
                lined_up = Some(game.swap_match());
            }
            if game.phase() == Phase::Idle || game.phase() == Phase::Finished {
                return (lined_up.expect("the swap never resolved"), seen);
            }
        }
        panic!("board never settled");
    }

    fn lined_up(game: &mut Game, a: Pos, b: Pos) -> u32 {
        swap_and_settle(game, a, b).0
    }

    #[test]
    fn a_rocket_arcs_over_the_board_and_still_lands_where_it_was_aimed() {
        let from = Pos::new(6, 1);
        let to = Pos::new(6, 7);

        // Both ends are where they were: the arc is the middle of the flight,
        // not a different flight.
        assert_eq!(lob(from, to, 0.0), (0.0, 0.0));
        let (dx, dy) = lob(from, to, 1.0);
        assert!((dx - 6.0).abs() < 0.001 && dy.abs() < 0.001, "it landed at ({dx}, {dy})");

        // And it leaves the straight line in between, upward: rows count down
        // the screen, so a lob is a negative row offset.
        let (mx, my) = lob(from, to, 0.5);
        assert!((mx - 3.0).abs() < 0.001, "the middle of the flight slid along to {mx}");
        assert!(my < -0.5, "the middle of the flight only rose to {my}");
        // Half the control point's own reach, which is what a quadratic does.
        assert!((my + 6.0 * LAUNCH_BOW / 2.0).abs() < 0.001, "the arc is {my} high");

        // A flight straight down the board has no up to bow toward, so it
        // takes a side instead, and two starting on different squares do not
        // trace the same path.
        let down = lob(Pos::new(1, 4), Pos::new(7, 4), 0.5);
        let beside = lob(Pos::new(1, 5), Pos::new(7, 5), 0.5);
        assert!(down.0.abs() > 0.5, "a vertical flight went straight: {down:?}");
        assert_eq!(down.0, -beside.0, "two vertical flights bowed the same way");

        // Nothing to arc over when it is going nowhere.
        assert_eq!(lob(from, from, 0.5), (0.0, 0.0));
    }

    #[test]
    fn a_rocket_in_the_air_is_never_off_the_board() {
        // The arc puts the rocket somewhere no cell is, which is the point,
        // but it has to stay somewhere the board is drawn: a lob that leaves
        // the top of a nine row board is a rocket that vanishes mid-flight.
        //
        // The longest flights are corner to corner, which is where the arc
        // reaches furthest.
        for (from, to) in [
            (Pos::new(0, 0), Pos::new(8, 8)),
            (Pos::new(8, 0), Pos::new(0, 8)),
            (Pos::new(8, 8), Pos::new(0, 0)),
            (Pos::new(4, 0), Pos::new(4, 8)),
            (Pos::new(0, 4), Pos::new(8, 4)),
        ] {
            for step in 0..=20 {
                let t = step as f32 / 20.0;
                let (dx, dy) = lob(from, to, t);
                let (r, c) = (from.r as f32 + dy, from.c as f32 + dx);
                // A cell of slack at each edge, because a rocket half off the
                // top still reads as a rocket and the board is drawn with a
                // margin around it.
                assert!(
                    (-1.0..9.0).contains(&r) && (-1.0..9.0).contains(&c),
                    "flying {from:?} to {to:?}, at {t} it was at row {r}, column {c}",
                );
            }
        }
    }

    /// Plays a consumable out and hands back everything it raised.
    fn spend(game: &mut Game, kind: Consumable, target: Option<Pos>) -> (bool, Vec<Event>) {
        let spent = game.use_consumable(kind, target);
        // Taken before a frame goes by, because spending one raises its whole
        // first clear then and there and `update` empties the list.
        let mut seen: Vec<Event> = game.events().to_vec();
        for _ in 0..4000 {
            if game.phase() == Phase::Idle || game.phase() == Phase::Finished {
                break;
            }
            game.update(16.0);
            seen.extend_from_slice(game.events());
        }
        (spent, seen)
    }

    #[test]
    fn a_spent_rocket_strikes_where_it_was_pointed_and_costs_no_move() {
        let mut game = Game::new(spec(9, 9, 5, 20), 4);
        let at = Pos::new(4, 4);
        let before = game.moves_left;

        let (spent, seen) = spend(&mut game, Consumable::Rocket, Some(at));
        assert!(spent, "a rocket pointed at an open cell was refused");
        assert_eq!(game.moves_left, before, "a bonus cost a move");
        assert!(
            seen.iter().any(|e| e.kind == EV_ROCKET_HIT && (e.r, e.c) == (at.r as u8, at.c as u8)),
            "nothing struck the cell it was pointed at",
        );

        // Nothing to point at is a refusal rather than a waste. The caller
        // only takes the item out of the inventory when this says yes.
        let mut game = Game::new(spec(9, 9, 5, 20), 4);
        assert!(!game.use_consumable(Consumable::Rocket, None), "an aimed one fired unaimed");
        assert!(
            !game.use_consumable(Consumable::Rocket, Some(Pos::new(99, 99))),
            "it fired at a cell off the board",
        );
    }

    #[test]
    fn a_spent_rainbow_takes_the_color_it_was_pointed_at() {
        let mut game = Game::new(spec(9, 9, 5, 20), 4);
        let at = Pos::new(4, 4);
        let color = game.board.color(at).expect("a gem to point at");
        let wearing =
            game.board.positions().filter(|p| game.board.match_color(*p) == Some(color)).count();
        assert!(wearing > 3, "this board has too few of that color to tell anything");

        let (spent, seen) = spend(&mut game, Consumable::Rainbow, Some(at));
        assert!(spent);
        let took = seen.iter().filter(|e| e.kind == EV_CLEAR && e.color == color).count();
        assert!(took >= wearing, "it took {took} of the {wearing} on the board");

        // An Archipelago gem answers to no color, and spending a rainbow to
        // clear one cell would be the worst trade in the game. Refused.
        let mut game = Game::new(spec(9, 9, 5, 20), 4);
        game.board.set_gem(at, Some(Gem::archipelago()));
        assert!(
            !game.use_consumable(Consumable::Rainbow, Some(at)),
            "a rainbow was spent on a check",
        );

        // Against another rainbow it takes the board, the same as swapping
        // two together.
        let mut game = Game::new(spec(9, 9, 5, 20), 4);
        game.board.set_gem(at, Some(Gem { color: 0, special: Special::Rainbow }));
        let occupied = game.board.occupied().len();
        let (spent, seen) = spend(&mut game, Consumable::Rainbow, Some(at));
        assert!(spent);
        let cleared = seen.iter().filter(|e| e.kind == EV_CLEAR).count();
        assert!(cleared >= occupied, "it took {cleared} of the {occupied} on the board");
    }

    #[test]
    fn a_spent_cross_takes_the_row_and_the_column() {
        let mut game = Game::new(spec(9, 9, 5, 20), 4);
        game.spec.rules.refill = RefillMode::None;
        let at = Pos::new(4, 6);

        let (spent, seen) = spend(&mut game, Consumable::CrossClear, Some(at));
        assert!(spent);
        let struck: Vec<(u8, u8)> =
            seen.iter().filter(|e| e.kind == EV_CLEAR).map(|e| (e.r, e.c)).collect();
        for c in 0..9u8 {
            assert!(struck.contains(&(4, c)), "the row missed column {c}");
        }
        for r in 0..9u8 {
            assert!(struck.contains(&(r, 6)), "the column missed row {r}");
        }
    }

    #[test]
    fn a_spent_cluster_fires_a_handful_and_aims_none_of_them() {
        // Three to five, each choosing its own target the way any rocket does,
        // and no cell named by the player.
        let mut seen_counts = Vec::new();
        for seed in 0..40 {
            let mut game = Game::new(spec(9, 9, 5, 20), seed);
            let before = game.moves_left;
            assert!(
                game.use_consumable(Consumable::RocketCluster, None),
                "a cluster was refused on a full board",
            );
            // Counted where they set off rather than over the whole settle:
            // what the strikes go on to mint is the board's doing, and a
            // rocket the cascade made fires the same event.
            let hits = game
                .events()
                .iter()
                .filter(|e| e.kind == EV_SPECIAL_FIRED && e.special == Special::Rocket.code())
                .count();
            seen_counts.push(hits);
            settle(&mut game);
            assert_eq!(game.moves_left, before, "a bonus cost a move");
        }
        let least = *seen_counts.iter().min().unwrap();
        let most = *seen_counts.iter().max().unwrap();
        assert_eq!(least, 3, "the smallest cluster was {least}");
        assert_eq!(most, 5, "the biggest cluster was {most}");
    }

    #[test]
    fn a_spent_rocket_is_reported_in_the_air_because_no_cell_holds_it() {
        // One fired off the board is a gem in a cell and rides that cell's
        // offset. One spent out of the inventory comes up from under the
        // bottom row, where there is no cell to hang an offset on, so the
        // snapshot has a list of its own for them.
        let mut game = Game::new(spec(9, 9, 5, 20), 4);
        let at = Pos::new(4, 4);
        assert!(game.flights().is_empty(), "something was in the air before anything was spent");
        assert!(game.use_consumable(Consumable::Rocket, Some(at)));

        let mut rows = Vec::new();
        for _ in 0..400 {
            if game.phase() == Phase::Idle || game.phase() == Phase::Finished {
                break;
            }
            game.update(8.0);
            if let Phase::Launching { .. } = game.phase {
                assert_eq!(game.flights().len(), 3, "one rocket, one slot");
                if game.flights()[2] == 1.0 {
                    rows.push(game.flights()[1]);
                }
            }
        }

        let first = rows.first().copied().expect("the rocket was never seen flying");
        let last = rows.last().copied().expect("the rocket was never seen flying");
        assert!(rows.len() > 3, "it was only in the air for {} frames", rows.len());
        // It starts under the board, which is where the bar it came out of is,
        // and it works its way up toward the row it was aimed at. Against the
        // board's own size rather than a number written here: what this says
        // is that it came from under the board, whatever size the board is.
        let bottom = (game.board.rows - 1) as f32;
        assert!(first > bottom, "it set off from row {first}, which is on the board");
        assert!(last < first, "it ended at row {last}, having started at row {first}");
    }

    #[test]
    fn a_landed_rocket_keeps_its_place_in_the_list() {
        // A cluster is several at once, and they come down one at a time. The
        // front end tells one from the next by where it sits in this list, so
        // a rocket dropping out of it as it lands would hand every rocket
        // behind it somebody else's heading: they would all jump a slot along
        // and turn to face wherever their neighbor had been going.
        let mut game = Game::new(spec(9, 9, 5, 20), 4);
        assert!(game.use_consumable(Consumable::RocketCluster, None));

        let mut slots = None;
        let mut landed = 0;
        for _ in 0..400 {
            let Phase::Launching { .. } = game.phase else { break };
            game.update(8.0);
            if let Phase::Launching { .. } = game.phase {
                let now = game.flights().len();
                assert_eq!(*slots.get_or_insert(now), now, "the list changed length mid-flight");
                landed = landed.max(game.flights().chunks(3).filter(|f| f[2] == 0.0).count());
            }
        }
        assert!(slots.unwrap_or(0) >= 9, "a cluster is three rockets at least");
        assert!(landed > 0, "no rocket was ever seen down while the others were still up");
    }

    #[test]
    fn a_spent_cross_clear_is_a_cross_going_off_and_not_a_shape_of_matches() {
        // Spending one is the player firing a cross, so it has to do what a
        // cross does. Handing the cells its beams cover in as though the
        // player had lined them all up there is not the same thing, and comes
        // apart in three places at once: a brick cell holds no gem, so the
        // clear drops it and the beam passes straight over the one thing it
        // was aimed at; cells the player lined up crack what they are beside,
        // which a beam does not; and they all pop together rather than
        // sweeping out from the middle.
        let at = Pos::new(1, 2);
        let brick = Pos::new(3, 2);

        // What a real cross does there, to be measured against.
        let mut real = bricked_game(180);
        real.board.set_gem(at, Some(Gem { color: 1, special: Special::Cross }));
        let blast = matching::detonate(&real.board, &[at], &[], &[], &mut real.rng, 0.0);
        assert!(blast.struck.contains(&brick), "a real cross does not reach the brick either");

        // Gems along the row, and one above the middle. Nothing next to the
        // brick: (2,2) is left empty on purpose, so the only thing that can
        // reach it is the beam itself rather than a clear going off beside it.
        let mut game = bricked_game(180);
        for c in 0..5 {
            game.board.set_gem(Pos::new(1, c), Some(Gem::plain(if c % 2 == 0 { 1 } else { 2 })));
        }
        game.board.set_gem(Pos::new(0, 2), Some(Gem::plain(3)));
        assert_eq!(game.board.brick(brick), 2, "the board did not start with a whole brick");

        let (spent, seen) = spend(&mut game, Consumable::CrossClear, Some(at));
        assert!(spent, "a cross clear aimed at an open cell was refused");
        assert!(
            seen.iter().any(|e| e.kind == EV_BRICK && (e.r, e.c) == (brick.r as u8, brick.c as u8)),
            "the beam went down the column and over the brick in it",
        );
        assert_eq!(game.board.brick(brick), 1, "the brick should have been cracked once");

        // And it sweeps outward rather than going off all at once, which is
        // what makes a beam read as a beam.
        let delay_at = |c: i32| {
            seen.iter()
                .find(|e| e.kind == EV_CLEAR && (e.r, e.c) == (1, c as u8))
                .map(|e| e.value)
                .unwrap_or_else(|| panic!("nothing cleared at (1, {c})"))
        };
        assert_eq!(delay_at(2), 0, "the cell it was aimed at should go first");
        assert!(
            delay_at(0) > delay_at(1) && delay_at(1) > delay_at(2),
            "the row popped at {}, {}, {} rather than sweeping out",
            delay_at(2),
            delay_at(1),
            delay_at(0),
        );
    }

    #[test]
    fn nothing_is_spent_on_a_board_that_is_not_the_players_to_touch() {
        // Mid-animation and after the level is over are both hands off: a
        // consumable is a thing you do on your turn.
        let mut game = Game::new(spec(9, 9, 5, 20), 4);
        let (a, b) = matching::find_move(&game.board, game.rules()).expect("a move");
        game.try_swap(a, b);
        assert!(!game.accepts_input(), "the board should be busy mid-swap");
        for kind in [Consumable::Rocket, Consumable::RocketCluster] {
            assert!(
                !game.use_consumable(kind, Some(Pos::new(4, 4))),
                "{kind:?} went off while the board was busy",
            );
        }
    }

    #[test]
    fn the_flourish_never_spends_a_move_on_an_archipelago_gem() {
        // One of these is a check standing on the board. Minting over it takes
        // the check away with it and the run can never find it, which is a
        // worse thing to spend a leftover move on than anything else there.
        //
        // The trap is that an Archipelago gem carries no special by
        // `is_special`, so "not a special" and "plain" are different sets and
        // the flourish was reading the wrong one.
        let mut game = Game::new(spec(4, 4, 6, 10), 5);
        game.spec.rules.refill = RefillMode::None;
        game.board = Board::from_layout(&["0*1*", "*2*3", "1*0*", "*3*2"]);
        let gems = game.board.ap_gems();
        assert_eq!(gems.len(), 8, "the board was meant to be half Archipelago gems");

        for _ in 0..200 {
            game.mint_one();
        }
        assert_eq!(game.board.ap_gems(), gems, "the flourish took an Archipelago gem");

        // And it was minting all along, so this is not passing because the
        // flourish had nothing to do.
        let made = game
            .board
            .positions()
            .filter(|p| game.board.gem(*p).map_or(false, |gem| gem.special.is_special()))
            .count();
        assert!(made > 0, "the flourish never placed anything, so nothing here is tested");

        // With nothing but checks on the board there is nothing to spend a
        // move on, and it spends it on nothing rather than on one of those.
        let mut all = Game::new(spec(2, 2, 6, 10), 5);
        all.spec.rules.refill = RefillMode::None;
        all.board = Board::from_layout(&["**", "**"]);
        let every = all.board.ap_gems();
        assert_eq!(every.len(), 4);
        for _ in 0..50 {
            all.mint_one();
        }
        assert_eq!(all.board.ap_gems(), every, "a board of nothing but checks lost one");
    }

    #[test]
    fn a_hint_is_drawn_from_every_move_the_board_allows() {
        // Always legal, never the same one every time, and never anything the
        // board does not allow. Walking the board and taking the first hit
        // would pass the first of those and fail the other two.
        let mut game = Game::new(spec(9, 9, 5, 60), 3);
        let legal = matching::legal_moves(&game.board, game.rules());
        assert!(legal.len() > 3, "this board offers {} moves, so a draw says little", legal.len());

        // Forgetting what it last said between asks, because it does not
        // forget on its own: that is the next test's business. This one is
        // about the draw.
        let mut seen = Vec::new();
        for _ in 0..200 {
            game.hinted = None;
            let move_ = game.hint().expect("a board with moves on it offered none");
            assert!(legal.contains(&move_), "{move_:?} is not a move this board allows");
            if !seen.contains(&move_) {
                seen.push(move_);
            }
        }
        assert_eq!(
            seen.len(),
            legal.len(),
            "two hundred draws from {} moves turned up only {}: {seen:?}",
            legal.len(),
            seen.len(),
        );

        // Nothing to offer on a board with nothing to do.
        let mut stuck = quiet_game(&["01", "10"]);
        assert_eq!(stuck.hint(), None, "a board with no move offered one");
    }

    #[test]
    fn a_board_has_one_hint_and_keeps_it_until_it_is_gone() {
        // The nudge goes away when the player touches anything and comes back
        // when they stop, so a hint that was drawn afresh each time would let
        // them tap twice for another suggestion, and again, until they liked
        // one. That is a reroll for free and a way to be shown every move on
        // the board without making any of them.
        let mut game = Game::new(spec(9, 9, 5, 60), 3);
        assert!(
            matching::legal_moves(&game.board, game.rules()).len() > 3,
            "this board offers too few moves for asking twice to mean anything",
        );

        let offered = game.hint().expect("a fresh board has a move");
        for _ in 0..50 {
            assert_eq!(game.hint(), Some(offered), "asking again dealt another hint");
        }

        // And it is retired by the move going away rather than by anything
        // announcing that it has. Taken off the board here rather than played:
        // playing it deals fresh gems into those very cells, and they may well
        // make the same two positions worth swapping all over again, which is
        // a perfectly good answer and not the one this is trying to catch.
        let (a, _) = offered;
        game.board.set_gem(a, None);
        assert!(
            !matching::is_useful_swap(&game.board, game.rules(), offered.0, offered.1),
            "the move this is about is somehow still on the board",
        );

        let next = game.hint().expect("this board still has moves");
        assert_ne!(next, offered, "the hint outlived the move it was pointing at");
        assert!(
            matching::legal_moves(&game.board, game.rules()).contains(&next),
            "{next:?} is not a move this board allows",
        );
    }

    #[test]
    fn asking_for_a_hint_changes_nothing_else_about_the_run() {
        // The draw comes from a stream of its own. Sharing the deal's would
        // make every gem that falls afterwards depend on how often the player
        // asked for help, and a run would stop being the same run.
        let mut asked = Game::new(spec(9, 9, 5, 60), 11);
        let mut quiet = Game::new(spec(9, 9, 5, 60), 11);
        for _ in 0..25 {
            asked.hint();
        }

        let (a, b) = matching::find_move(&quiet.board, quiet.rules()).expect("a move");
        assert!(asked.try_swap(a, b));
        assert!(quiet.try_swap(a, b));
        settle(&mut asked);
        settle(&mut quiet);

        let gems = |game: &Game| -> Vec<Option<Gem>> {
            game.board.positions().map(|p| game.board.gem(p)).collect()
        };
        assert_eq!(gems(&asked), gems(&quiet), "asking for hints dealt a different board");
        assert_eq!(asked.progress.score, quiet.progress.score);
    }

    #[test]
    fn a_swap_is_weighed_by_the_gems_it_lines_up() {
        // Three in a row is three.
        let mut game = quiet_game(&["01010", "10101", "03230", "22101", "01010"]);
        assert_eq!(lined_up(&mut game, Pos::new(2, 2), Pos::new(3, 2)), 3);

        // Five in a row is five, and not the three it contains: each size is
        // its own shape, so growing into a bigger one is not reaching a
        // smaller one.
        let mut game = quiet_game(&["01010", "10101", "03230", "22122", "01010"]);
        assert_eq!(lined_up(&mut game, Pos::new(2, 2), Pos::new(3, 2)), 5);

        // An L is two runs of three sharing a corner, which is five gems and
        // not six. Counting the runs rather than the cells would say six.
        let mut game = quiet_game(&["01010", "10201", "01210", "10322", "01210"]);
        assert_eq!(lined_up(&mut game, Pos::new(3, 2), Pos::new(4, 2)), 5);

        // One swap can line up two runs that touch nothing of each other's,
        // and the player lined up all six.
        let mut game = quiet_game(&["01010", "10101", "01210", "22101", "01010"]);
        assert_eq!(lined_up(&mut game, Pos::new(2, 2), Pos::new(3, 2)), 6);
    }

    #[test]
    fn a_swap_that_lines_nothing_up_is_worth_nothing() {
        let mut game = quiet_game(&["01010", "10101", "03230", "22101", "01010"]);
        assert_eq!(lined_up(&mut game, Pos::new(2, 2), Pos::new(3, 2)), 3);

        // And the next swap is the next swap's own answer, not a number left
        // over from the good one before it.
        let mut game = quiet_game(&["01010", "10101", "03230", "22101", "01010"]);
        game.swap_match = 5;
        game.spec.rules.revert_invalid = false;
        assert_eq!(lined_up(&mut game, Pos::new(0, 0), Pos::new(0, 1)), 0);

        // The same for a swap the board slides back, which is the other way a
        // move can come to nothing.
        let mut game = quiet_game(&["01010", "10101", "03230", "22101", "01010"]);
        game.swap_match = 5;
        assert!(game.spec.rules.revert_invalid, "this half is about the revert");
        assert_eq!(lined_up(&mut game, Pos::new(0, 0), Pos::new(0, 1)), 0);
    }

    #[test]
    fn only_the_gems_that_lined_up_are_counted() {
        // A gem carrying a beam is a gem of its color and matches like one,
        // so it counts as the one gem it is. What it goes on to clear is the
        // match's doing rather than part of the match: this row clears far
        // more than three and the swap is still a three.
        let mut game = quiet_game(&["01010", "10101", "03230", "22101", "01010"]);
        game.board
            .set_gem(Pos::new(3, 0), Some(Gem { color: 2, special: Special::LineH }));
        let (matched, seen) = swap_and_settle(&mut game, Pos::new(2, 2), Pos::new(3, 2));
        assert_eq!(matched, 3);
        let cleared = seen.iter().filter(|e| e.kind == EV_CLEAR).count();
        assert!(cleared > 3, "the beam should have taken the row, and took {cleared}");

        // A brick is broken beside a match rather than being part of one, so
        // it is not one of the gems that lined up.
        let mut game = quiet_game(&["01010", "1010=", "03230", "22101", "01010"]);
        assert_eq!(lined_up(&mut game, Pos::new(2, 2), Pos::new(3, 2)), 3);

        // Nor is a rainbow: swapping one against a gem takes a whole color,
        // and none of it is a match. A swap that matches nothing is worth
        // nothing however much it clears.
        let mut game = quiet_game(&["01010", "10101", "03230", "22101", "01010"]);
        game.board
            .set_gem(Pos::new(0, 0), Some(Gem { color: 0, special: Special::Rainbow }));
        let (matched, seen) = swap_and_settle(&mut game, Pos::new(0, 0), Pos::new(0, 1));
        assert_eq!(matched, 0);
        let cleared = seen.iter().filter(|e| e.kind == EV_CLEAR).count();
        assert!(cleared > 1, "the rainbow should have taken a color, and took {cleared}");
    }

    #[test]
    fn a_match_a_cascade_lines_up_is_not_the_players() {
        // The number is what the player did. A board that goes on to match
        // itself several times over is the board's doing, and none of it
        // changes what they swapped.
        //
        // A seed that chains, because a board that settles in one go proves
        // nothing here. Swept rather than pinned: which seeds chain moves with
        // the deal, and a hunt that finds one is a test that keeps working.
        let mut chained = None;
        for seed in 0..64 {
            let mut game = Game::new(spec(9, 9, 5, 30), seed);
            let (a, b) = game.hint().expect("a fresh board has a move");
            assert!(game.try_swap(a, b));

            let mut readings = Vec::new();
            // Watched as it goes: the board ends its chain when it comes to
            // rest, so the counter reads zero by the time it is idle.
            let mut deepest = 0;
            for _ in 0..4000 {
                game.update(16.0);
                deepest = deepest.max(game.cascade());
                if !matches!(game.phase(), Phase::Swapping { .. }) {
                    readings.push(game.swap_match());
                }
                if game.phase() == Phase::Idle || game.phase() == Phase::Finished {
                    break;
                }
            }
            if deepest > 1 {
                readings.dedup();
                chained = Some((seed, deepest, readings));
                break;
            }
        }

        let (seed, depth, readings) =
            chained.expect("no board in sixty-four seeds ever chained off its first move");
        assert_eq!(
            readings.len(),
            1,
            "on seed {seed}, a chain {depth} deep moved the number: {readings:?}",
        );
        assert!(readings[0] >= 3, "the hint's own swap matched {} gems", readings[0]);
    }

    /// A board with a brick in the middle of the bottom row and a match lined
    /// up beside it.
    fn bricked_game(seed: u64) -> Game {
        let mut game = Game::new(spec(4, 5, 8, 10), seed);
        game.spec.rules.refill = RefillMode::None;
        game.board = Board::from_layout(&[".....", ".....", ".....", "..=.."]);
        game.cascade = 1;
        game
    }

    /// The same, with a seal keyed to color 1 where the brick was.
    fn sealed_game(seed: u64) -> Game {
        let mut game = Game::new(spec(4, 5, 8, 10), seed);
        game.spec.rules.refill = RefillMode::None;
        game.board = Board::from_layout(&[".....", ".....", ".....", "..B.."]);
        game.cascade = 1;
        game
    }

    /// Clears a row of three of `color` at row 2, which sits directly over the
    /// blocker at (3,2), and hands back what that raised.
    fn clear_beside_the_blocker(game: &mut Game, color: u8) -> Vec<Event> {
        for c in 0..3 {
            game.board.set_gem(Pos::new(2, c), Some(Gem::plain(color)));
        }
        let resolution = game.plan_resolution(None).expect("the row should match");
        game.begin_clear(resolution);
        game.events().to_vec()
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
    fn a_seal_cracks_only_for_its_own_color() {
        // Color 1 is what this seal answers to. Three of color 4 going off
        // right on top of it do nothing at all.
        let mut game = sealed_game(190);
        let seal = Pos::new(3, 2);
        let wrong = clear_beside_the_blocker(&mut game, 4);
        assert_eq!(game.board.brick(seal), 2, "the wrong color should not have touched it");
        assert!(wrong.iter().all(|e| e.kind != EV_BRICK));

        // The same clear in the color it answers to cracks it.
        let mut game = sealed_game(191);
        let right = clear_beside_the_blocker(&mut game, 1);
        assert_eq!(game.board.brick(seal), 1, "its own color should have cracked it");
        let broke: Vec<&Event> = right.iter().filter(|e| e.kind == EV_BRICK).collect();
        assert_eq!(broke.len(), 1);
        assert_eq!(broke[0].color, 1, "and the event should carry the color it was");
    }

    #[test]
    fn a_plain_brick_answers_to_any_color() {
        // The difference between the two: a brick does not care what broke it.
        let mut game = bricked_game(192);
        clear_beside_the_blocker(&mut game, 4);
        assert_eq!(game.board.brick(Pos::new(3, 2)), 1, "a brick takes any color");
    }

    #[test]
    fn a_beam_shoots_a_seal_whatever_color_it_is() {
        // Being shot is not a question of color: the beam goes through the seal
        // and counts against it the same as it would a brick.
        let mut game = sealed_game(193);
        let seal = Pos::new(3, 2);
        game.board.set_gem(Pos::new(3, 0), Some(Gem { color: 4, special: Special::LineH }));
        game.board.set_gem(Pos::new(3, 4), Some(Gem::plain(4)));

        let blast =
            matching::detonate(&game.board, &[Pos::new(3, 0)], &[], &[], &mut game.rng, 0.0);
        assert!(blast.struck.contains(&seal), "the beam should have marked it");
        game.strike_bricks(&blast, 1);
        assert_eq!(game.board.brick(seal), 1, "and cracked it, though the color was wrong");
    }

    #[test]
    fn a_beam_running_over_a_gem_does_not_crack_the_brick_beside_it() {
        // The gem at (2,2) sits directly over the brick. Taken by a match it
        // would crack it; swept up by a beam crossing row 2 it does not,
        // because a beam is a line drawn across the board rather than
        // something the player lined up there.
        let mut game = bricked_game(170);
        game.board.set_gem(Pos::new(2, 0), Some(Gem { color: 1, special: Special::LineH }));
        for c in 1..5 {
            game.board.set_gem(Pos::new(2, c), Some(Gem::plain(c as u8 % 3 + 1)));
        }

        let blast =
            matching::detonate(&game.board, &[Pos::new(2, 0)], &[], &[], &mut game.rng, 0.0);
        assert!(blast.cleared.contains(&Pos::new(2, 2)), "the beam took the gem over the brick");
        game.strike_bricks(&blast, 1);

        assert_eq!(game.board.brick(Pos::new(3, 2)), 2, "the brick should be untouched");
        assert!(game.events().iter().all(|e| e.kind != EV_BRICK));
    }

    #[test]
    fn a_rainbow_sweeping_up_a_color_cracks_what_it_goes_off_beside() {
        // A rainbow takes its color wherever it is on the board, and each of
        // those is a gem going away because the player spent the rainbow on it.
        // So it hits what it is next to, unlike a beam.
        let mut game = bricked_game(171);
        game.board.set_gem(Pos::new(2, 2), Some(Gem::plain(1)));
        game.board.set_gem(Pos::new(0, 4), Some(Gem::plain(1)));

        // Seeded the way a spent rainbow seeds its color: the cells themselves.
        let blast = matching::detonate(
            &game.board,
            &[Pos::new(2, 2), Pos::new(0, 4)],
            &[],
            &[],
            &mut game.rng,
            0.0,
        );
        game.strike_bricks(&blast, 1);

        assert_eq!(game.board.brick(Pos::new(3, 2)), 1, "the brick under it should be cracked");
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

    /// Flies a rocket from `from` to `to` and runs the clock until it lands,
    /// handing back everything raised on the way.
    fn strike(game: &mut Game, from: Pos, to: Pos) -> Vec<Event> {
        game.board.set_gem(from, Some(Gem { color: 2, special: Special::Rocket }));
        game.launches = vec![Launch {
            from,
            to,
            flight_ms: flight_time(cells_between(from, to)),
            landed: false,
            from_inventory: false,
        }];
        game.launch_ms = game.launches[0].flight_ms + LAUNCH_HOLD_MS;
        game.phase = Phase::Launching { elapsed: 0.0 };

        let mut seen = Vec::new();
        for _ in 0..400 {
            game.update(16.0);
            seen.extend_from_slice(game.events());
            if seen.iter().any(|e| e.kind == EV_ROCKET_HIT) {
                break;
            }
        }
        seen
    }

    #[test]
    fn a_rainbow_leaves_a_waiting_rocket_standing() {
        // The rocket is carrying the color the rainbow is spent on. It is an
        // item holding its cell rather than a gem in the pool of colors, so the
        // sweep goes around it, the same as a beam crossing it does.
        let mut game = Game::new(spec(6, 6, 6, 10), 200);
        paint(&mut game, &latin_board());
        game.board.set_gem(Pos::new(0, 0), Some(Gem { color: 0, special: Special::Rainbow }));
        // (0,1) is color 1, so the swap spends the rainbow on color 1. Make one
        // of the other 1s a rocket waiting to go.
        let waiting = Pos::new(1, 0);
        assert_eq!(game.board.color(waiting), Some(1), "the fixture should put a 1 here");
        game.board.set_gem(waiting, Some(Gem { color: 1, special: Special::Rocket }));

        assert!(game.try_swap(Pos::new(0, 0), Pos::new(0, 1)));
        let cleared = first_clear(&mut game);

        assert!(
            !cleared.iter().any(|e| (e.r, e.c) == (waiting.r as u8, waiting.c as u8)),
            "the rainbow should have swept around the rocket",
        );
        assert_eq!(
            game.board.gem(waiting).map(|g| g.special),
            Some(Special::Rocket),
            "and left it standing",
        );
    }

    #[test]
    fn a_rocket_can_be_aimed_at_a_brick() {
        // Everything else that breaks a brick is incidental: something went off
        // beside it, or a beam happened to run through it. A rocket is the one
        // thing that can be pointed at one deliberately.
        let mut game = bricked_game(166);
        let brick = Pos::new(3, 2);
        let events = strike(&mut game, Pos::new(0, 0), brick);

        let struck: Vec<&Event> = events.iter().filter(|e| e.kind == EV_BRICK).collect();
        assert_eq!(struck.len(), 1, "the strike should have hit the brick");
        assert_eq!((struck[0].r, struck[0].c), (brick.r as u8, brick.c as u8));
        assert_eq!(struck[0].value, 1, "cracked, not broken outright");
        assert_eq!(game.board.brick(brick), 1);
        assert!(
            events.iter().any(|e| e.kind == EV_ROCKET_HIT),
            "and it still lands as a strike, with the boom that goes with one",
        );
    }

    /// A board of one plain color with a rocket at (0,0), ready to have the
    /// interesting cells put on it.
    fn targeting_game(seed: u64, objectives: Vec<Objective>) -> Game {
        let mut spec = spec(5, 5, 6, 10);
        spec.objectives = objectives;
        let mut game = Game::new(spec, seed);
        for p in game.board.positions().collect::<Vec<_>>() {
            game.board.set_gem(p, Some(Gem::plain(3)));
        }
        game.board.set_gem(Pos::new(0, 0), Some(Gem { color: 3, special: Special::Rocket }));
        game
    }

    /// Where a rocket at (0,0) aims, over enough draws to see the whole pool.
    fn aim_spread(game: &mut Game) -> Vec<Pos> {
        let mut seen = Vec::new();
        for _ in 0..300 {
            for (_, to) in game.pick_targets(&[Pos::new(0, 0)]) {
                if !seen.contains(&to) {
                    seen.push(to);
                }
            }
        }
        seen
    }

    #[test]
    fn no_two_rockets_share_a_target() {
        // Four rockets and five gems: a pool barely bigger than the volley, so
        // a duplicate turns up immediately if targets are drawn rather than
        // taken out of the pool.
        let mut game = targeting_game(184, vec![Objective::Score(10_000)]);
        for p in game.board.positions().collect::<Vec<_>>() {
            game.board.set_gem(p, None);
        }
        let rockets: Vec<Pos> = (0..4).map(|c| Pos::new(0, c)).collect();
        for p in &rockets {
            game.board.set_gem(*p, Some(Gem { color: 3, special: Special::Rocket }));
        }
        for c in 0..5 {
            game.board.set_gem(Pos::new(4, c), Some(Gem::plain(3)));
        }

        for _ in 0..200 {
            let launches = game.pick_targets(&rockets);
            assert_eq!(launches.len(), 4, "every rocket should have found something");
            let mut taken: Vec<Pos> = launches.iter().map(|(_, to)| *to).collect();
            taken.sort_by_key(|p| (p.r, p.c));
            taken.dedup();
            assert_eq!(taken.len(), 4, "two rockets were sent at the same cell");
        }
    }

    #[test]
    fn a_volley_bigger_than_its_best_targets_drops_to_the_next_tier() {
        // One gem worth having and three rockets. The first takes it and the
        // other two go elsewhere, rather than all three piling onto the one
        // cell because it is the best thing on the board.
        let mut game = targeting_game(185, vec![Objective::Color { color: 5, count: 10 }]);
        let prize = Pos::new(4, 4);
        game.board.set_gem(prize, Some(Gem::plain(5)));
        let rockets: Vec<Pos> = (0..3).map(|c| Pos::new(0, c)).collect();
        for p in &rockets {
            game.board.set_gem(*p, Some(Gem { color: 3, special: Special::Rocket }));
        }

        let launches = game.pick_targets(&rockets);
        assert_eq!(launches.len(), 3, "all three should have found something");
        assert_eq!(
            launches.iter().filter(|(_, to)| *to == prize).count(),
            1,
            "the one gem worth shooting should be shot once",
        );
    }

    #[test]
    fn a_rocket_goes_for_whatever_moves_an_objective_along() {
        // One gem of the wanted color and one sitting on jelly, in a board of
        // gems that are neither. Those two are the whole pool.
        let mut game = targeting_game(
            180,
            vec![Objective::Color { color: 5, count: 10 }, Objective::Jelly],
        );
        game.board.set_gem(Pos::new(4, 4), Some(Gem::plain(5)));
        game.board.cell_mut(Pos::new(2, 1)).unwrap().jelly = 1;
        game.progress.jelly_total = 1;
        game.progress.jelly_left = 1;

        let seen = aim_spread(&mut game);
        seen.iter().for_each(|p| {
            assert!(
                *p == Pos::new(4, 4) || *p == Pos::new(2, 1),
                "aimed at {p:?}, which does no objective any good",
            );
        });
        assert_eq!(seen.len(), 2, "and both kinds of progress should come up");
    }

    #[test]
    fn a_rocket_does_not_aim_at_jelly_with_nothing_standing_on_it() {
        // Jelly is peeled by clearing the gem on top of it, so bare jelly has
        // nothing to clear and a strike there is a wasted rocket. Orphaned
        // jelly in a pocket nothing can refill is exactly where that would
        // happen, and it would happen every volley for the rest of the level.
        let mut game = targeting_game(186, vec![Objective::Jelly]);
        let bare = Pos::new(2, 2);
        let covered = Pos::new(4, 4);
        for p in [bare, covered] {
            game.board.cell_mut(p).expect("on the board").jelly = 1;
        }
        game.board.set_gem(bare, None);
        game.progress.jelly_total = 2;
        game.progress.jelly_left = 2;

        let seen = aim_spread(&mut game);
        assert!(!seen.contains(&bare), "it aimed at jelly with nothing standing on it");
        assert_eq!(seen, vec![covered], "the covered jelly is the only jelly worth shooting");
    }

    #[test]
    fn a_rocket_ignores_empty_cells_entirely() {
        // Nothing to hit is nothing to aim at, jelly or no jelly. A board that
        // has holes in it is ordinary now, so this is not a corner case.
        let mut game = targeting_game(187, vec![Objective::Score(10_000)]);
        let holes = [Pos::new(1, 1), Pos::new(3, 2), Pos::new(4, 0)];
        for p in holes {
            game.board.set_gem(p, None);
        }

        let seen = aim_spread(&mut game);
        for hole in holes {
            assert!(!seen.contains(&hole), "it aimed at the empty cell {hole:?}");
        }
        assert!(!seen.is_empty(), "and it still found the gems that are there");
    }

    #[test]
    fn a_score_target_alone_does_not_make_everything_worth_shooting() {
        // Every gem on the board advances a score target, so counting score as
        // an objective would put the whole board in the top tier and the tiers
        // would mean nothing. A plain gem it is.
        let mut game = targeting_game(181, vec![Objective::Score(10_000)]);
        game.board.set_gem(Pos::new(4, 4), Some(Gem { color: 3, special: Special::Cross }));

        let seen = aim_spread(&mut game);
        assert!(seen.len() > 5, "it should be drawing from the plain gems at large");
        assert!(!seen.contains(&Pos::new(4, 4)), "and not from the special while those last");
    }

    #[test]
    fn a_rocket_falls_back_to_a_special_only_when_nothing_else_is_left() {
        // The board is nothing but the rocket and one cross.
        let mut game = targeting_game(182, vec![Objective::Score(10_000)]);
        for p in game.board.positions().collect::<Vec<_>>() {
            if p != Pos::new(0, 0) {
                game.board.set_gem(p, None);
            }
        }
        game.board.set_gem(Pos::new(4, 4), Some(Gem { color: 3, special: Special::Cross }));

        assert_eq!(
            game.pick_targets(&[Pos::new(0, 0)]),
            vec![(Pos::new(0, 0), Pos::new(4, 4))],
            "with nothing plain left, the special is the target",
        );
    }

    #[test]
    fn a_strike_on_a_special_sets_it_off_instead_of_taking_it() {
        // A cross under the strike should clear its row and column, which is
        // far more than the one cell the rocket would have taken.
        let mut game = targeting_game(183, vec![Objective::Score(10_000)]);
        let cross = Pos::new(2, 2);
        game.board.set_gem(cross, Some(Gem { color: 3, special: Special::Cross }));
        let events = strike(&mut game, Pos::new(0, 0), cross);

        assert!(events.iter().any(|e| e.kind == EV_ROCKET_HIT), "it still lands as a strike");

        // Run on until the clear it set off has resolved.
        let after = settle(&mut game);
        let fired: Vec<&Event> = after
            .iter()
            .chain(events.iter())
            .filter(|e| e.kind == EV_SPECIAL_FIRED && e.special == Special::Cross.code())
            .collect();
        assert_eq!(fired.len(), 1, "the cross should have gone off");
    }

    #[test]
    fn a_rocket_can_pick_a_brick_out_of_the_board() {
        let mut game = bricked_game(167);
        for p in game.board.positions().collect::<Vec<_>>() {
            if game.board.brick(p) == 0 {
                game.board.set_gem(p, Some(Gem::plain((p.r + p.c) as u8 % 4)));
            }
        }
        let rocket = Pos::new(0, 0);
        let gem = game.board.gem(rocket).expect("just filled");
        game.board.set_gem(rocket, Some(Gem { special: Special::Rocket, ..gem }));

        let mut aimed_at_brick = false;
        for _ in 0..200 {
            for (_, to) in game.pick_targets(&[rocket]) {
                aimed_at_brick |= game.board.brick(to) > 0;
            }
        }
        assert!(aimed_at_brick, "a brick should be in the pool a rocket draws from");
    }

    #[test]
    fn a_rocket_strike_leaves_the_bricks_around_it_alone() {
        // A strike is not a blast: it takes the one cell it was aimed at. What
        // is standing beside that cell is none of its business, unlike a gem
        // going off in a match.
        let mut game = bricked_game(168);
        let beside = Pos::new(3, 1);
        game.board.set_gem(beside, Some(Gem::plain(1)));
        let events = strike(&mut game, Pos::new(0, 0), beside);

        assert!(
            events.iter().any(|e| e.kind == EV_ROCKET_HIT),
            "the rocket should have landed on the gem",
        );
        assert!(
            events.iter().all(|e| e.kind != EV_BRICK),
            "and the brick next to it should not have felt a thing",
        );
        assert_eq!(game.board.brick(Pos::new(3, 2)), 2, "still whole");
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

        while game.board.settle_stage(&game.spec.rules, &mut game.rng, 0).is_some() {}

        assert_eq!(game.board.brick(Pos::new(3, 2)), 2, "the brick stayed put");
        assert!(
            game.board.gem(Pos::new(3, 2)).is_none(),
            "and nothing fell into the cell it is holding",
        );
        // The gem stopped on the brick rather than passing through it, and
        // then slid off, which is what a gem resting on something does when
        // there is a gap beside it. Left first, so it ends up at (3,1).
        assert_eq!(game.board.gem(Pos::new(3, 1)).map(|g| g.color), Some(1));
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
        let shot = Launch {
            from,
            to,
            flight_ms: flight_time(cells_between(from, to)),
            landed: false,
            from_inventory: false,
        };
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
    fn moves_left_over_are_cashed_in_before_the_level_ends() {
        // The whole point of finishing early: the moves still in hand each
        // turn a gem into a special, and then the board goes off.
        let mut level = spec(9, 9, 6, 30);
        level.objectives = vec![Objective::Score(1)];
        let mut game = Game::new(level, 501);
        let (a, b) = game.hint().expect("a fresh board has a move");
        game.try_swap(a, b);
        let events = settle(&mut game);

        assert_eq!(game.status(), Status::Won);
        assert_eq!(game.moves_left, 0, "the moves left over should have been spent");
        // Exactly one spend per move left over.
        let spent = events.iter().filter(|e| e.kind == EV_CASH_IN).count();
        assert_eq!(spent, 29, "a 29 move surplus was spent {spent} times");

        let cleared = events.iter().filter(|e| e.kind == EV_CLEARED).count();
        assert_eq!(cleared, 1, "the level said it was cleared {cleared} times");
        assert!(
            events.iter().position(|e| e.kind == EV_CLEARED)
                < events.iter().position(|e| e.kind == EV_CASH_IN),
            "it should say so before spending anything, not after",
        );
        assert!(
            events.iter().position(|e| e.kind == EV_CLEARED)
                < events.iter().position(|e| e.kind == EV_WON),
            "the level ended before it was announced as cleared",
        );
    }

    #[test]
    fn a_beaten_level_holds_still_while_its_goals_finish_showing() {
        // The clear that wins a level is still crossing the screen toward the
        // goals it counted for, and watching them reach their totals is the
        // payoff. The flourish waits that out rather than starting over the
        // top of it.
        //
        // How long is the page's number, because the page owns the animation:
        // told nothing, a board does not hold at all, which is what every
        // other test here relies on.
        let mut level = spec(9, 9, 6, 30);
        level.objectives = vec![Objective::Score(1)];

        let mut bare = Game::new(level.clone(), 513);
        let (a, b) = bare.hint().expect("a fresh board has a move");
        bare.try_swap(a, b);
        let mut held_bare = 0;
        for _ in 0..4000 {
            if bare.phase() == Phase::Idle || bare.phase() == Phase::Finished {
                break;
            }
            if matches!(bare.phase(), Phase::Tallying { .. }) {
                held_bare += 1;
            }
            bare.update(16.0);
        }
        assert_eq!(held_bare, 0, "a board told nothing about a page held anyway");

        let mut game = Game::new(level, 513);
        game.goal_hold_ms = 800.0;
        let (a, b) = game.hint().expect("a fresh board has a move");
        game.try_swap(a, b);

        let mut held = 0;
        let mut spent_while_holding = 0;
        let mut seen = Vec::new();
        for _ in 0..4000 {
            if game.phase() == Phase::Idle || game.phase() == Phase::Finished {
                break;
            }
            if matches!(game.phase(), Phase::Tallying { .. }) {
                held += 1;
                spent_while_holding +=
                    game.events().iter().filter(|e| e.kind == EV_CASH_IN).count();
            }
            game.update(16.0);
            seen.extend_from_slice(game.events());
        }

        // 800ms at 16ms a frame, give or take the frame it starts on.
        assert!((48..=51).contains(&held), "the board held for {held} frames rather than 50");
        assert_eq!(spent_while_holding, 0, "the flourish started over the top of the hold");
        assert_eq!(game.status(), Status::Won, "holding lost the level");
        assert_eq!(game.moves_left, 0, "holding swallowed the flourish");

        // Once a level, not once per settle: the board comes to rest between
        // every round of the flourish, and each of those would hold again.
        let rounds = seen.iter().filter(|e| e.kind == EV_CASH_IN).count();
        assert!(rounds > 1, "this seed spends nothing, so nothing here is being tested");
        assert!(
            held < 60,
            "the board held {held} frames, which is more than one beat's worth",
        );
    }

    /// Steps until the leftover moves start being spent, and says how many
    /// frames that took.
    fn run_to_cash_in(game: &mut Game) -> u32 {
        let mut frames = 0;
        while !matches!(game.phase(), Phase::CashingIn { .. }) && frames < 400 {
            game.update(16.0);
            frames += 1;
        }
        assert!(
            matches!(game.phase(), Phase::CashingIn { .. }),
            "the level never started spending its leftover moves",
        );
        frames
    }

    /// Every leftover move spent, as the special it left behind, which is 0
    /// for a move spent with nothing to place.
    fn cash_in_placements(game: &mut Game) -> Vec<u8> {
        let mut frames = run_to_cash_in(game);
        let mut placed = Vec::new();
        while matches!(game.phase(), Phase::CashingIn { .. }) && frames < 4000 {
            game.update(16.0);
            frames += 1;
            placed
                .extend(game.events().iter().filter(|e| e.kind == EV_CASH_IN).map(|e| e.special));
        }
        placed
    }

    #[test]
    fn the_leftover_moves_are_spent_one_at_a_time() {
        // Watchable, not instant: the counter running down is the whole of
        // what tells the player their spare moves were worth something.
        let mut level = spec(9, 9, 6, 30);
        level.objectives = vec![Objective::Score(1)];
        let mut game = Game::new(level, 506);
        let (a, b) = game.hint().expect("a fresh board has a move");
        game.try_swap(a, b);

        let mut frames = run_to_cash_in(&mut game);
        assert_eq!(game.moves_left, 29, "nothing should be spent before the run down starts");

        let mut prev = game.moves_left;
        let mut steps = 0;
        while matches!(game.phase(), Phase::CashingIn { .. }) && frames < 4000 {
            game.update(16.0);
            frames += 1;
            if game.moves_left != prev {
                assert_eq!(game.moves_left, prev - 1, "the count jumped rather than stepping");
                prev = game.moves_left;
                steps += 1;
            }
        }
        assert_eq!(steps, 29, "29 moves should take 29 steps to spend");
        assert_eq!(game.moves_left, 0);
        // 29 steps at 300ms each, and a frame here is 16ms.
        assert!(frames > 500, "the whole run down took only {frames} frames");
    }

    #[test]
    fn the_flourish_is_one_chain_of_its_own() {
        // It starts over once, where the player's last chain ended and the
        // flourish begins, so the music opens at the bottom of its
        // progression. From there it climbs through every round: nobody moves
        // between them, and restarting on each would drop the music back to
        // that first chord over and over.
        //
        // The seed is picked for taking three rounds to finish. Most take
        // one, and on those this passes whatever the rule is, which is how an
        // earlier version of it sat here proving nothing.
        let mut level = spec(9, 9, 6, 30);
        level.objectives = vec![Objective::Score(1)];
        let mut game = Game::new(level, 513);
        let (a, b) = game.hint().expect("a fresh board has a move");
        game.try_swap(a, b);
        let events = settle(&mut game);

        let at_clear = events
            .iter()
            .position(|e| e.kind == EV_CLEARED)
            .expect("the level never said it was cleared");
        let flourish: Vec<u16> = events[at_clear..]
            .iter()
            .filter(|e| e.kind == EV_MATCH)
            .map(|e| e.value)
            .collect();

        // Enough links to span the rounds, or the checks below say nothing.
        assert!(flourish.len() >= 8, "the flourish resolved only {} matches", flourish.len());
        assert_eq!(
            flourish.first().copied(),
            Some(1),
            "the flourish should open at the bottom of the progression: {flourish:?}",
        );
        assert!(
            flourish.windows(2).all(|pair| pair[1] > pair[0]),
            "and climb from there without starting over: {flourish:?}",
        );
    }

    #[test]
    fn a_won_board_is_looked_at_before_the_level_is_declared_over() {
        // Without the beat, the last thing a player sees is a panel sliding
        // over the board they just cleared.
        let mut level = spec(4, 4, 6, 1);
        level.objectives = vec![Objective::Score(1)];
        let mut game = Game::new(level, 507);
        let (a, b) = game.hint().expect("a fresh board has a move");
        game.try_swap(a, b);

        let mut frames = 0;
        while !matches!(game.phase(), Phase::Finishing { .. }) && frames < 800 {
            game.update(16.0);
            frames += 1;
        }
        assert!(matches!(game.phase(), Phase::Finishing { .. }), "the level never held");
        assert_eq!(game.status(), Status::Playing, "it declared the win before the beat");

        let mut held = 0;
        while matches!(game.phase(), Phase::Finishing { .. }) && held < 400 {
            game.update(16.0);
            held += 1;
        }
        assert_eq!(game.status(), Status::Won);
        assert!(held > 40, "the beat lasted only {held} frames");
        assert!(held < 100, "the beat lasted {held} frames, which is a wait rather than a beat");
    }

    /// A board with an Archipelago gem in the middle of it and nothing else
    /// placed, so the gems around it can be arranged per test.
    fn ap_board(game: &mut Game, at: Pos) {
        for p in game.board.positions().collect::<Vec<_>>() {
            game.board.set_gem(p, Some(Gem::plain(((p.r * 3 + p.c * 7) % 4) as u8)));
        }
        game.board.set_gem(at, Some(Gem::archipelago()));
    }

    #[test]
    fn an_archipelago_gem_never_matches() {
        // It has no color, and every one of them carries the same absence of
        // one. Left matchable, three in a row would match each other.
        let mut game = Game::new(spec(6, 6, 4, 20), 9);
        for c in 0..6 {
            game.board.set_gem(Pos::new(2, c), Some(Gem::archipelago()));
        }
        assert!(
            matching::find_matches(&game.board, &game.spec.rules)
                .iter()
                .all(|group| group.cells.iter().all(|p| p.r != 2)),
            "a row of Archipelago gems matched itself",
        );
        assert_eq!(game.board.match_color(Pos::new(2, 0)), None);
    }

    #[test]
    fn a_rocket_struck_archipelago_gem_is_collected_rather_than_destroyed() {
        // A check is the best thing on the board, and a rocket is a thing that
        // reaches across it. Striking one has to collect it: taking the gem
        // away without raising the collection is the check gone for good, and
        // nothing on the level would ever say so.
        let mut game = Game::new(spec(6, 6, 4, 20), 9);
        let at = Pos::new(2, 3);
        ap_board(&mut game, at);
        game.ap_gems_wanted = 1;

        let (spent, seen) = spend(&mut game, Consumable::Rocket, Some(at));
        assert!(spent, "a rocket aimed at an Archipelago gem was refused");
        assert!(
            seen.iter().any(|e| e.kind == EV_ROCKET_HIT && (e.r, e.c) == (at.r as u8, at.c as u8)),
            "the rocket never reached it",
        );
        assert_eq!(
            seen.iter().filter(|e| e.kind == EV_AP_CLEAR).count(),
            1,
            "the gem was taken off the board without the check being collected",
        );
        // And the level knows it has one fewer worth dropping, or the next
        // refill sends another down for a check that is no longer there.
        assert_eq!(game.ap_gems_wanted, 0, "the level still wants a gem it has already given");
    }

    #[test]
    fn a_rocket_aims_at_an_archipelago_gem_as_readily_as_at_a_brick() {
        // Both are the best thing a rocket can be pointed at, so both go in
        // the top tier. A check sitting among plain gems used to read as one
        // of them, which meant a rocket took it only by accident and only
        // once everything that moved a goal along was gone.
        let mut game = Game::new(spec(6, 6, 4, 20), 9);
        let at = Pos::new(2, 3);
        ap_board(&mut game, at);

        // No goal is outstanding on this board and nothing is bricked, so the
        // top tier holds the check and nothing else: every rocket must take
        // it. Twenty runs, because one rocket landing on it proves nothing
        // when the board is mostly plain gems.
        let mut taken = 0;
        for seed in 0..20 {
            let mut game = Game::new(spec(6, 6, 4, 20), seed);
            ap_board(&mut game, at);
            let from = Pos::new(5, 0);
            let picked = game.pick_targets(&[from]);
            taken += usize::from(picked.first().map(|(_, to)| *to) == Some(at));
        }
        assert_eq!(taken, 20, "a rocket passed over the check {} times in 20", 20 - taken);
        // And the tiers still hold: it is chosen over plain gems, not because
        // it is the only thing there.
        assert!(game.board.gem(at).is_some(), "the board lost the gem before anything was aimed");
    }

    #[test]
    fn an_archipelago_gem_swaps_with_an_ordinary_one_to_set_up_a_match() {
        // It cannot match, but it can be moved, and moving it is how a gem
        // stuck behind one gets where it is going. Two in a row with the third
        // of their color below the gem: swapping the two of them completes the
        // row, and the clear happens beside where the gem has just landed, so
        // it is collected as well.
        let mut game = Game::new(spec(6, 6, 4, 20), 9);
        ap_board(&mut game, Pos::new(0, 2));
        game.board.set_gem(Pos::new(0, 0), Some(Gem::plain(1)));
        game.board.set_gem(Pos::new(0, 1), Some(Gem::plain(1)));
        game.board.set_gem(Pos::new(1, 2), Some(Gem::plain(1)));
        let (ap, gem) = (Pos::new(0, 2), Pos::new(1, 2));

        assert!(
            matching::is_useful_swap(&game.board, &game.spec.rules, ap, gem),
            "the hint does not see the move",
        );
        assert!(game.try_swap(ap, gem), "the swap was refused");
        let events = settle(&mut game);
        assert!(
            events.iter().any(|e| e.kind == EV_CLEAR && e.color == 1),
            "the row it completed never cleared",
        );
        assert_eq!(
            events.iter().filter(|e| e.kind == EV_AP_CLEAR).count(),
            1,
            "the clear happened right beside it and did not collect it",
        );
    }

    #[test]
    fn swapping_one_never_makes_a_match_of_the_gem_itself() {
        // Every Archipelago gem carries the same absence of a color, so a
        // swap judged on the raw color byte would read three of them in a row
        // as a run and offer a move that matches nothing.
        let mut game = Game::new(spec(6, 6, 4, 20), 9);
        // A board with no run of three in it and none a step away: two colors
        // alternating along each row, and every row a different pair.
        for p in game.board.positions().collect::<Vec<_>>() {
            game.board.set_gem(p, Some(Gem::plain(((p.r + 2 * p.c) % 4) as u8)));
        }
        assert!(
            matching::find_matches(&game.board, &game.spec.rules).is_empty(),
            "the board this is built on already matches, so it proves nothing",
        );

        // Two of them in a row, and a third one swap below the gap.
        game.board.set_gem(Pos::new(2, 0), Some(Gem::archipelago()));
        game.board.set_gem(Pos::new(2, 1), Some(Gem::archipelago()));
        game.board.set_gem(Pos::new(3, 2), Some(Gem::archipelago()));

        assert!(
            !matching::is_useful_swap(
                &game.board,
                &game.spec.rules,
                Pos::new(2, 2),
                Pos::new(3, 2),
            ),
            "three Archipelago gems in a row were offered as a match",
        );
    }

    #[test]
    fn a_rainbow_swapped_against_one_takes_every_one_on_the_board() {
        let mut game = Game::new(spec(6, 6, 4, 20), 9);
        ap_board(&mut game, Pos::new(2, 2));
        for p in [Pos::new(0, 0), Pos::new(5, 5), Pos::new(4, 1)] {
            game.board.set_gem(p, Some(Gem::archipelago()));
        }
        let rainbow = Pos::new(2, 1);
        game.board.set_gem(rainbow, Some(Gem { color: 0, special: Special::Rainbow }));

        assert!(
            matching::is_useful_swap(&game.board, &game.spec.rules, rainbow, Pos::new(2, 2)),
            "a rainbow beside one is not offered as a move",
        );
        assert!(game.try_swap(rainbow, Pos::new(2, 2)), "the rainbow would not swap with it");
        let events = settle(&mut game);
        assert_eq!(
            events.iter().filter(|e| e.kind == EV_AP_CLEAR).count(),
            4,
            "a rainbow should take every one on the board, not just the one it touched",
        );
        assert!(game.board.ap_gems().is_empty(), "one was left behind");
    }

    #[test]
    fn two_of_them_swapped_together_take_every_one_on_the_board() {
        let mut game = Game::new(spec(6, 6, 4, 20), 9);
        ap_board(&mut game, Pos::new(2, 2));
        for p in [Pos::new(2, 3), Pos::new(5, 5)] {
            game.board.set_gem(p, Some(Gem::archipelago()));
        }
        assert!(
            matching::is_useful_swap(&game.board, &game.spec.rules, Pos::new(2, 2), Pos::new(2, 3)),
            "two of them side by side is not offered as a move",
        );
        assert!(game.try_swap(Pos::new(2, 2), Pos::new(2, 3)));
        let events = settle(&mut game);
        assert_eq!(events.iter().filter(|e| e.kind == EV_AP_CLEAR).count(), 3);
        assert!(game.board.ap_gems().is_empty());
    }

    #[test]
    fn clearing_beside_one_collects_it() {
        // The ordinary way to take one: anything going off in the four cells
        // around it, the same reach that breaks a brick.
        let mut game = Game::new(spec(6, 6, 4, 20), 9);
        ap_board(&mut game, Pos::new(0, 0));
        // A row of three under it, made by swapping the third into line.
        for c in 0..3 {
            game.board.set_gem(Pos::new(1, c), Some(Gem::plain(1)));
        }
        game.board.set_gem(Pos::new(1, 2), Some(Gem::plain(2)));
        game.board.set_gem(Pos::new(2, 2), Some(Gem::plain(1)));
        assert!(game.try_swap(Pos::new(1, 2), Pos::new(2, 2)));

        let events = settle(&mut game);
        assert_eq!(
            events.iter().filter(|e| e.kind == EV_AP_CLEAR).count(),
            1,
            "a match beside it did not collect it",
        );
        assert!(!game.board.is_ap_gem(Pos::new(0, 0)), "it stayed on the board");
    }

    #[test]
    fn a_match_that_never_reaches_one_leaves_it_alone() {
        // The other half of the rule above: it is collected by being cleared
        // against, not by a clear happening somewhere on the same board.
        let mut game = Game::new(spec(6, 6, 4, 20), 9);
        ap_board(&mut game, Pos::new(0, 0));
        for c in 3..6 {
            game.board.set_gem(Pos::new(5, c), Some(Gem::plain(1)));
        }
        game.board.set_gem(Pos::new(5, 5), Some(Gem::plain(2)));
        game.board.set_gem(Pos::new(4, 5), Some(Gem::plain(1)));
        assert!(game.try_swap(Pos::new(5, 5), Pos::new(4, 5)));

        let events = settle(&mut game);
        assert_eq!(
            events.iter().filter(|e| e.kind == EV_AP_CLEAR).count(),
            0,
            "a match across the board collected it",
        );
    }

    #[test]
    fn an_archipelago_gem_falls_like_any_other_gem() {
        // Unlike a brick, which holds its cell. This is why it lives on the
        // gem rather than on the cell.
        let mut game = Game::new(spec(6, 6, 4, 20), 9);
        ap_board(&mut game, Pos::new(0, 3));
        game.board.set_gem(Pos::new(1, 3), None);
        game.board.set_gem(Pos::new(2, 3), None);

        game.begin_fall();
        for _ in 0..200 {
            game.update(16.0);
            if game.phase() == Phase::Idle {
                break;
            }
        }
        assert!(!game.board.is_ap_gem(Pos::new(0, 3)), "it stayed up where it was drawn");
        assert_eq!(
            game.board.ap_gems().len(),
            1,
            "it should have fallen, not vanished or multiplied",
        );
    }

    #[test]
    fn the_opening_deal_never_places_one() {
        // They fall in during play and only then. A board that opened with
        // one would hand over a check before the player had done anything,
        // and a level drawn with a layout would be deciding where checks go.
        //
        // At odds far denser than the yaml offers and with plenty wanted, so
        // this cannot pass by one merely not happening to be drawn: if the
        // deal went through the same path the refill does, every one of these
        // boards would open covered in them.
        for seed in 0..40 {
            let mut level = spec(8, 8, 6, 20);
            level.rules.ap_gem_odds = 2;
            let mut game = Game::new(level, seed);
            game.ap_gems_wanted = 99;
            assert!(
                game.board.ap_gems().is_empty(),
                "seed {seed} dealt an Archipelago gem onto a fresh board",
            );

            // A reshuffle deals a fresh board too, and it is still not play.
            game.shuffle_board();
            assert!(
                game.board.ap_gems().is_empty(),
                "seed {seed} put one on the board when it reshuffled",
            );
        }
    }

    #[test]
    fn a_lost_board_is_looked_at_before_the_level_is_declared_over() {
        // The same beat a win gets. Without it the panel lands on the frame
        // the last gem does, so the move that ran the counter out is never
        // seen to finish and the board is snatched away mid-fall.
        //
        // The default objective is a score nothing here will reach, so one
        // move is one loss.
        let mut game = Game::new(spec(4, 4, 6, 1), 507);
        let (a, b) = game.hint().expect("a fresh board has a move");
        game.try_swap(a, b);

        let mut frames = 0;
        let mut lost_early = false;
        while !matches!(game.phase(), Phase::Finishing { .. }) && frames < 800 {
            game.update(16.0);
            lost_early |= game.events().iter().any(|e| e.kind == EV_LOST);
            frames += 1;
        }
        assert!(matches!(game.phase(), Phase::Finishing { .. }), "the level never held");
        assert_eq!(game.status(), Status::Playing, "it declared the loss before the beat");
        assert!(!lost_early, "it announced the loss before the beat");

        let mut held = 0;
        while matches!(game.phase(), Phase::Finishing { .. }) && held < 400 {
            game.update(16.0);
            held += 1;
        }
        assert_eq!(game.status(), Status::Lost);
        assert!(
            game.events().iter().any(|e| e.kind == EV_LOST),
            "the beat ended without the loss being announced",
        );
        assert!(held > 40, "the beat lasted only {held} frames");
        assert!(held < 100, "the beat lasted {held} frames, which is a wait rather than a beat");
    }

    #[test]
    fn how_briskly_a_level_was_beaten_outlives_the_flourish() {
        // The flourish spends every move that was left, so afterwards the
        // counter always reads zero. Without this the difference between
        // beating a level on the first move and scraping it on the last would
        // be gone, and that difference is what a score tier is for.
        let mut level = spec(9, 9, 6, 30);
        level.objectives = vec![Objective::Score(1)];
        let mut game = Game::new(level, 505);
        let (a, b) = game.hint().expect("a fresh board has a move");
        game.try_swap(a, b);
        let _ = settle(&mut game);

        assert_eq!(game.moves_left, 0, "the flourish should have spent what was left");
        assert_eq!(
            game.progress.moves_spare, 29,
            "and the record of what there was should have survived it",
        );
    }

    #[test]
    fn finishing_with_moves_to_spare_scores_better_than_finishing_without() {
        // The incentive this exists to create. Same board, same objective;
        // the only difference is how much was left when it was met.
        let score_with = |spare: u32| {
            let mut level = spec(9, 9, 6, 30);
            level.objectives = vec![Objective::Score(1)];
            let mut game = Game::new(level, 502);
            game.moves_left = spare;
            let (a, b) = game.hint().expect("a fresh board has a move");
            game.try_swap(a, b);
            let _ = settle(&mut game);
            assert_eq!(game.status(), Status::Won);
            game.progress.score
        };
        assert!(
            score_with(25) > score_with(2),
            "keeping moves back was worth nothing",
        );
    }

    #[test]
    fn a_run_with_nothing_eligible_still_goes_through_the_motions() {
        // The board does not change, but the level still says it was cleared
        // and the counter still runs down at the same pace. A count that
        // vanished instead would read as the moves being taken away.
        let mut level = spec(9, 9, 6, 30);
        level.objectives = vec![Objective::Score(1)];
        level.rules.specials = SpecialSet::NONE;
        let mut game = Game::new(level, 503);
        let (a, b) = game.hint().expect("a fresh board has a move");
        game.try_swap(a, b);

        let placed = cash_in_placements(&mut game);
        assert_eq!(placed.len(), 29, "the moves were not all spent");
        assert!(
            placed.iter().all(|code| *code == Special::None.code()),
            "something was placed by a run that holds nothing: {placed:?}",
        );
        // Each one still lands somewhere, so the board can be seen and heard
        // flashing at the cell that missed out.
        let mut frames = 0;
        while game.phase() != Phase::Finished && frames < 400 {
            game.update(16.0);
            frames += 1;
        }
        assert_eq!(game.status(), Status::Won);
        assert_eq!(game.moves_left, 0);
    }

    #[test]
    fn a_move_spent_with_nothing_to_place_still_says_where_it_landed() {
        // The cell is what the sparkle and the bell hang off, so a spend that
        // placed nothing has to name one anyway or it happens invisibly.
        let mut level = spec(9, 9, 6, 30);
        level.objectives = vec![Objective::Score(1)];
        level.rules.specials = SpecialSet::NONE;
        let mut game = Game::new(level, 510);
        let (a, b) = game.hint().expect("a fresh board has a move");
        game.try_swap(a, b);

        let mut frames = run_to_cash_in(&mut game);
        let mut cells: Vec<(u8, u8)> = Vec::new();
        while matches!(game.phase(), Phase::CashingIn { .. }) && frames < 4000 {
            game.update(16.0);
            frames += 1;
            cells.extend(
                game.events().iter().filter(|e| e.kind == EV_CASH_IN).map(|e| (e.r, e.c)),
            );
        }
        assert_eq!(cells.len(), 29, "not every spend named a cell");
        assert!(
            cells.iter().all(|(r, c)| *r < 9 && *c < 9),
            "a spend landed off the board: {cells:?}",
        );
        assert!(
            cells.iter().collect::<std::collections::HashSet<_>>().len() > 10,
            "they all landed on the same handful of cells: {cells:?}",
        );
    }

    #[test]
    fn only_the_line_and_cross_clearers_can_be_minted() {
        // A boardful of rainbows takes every color off at once and a boardful
        // of rockets fires at somewhere else, so neither is a board coming
        // apart in front of you. The three that draw a line are.
        let mut level = spec(9, 9, 6, 30);
        level.objectives = vec![Objective::Score(1)];
        let mut game = Game::new(level, 504);
        assert_eq!(game.spec.rules.specials, SpecialSet::ALL, "the level offers all five");
        let (a, b) = game.hint().expect("a fresh board has a move");
        game.try_swap(a, b);

        let made = cash_in_placements(&mut game);
        assert!(!made.is_empty(), "nothing was minted at all");
        let eligible: Vec<u8> = CASH_IN_SPECIALS.iter().map(|s| s.code()).collect();
        assert!(
            made.iter().all(|code| eligible.contains(code)),
            "the run down minted something ineligible: {made:?}",
        );
    }

    #[test]
    fn the_run_down_only_mints_what_the_run_may_make() {
        // Eligible is not the same as earned: it can only hand out what the
        // level, and so the run, allows.
        let mut level = spec(9, 9, 6, 30);
        level.objectives = vec![Objective::Score(1)];
        level.rules.specials = SpecialSet { line_h: true, ..SpecialSet::NONE };
        let mut game = Game::new(level, 508);
        let (a, b) = game.hint().expect("a fresh board has a move");
        game.try_swap(a, b);

        let made = cash_in_placements(&mut game);
        assert!(!made.is_empty(), "nothing was minted at all");
        assert!(
            made.iter().all(|code| *code == Special::LineH.code()),
            "it handed out something the run had not earned: {made:?}",
        );
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
    fn a_layout_can_place_gems_and_the_deal_fills_around_them() {
        // What a placed gem is for: an opening arrangement the level is
        // designed around. The deal has to leave them where they are, or the
        // arrangement is only whatever the seed felt like.
        let mut level = spec(4, 4, 6, 10);
        level.layout = Some(&["0.1.", "....", "....", "2.3."]);
        for seed in 0..20 {
            let game = Game::new(level.clone(), seed);
            assert_eq!(game.board.gem(Pos::new(0, 0)).map(|g| g.color), Some(0));
            assert_eq!(game.board.gem(Pos::new(0, 2)).map(|g| g.color), Some(1));
            assert_eq!(game.board.gem(Pos::new(3, 0)).map(|g| g.color), Some(2));
            assert_eq!(game.board.gem(Pos::new(3, 2)).map(|g| g.color), Some(3));
            assert!(
                game.board.positions().all(|p| game.board.gem(p).is_some()),
                "the deal left a hole around the placed gems",
            );
        }
    }

    #[test]
    fn a_placed_gem_is_the_opening_board_and_not_the_cell() {
        // Once its cell is cleared it refills like any other. A cell that kept
        // dealing one color would be a different thing entirely, and this is
        // not that.
        let mut level = spec(4, 4, 6, 10);
        level.layout = Some(&["0000", "....", "....", "...."]);
        let mut game = Game::new(level, 7);
        for p in game.board.occupied() {
            game.board.set_gem(p, None);
        }
        let rules = game.spec.rules.clone();
        for _ in 0..16 {
            if game.board.settle_stage(&rules, &mut game.rng, 0).is_none() {
                break;
            }
        }
        assert!(
            game.board.positions().all(|p| game.board.gem(p).is_some()),
            "the board did not refill",
        );
        let top: Vec<u8> =
            (0..4).filter_map(|c| game.board.gem(Pos::new(0, c)).map(|g| g.color)).collect();
        assert!(
            top.iter().any(|color| *color != 0),
            "the placed row dealt itself again, so it is a property of the cells",
        );
    }

    #[test]
    fn a_board_is_never_dealt_without_a_move_on_it() {
        // The one failure a deal may not accept. An opening match resolves
        // itself and the level carries on; a board with nothing to do is not
        // a game, and the player has no way out of it.
        for seed in 0..200 {
            let game = Game::new(spec(6, 6, 4, 20), seed);
            assert!(
                matching::find_move(&game.board, game.rules()).is_some(),
                "seed {seed} opened with no legal move",
            );
        }
    }

    #[test]
    fn softening_a_double_jelly_does_not_move_the_counter() {
        // What a level asks for is that every jellied cell be cleared, so the
        // counter says how many are left to clear rather than how many hits
        // are left to land. Peeling a double layer down to one has finished
        // nothing, and the board is where that work shows.
        let mut level = spec(4, 4, 6, 10);
        level.layout = Some(&["OOOO", "OOOO", "OOOO", "OOOO"]);
        level.objectives = vec![Objective::Jelly];
        let mut game = Game::new(level, 23);
        assert_eq!(game.progress.jelly_total, 16, "sixteen cells, however deep they are");

        paint(&mut game, &["1200", "1300", "2100", "0000"]);
        game.try_swap(Pos::new(2, 0), Pos::new(2, 1));
        let _ = settle(&mut game);
        assert_eq!(game.progress.jelly_left, 16, "softening a layer moved the counter");

        // Down to a single layer everywhere, so the same move finishes cells
        // rather than softening them.
        let everywhere: Vec<Pos> = game.board.positions().collect();
        for p in everywhere {
            game.board.peel_jelly(p);
        }
        paint(&mut game, &["1200", "1300", "2100", "0000"]);
        game.try_swap(Pos::new(2, 0), Pos::new(2, 1));
        let _ = settle(&mut game);
        assert!(
            game.progress.jelly_left < 16,
            "clearing the last layer off a cell left it on the counter",
        );
    }

    #[test]
    fn cracking_a_brick_does_not_move_the_counter() {
        // The same rule for the blockers that take two hits.
        let mut level = spec(4, 5, 8, 10);
        level.rules.refill = RefillMode::None;
        level.layout = Some(&[".....", ".....", ".....", "..=.."]);
        level.objectives = vec![Objective::Brick];
        let mut game = Game::new(level, 160);
        assert_eq!(game.progress.brick_total, 1, "one cell holds a brick");

        clear_beside_the_blocker(&mut game, 1);
        assert_eq!(game.board.brick(Pos::new(3, 2)), 1, "the hit should have cracked it");
        assert_eq!(game.progress.brick_left, 1, "cracking it moved the counter");

        // And again from cracked, which is the hit that finishes it.
        let mut level = spec(4, 5, 8, 10);
        level.rules.refill = RefillMode::None;
        level.layout = Some(&[".....", ".....", ".....", "..=.."]);
        level.objectives = vec![Objective::Brick];
        let mut game = Game::new(level, 161);
        game.board.damage_brick(Pos::new(3, 2));
        clear_beside_the_blocker(&mut game, 1);
        assert_eq!(game.board.brick(Pos::new(3, 2)), 0, "the second hit should take it");
        assert_eq!(game.progress.brick_left, 0, "breaking it did not move the counter");
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
