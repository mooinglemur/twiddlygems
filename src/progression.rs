//! What a run has been given, and where it found it.
//!
//! This is the seam the Archipelago layer slots into. A run receives items;
//! the items decide what the player may do. Nothing here knows the difference
//! between solo and multiworld, because there is only one difference: who
//! filled the locations. Solo fills its own, from its own seed, by the same
//! reachability walk a generator does; Archipelago will have the server fill
//! them. Both walk the same [`Inventory`] through the same
//! [`apply`](Inventory::apply).
//!
//! Keeping both on one path is the point. The rule the multiworld has to hold
//! to, that a level is only ever placed somewhere the run can already clear
//! it, is a claim about this module, so it can be checked here rather than
//! rediscovered on the Python side.

use crate::board::Special;
use crate::level::LevelSpec;
use crate::options::{Goal, Options, GOAL as GOAL_SETTING};
use crate::rng::Rng;
use crate::rules::SpecialSet;

/// Splits the fill's randomness off the run seed.
///
/// A solo run generates its own progression, the way a multiworld does: the
/// run seed decides where the items went, so no two runs are the same game and
/// a returning one is handed back exactly what it found. Keeping the fill on
/// its own stream rather than drawing it alongside the boards means changing
/// how boards are dealt can never quietly reshuffle a saved run's items.
const FILL_SALT: u64 = 0x7477_6964_646c_7967;

/// What a run's placement is dealt from, given the seed the run itself was
/// dealt from. See [`solo_placement`].
pub fn fill_seed(run: u64) -> u64 {
    run ^ FILL_SALT
}

/// Something a run can receive.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Item {
    /// Matches may create this special again.
    ///
    /// Without it the board still matches and still clears; it simply leaves
    /// nothing behind. That is the state every run starts in.
    Unlock(Special),
    /// More room on one named level.
    ///
    /// Progressive: several can land on the same level and each adds the step
    /// again. Never needed to clear a level, because every level has to be
    /// beatable on its own budget or the ladder dead-ends. What they buy is a
    /// better run at one: more moves is more score.
    Moves { level: usize },
    /// Nothing at all, for a location with nothing better in it.
    ///
    /// There are more places to look than things to find, which is the shape
    /// that leaves room for another world's items in a multiworld. What goes
    /// in the leftovers on this side has to be something, and until there are
    /// traps and consumables to put there, this is the honest something.
    ///
    /// It was a spare moves upgrade before, which stopped meaning anything the
    /// moment an upgrade started landing whole and once: a second copy changed
    /// nothing, so a run could clear a level, be handed an item it already had
    /// all of, and be none the wiser. Better a name that says so.
    Filler,
}

impl Item {
    /// A stable number for this item, the way [`Location::id`] is one for a
    /// location.
    ///
    /// What Archipelago calls it by in a datapackage, which is the same kind
    /// of promise: a number that moves once a seed has been rolled hands the
    /// player somebody else's item. Each kind gets its own thousand so a new
    /// kind of item, a trap say, cannot renumber the ones already out there.
    /// An unlock takes the special's own code, which is already fixed.
    pub fn id(self) -> u32 {
        match self {
            Item::Unlock(special) => special.code() as u32,
            Item::Moves { level } => MOVES_ID_BASE + level as u32,
            Item::Filler => FILLER_ID,
        }
    }

    /// Which sort of item this is, for the event that announces it. See
    /// [`crate::game::EV_ITEM`].
    pub fn kind(self) -> u8 {
        match self {
            Item::Unlock(_) => 0,
            Item::Moves { .. } => 1,
            Item::Filler => 2,
        }
    }

    /// The item's one parameter, alongside its kind.
    pub fn value(self) -> u16 {
        match self {
            Item::Unlock(special) => special.code() as u16,
            Item::Moves { level } => level as u16,
            // Nothing to say about it; there is only the one.
            Item::Filler => 0,
        }
    }
}

/// Somewhere an item is found.
///
/// The set a multiworld would shuffle items across, and the set a solo run
/// fills from the table below. Checking one is what hands its item over,
/// whichever side is playing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Location {
    /// Clearing the level at this index.
    LevelClear(usize),
    /// Clearing it with a score past its silver mark.
    LevelSilver(usize),
    /// Clearing it with a score past its gold mark.
    LevelGold(usize),
    /// Reaching a chain this long: one clear setting off the next, that many
    /// deep, off a single move.
    Chain(u32),
    /// Lining up exactly this many gems in one move.
    ///
    /// Exactly, so a five does not pay the four: each of these asks for a
    /// particular shape rather than for "at least this good", and a player
    /// going for the six has to aim for it rather than grow into it.
    ///
    /// The player's own swap and nothing else. What the match sets off is not
    /// part of it, and neither is a match a cascade lines up afterwards: the
    /// number is what the player did, not what the board did next.
    ///
    /// Counted in gems that lined up, which is what a match is made of: a
    /// plain gem and one carrying a beam both count, while a rocket, a
    /// rainbow and an Archipelago gem answer to no color and are never in a
    /// match at all. Bricks and seals hold no gem; they are broken beside a
    /// match rather than being part of one.
    Match(u32),
    /// Collecting an Archipelago gem on the level at `level`.
    ///
    /// `index` is a place in that level's sequence rather than a particular
    /// gem: clearing one checks the lowest of the level's gems not yet
    /// checked, so the first ever collected on level three is `index` 0 and
    /// the next is 1, whichever playthrough each happened on. A level holds
    /// [`AP_GEMS_PER_LEVEL`] of these however few are in play, because the
    /// numbers and names are a datapackage and a datapackage is fixed; which
    /// of them a run actually has is [`ap_gems_per_level`].
    ApGem { level: usize, index: u32 },
}

/// How well a level has been beaten, at best.
///
/// Read back from the locations a run has checked rather than recorded
/// separately, so there is one account of what has happened and no way for the
/// two to disagree.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Tier {
    /// Never cleared.
    None,
    Clear,
    Silver,
    Gold,
}

impl Tier {
    pub fn code(self) -> u32 {
        match self {
            Tier::None => 0,
            Tier::Clear => 1,
            Tier::Silver => 2,
            Tier::Gold => 3,
        }
    }
}

/// The shortest chain worth asking for. A single clear is not a chain.
pub const SHORTEST_CHAIN: u32 = 2;
/// The longest chain asked for. Past this a board cannot be relied on to
/// produce one, so a location there would be a location nobody can check.
pub const LONGEST_CHAIN: u32 = 12;

/// The deepest chain a run can be counted on to reach.
///
/// `make balance` measures this on a run holding nothing, which is what a
/// chain's rule asks for: every playthrough gets three deep, most get five,
/// half get six, one in twenty-five gets twelve. Items are kept at or under
/// this, so a solo run is not asked to produce something rare to finish its
/// own pool. The deeper ones are locations all the same, and in a multiworld
/// they will be holding somebody else's item.
pub const RELIABLE_CHAIN: u32 = 6;

/// The smallest match there is, and the largest asked for.
///
/// Three is the game. Six is a straight run of six, or two runs off one swap;
/// past it the boards here are too small to rely on.
pub const SHORTEST_MATCH: u32 = 3;
pub const LONGEST_MATCH: u32 = 6;

impl Location {
    /// Which sort of location this is, for the event that names it. A kind of
    /// [`NO_LOCATION`] means the item came from nowhere on this board, which
    /// is what a multiworld handing one over looks like.
    pub fn kind(self) -> u8 {
        match self {
            Location::LevelClear(_) => 0,
            Location::Chain(_) => 1,
            Location::LevelSilver(_) => 2,
            Location::LevelGold(_) => 3,
            Location::ApGem { .. } => 4,
            Location::Match(_) => 5,
        }
    }

    /// The location's one parameter, alongside its kind.
    pub fn param(self) -> u16 {
        match self {
            Location::LevelClear(index)
            | Location::LevelSilver(index)
            | Location::LevelGold(index) => index as u16,
            Location::Chain(length) => length as u16,
            Location::Match(gems) => gems as u16,
            // Both halves, because neither alone names it. Ten to a level, so
            // a fifty level ladder needs nine bits and this has sixteen.
            Location::ApGem { level, index } => {
                level as u16 * AP_GEMS_PER_LEVEL as u16 + index as u16
            }
        }
    }

    /// A stable number for this location.
    ///
    /// Stability is the whole point: these are written into a save so a run
    /// can be handed back what it had already found. Renumbering them quietly
    /// gives a returning player somebody else's items.
    pub fn id(self) -> u32 {
        match self {
            Location::LevelClear(index) => index as u32,
            Location::Chain(length) => CHAIN_ID_BASE + length,
            Location::LevelSilver(index) => SILVER_ID_BASE + index as u32,
            Location::LevelGold(index) => GOLD_ID_BASE + index as u32,
            // Ten to a level, packed in order, so a level's whole sequence is
            // one run of numbers and appending levels appends numbers.
            Location::ApGem { level, index } => {
                AP_GEM_ID_BASE + level as u32 * AP_GEMS_PER_LEVEL + index
            }
            Location::Match(gems) => MATCH_ID_BASE + gems,
        }
    }

    /// Reads one back. `None` for a number no location has, which is what a
    /// save written by a later version looks like from here.
    pub fn from_id(id: u32) -> Option<Location> {
        if id < CHAIN_ID_BASE {
            return Some(Location::LevelClear(id as usize));
        }
        if id >= MATCH_ID_BASE {
            let gems = id - MATCH_ID_BASE;
            return (SHORTEST_MATCH..=LONGEST_MATCH)
                .contains(&gems)
                .then_some(Location::Match(gems));
        }
        if id >= AP_GEM_ID_BASE {
            let at = id - AP_GEM_ID_BASE;
            return Some(Location::ApGem {
                level: (at / AP_GEMS_PER_LEVEL) as usize,
                index: at % AP_GEMS_PER_LEVEL,
            });
        }
        if id >= GOLD_ID_BASE {
            return Some(Location::LevelGold((id - GOLD_ID_BASE) as usize));
        }
        if id >= SILVER_ID_BASE {
            return Some(Location::LevelSilver((id - SILVER_ID_BASE) as usize));
        }
        let length = id - CHAIN_ID_BASE;
        (SHORTEST_CHAIN..=LONGEST_CHAIN).contains(&length).then_some(Location::Chain(length))
    }
}

/// Each kind gets its own thousand, well clear of any ladder length, so they
/// never collide however many levels there come to be.
///
/// A level clear keeps the bare index it has always had, because these
/// numbers are written into saves: renumbering them hands a returning player
/// somebody else's items.
const CHAIN_ID_BASE: u32 = 1_000;
const SILVER_ID_BASE: u32 = 2_000;
const GOLD_ID_BASE: u32 = 3_000;
/// Fifty levels of ten fit inside this thousand, which is the ladder's own
/// ceiling, so this stays one block like the rest.
const AP_GEM_ID_BASE: u32 = 4_000;
/// The match sizes, which puts a ceiling on the gems above: their block runs
/// to the top of its own thousand and no further.
const MATCH_ID_BASE: u32 = 5_000;

/// How many Archipelago gem locations every level has, whatever a run puts in
/// play.
///
/// The table is the datapackage: fixed names and fixed numbers, the same for
/// everybody, so a run asking for one gem and a run asking for ten are reading
/// the same list. [`ap_gems_per_level`] is how many of them a given run
/// actually holds.
pub const AP_GEMS_PER_LEVEL: u32 = 10;

/// Where the move items start. The unlocks sit below it on their own codes.
const MOVES_ID_BASE: u32 = 1_000;

/// Filler's own number, in a thousand of its own like every other kind.
const FILLER_ID: u32 = 2_000;

/// What Archipelago's own numbers are offset by.
///
/// Its ids only have to be unique within one game, so the engine's own
/// numbering would do. Offsetting keeps the two apart all the same: a number
/// read off a spoiler log or a tracker is unmistakably an Archipelago id and
/// not a location id out of a solo save, and the day one of them has to move
/// the other can stay put. The digits are the ASCII for "tw".
pub const AP_ID_BASE: u32 = 7_477_000;

/// The location index standing for no location at all: an item the multiworld
/// sent rather than one this run found.
pub const NO_LOCATION: u16 = u16::MAX;

/// What a run must hold, or have reached, before a location can be checked.
///
/// Shaped to Archipelago's own rule vocabulary rather than to anything of
/// ours, so it goes over to the apworld as the rules its builder already reads
/// back: [`Requirement::All`] is `And`, [`Requirement::Any`] is `Or`,
/// [`Requirement::Has`] is `Has`, [`Requirement::Reached`] is
/// `CanReachLocation`, and [`Requirement::When`] is the option filter every
/// rule can carry. The engine evaluates the same trees for the solo run, so
/// neither side translates the other and neither can drift.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Requirement {
    /// Nothing at all: reachable from the start.
    Always,
    /// Every one of these.
    All(Vec<Requirement>),
    /// Any one of these. What a setting with several values comes to: one
    /// branch per value, each true only under its own.
    Any(Vec<Requirement>),
    /// At least `count` of an item.
    Has { item: Item, count: Count },
    /// Somewhere else has to be checkable first. What gates the ladder: a
    /// level can only be played once the one below it has been cleared.
    Reached(Location),
    /// This, but only when a setting is set a particular way. Under any other
    /// value it is simply false, which is what makes a list of these behave
    /// like a switch when they are gathered under an [`Requirement::Any`].
    When { setting: &'static str, is: u32, then: Box<Requirement> },
}

/// How many of an item a rule asks for.
///
/// A literal for most, and a setting for the ones a player can turn up or
/// down. The difference matters because the apworld is generated once and read
/// by everybody: a count baked in at emit time would be whatever the engine
/// was built with rather than what that player asked for. Archipelago resolves
/// the same thing the same way, with `FromOption`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Count {
    Exactly(u32),
    Setting(&'static str),
}

impl Count {
    pub fn resolve(self, options: &Options) -> u32 {
        match self {
            Count::Exactly(count) => count,
            // A setting that no longer exists asks for nothing rather than
            // failing: a rule is a promise about what is enough, and the
            // safest reading of a missing one is not to gate on it.
            Count::Setting(key) => options.value_of(key).unwrap_or(0),
        }
    }
}

impl Requirement {
    /// Whether a run holding `inventory`, having reached `reached`, set up the
    /// way `options` says, may check this.
    pub fn met(&self, inventory: &Inventory, reached: &Reached, options: &Options) -> bool {
        match self {
            Requirement::Always => true,
            Requirement::All(parts) => {
                parts.iter().all(|part| part.met(inventory, reached, options))
            }
            Requirement::Any(parts) => {
                parts.iter().any(|part| part.met(inventory, reached, options))
            }
            Requirement::Has { item, count } => inventory.count(*item) >= count.resolve(options),
            Requirement::Reached(location) => reached.has(*location),
            Requirement::When { setting, is, then } => {
                options.value_of(setting) == Some(*is) && then.met(inventory, reached, options)
            }
        }
    }
}

/// Which locations a run can get to.
///
/// Indexed rather than a list to search, because working a placement out asks
/// this question once per rule per item and the ladder is long.
#[derive(Clone, Debug)]
pub struct Reached {
    seen: Vec<bool>,
    levels: usize,
}

impl Reached {
    /// A run that has got nowhere yet.
    ///
    /// Sized off [`locations`] rather than counted out again here, which is
    /// what kept it right when the gems were added: the two have to agree
    /// about how long the table is, and the second copy of the arithmetic was
    /// the one that went stale.
    pub fn none(levels: usize) -> Self {
        Reached { seen: vec![false; locations(levels).len()], levels }
    }

    pub fn has(&self, at: Location) -> bool {
        location_index(at, self.levels).is_some_and(|index| self.seen[index])
    }

    pub fn add(&mut self, at: Location) {
        if let Some(index) = location_index(at, self.levels) {
            self.seen[index] = true;
        }
    }

    pub fn count(&self) -> usize {
        self.seen.iter().filter(|seen| **seen).count()
    }
}

/// What a location asks before it can be checked.
///
/// The ladder gates itself: a level is playable once the one below it has been
/// cleared, and nothing more is asked to clear it, which is the rule that says
/// every level must be beatable on its own budget.
///
/// A score mark asks for the five unlocks as well. Nearly all of a good score
/// comes from the flourish at the end of a level, and the flourish has nothing
/// to mint without them, so a run holding none of them is at best flipping a
/// coin for a mark. Logic should not depend on a coin landing. For most of the
/// ladder this asks for nothing extra, since reaching level six already means
/// having cleared the five levels the unlocks sit on; it bites only on the
/// opening levels, which are exactly the ones a run comes back to later.
///
/// Gold asks for that level's move items on top: it means beating the level as
/// well as it can be beaten, so everything that level has to offer should be
/// in hand first. Putting progression behind the location that needs the most
/// progression is what makes no sense, not requiring it.
///
/// That makes the move items progression, and progression can be found later
/// than the level it belongs to: level five's moves behind level ten's clear
/// is a perfectly ordinary shape. What it cannot be is behind level five's own
/// gold, and that is a constraint on the placement rather than on the rule.
/// See [`solo_placement`], which fills by reachability for exactly this
/// reason.
pub fn requirement(location: Location, _levels: usize) -> Requirement {
    let tools = || {
        Requirement::All(
            UNLOCKABLE
                .iter()
                .map(|special| Requirement::Has {
                    item: Item::Unlock(*special),
                    count: Count::Exactly(1),
                })
                .collect(),
        )
    };
    match location {
        Location::LevelClear(0) => Requirement::Always,
        Location::LevelClear(index) => Requirement::Reached(Location::LevelClear(index - 1)),
        Location::LevelSilver(index) => Requirement::All(vec![
            Requirement::Reached(Location::LevelClear(index)),
            tools(),
        ]),
        Location::LevelGold(index) => Requirement::All(vec![
            Requirement::Reached(Location::LevelClear(index)),
            tools(),
            Requirement::Has {
                item: Item::Moves { level: index },
                // One, because a level's upgrade is one item holding the whole
                // of what that level grants. When it can be split into pieces
                // this becomes however many pieces that player asked for,
                // which is a setting rather than a number, because the apworld
                // is generated once and read by everybody.
                count: Count::Exactly(1),
            },
        ]),
        // A chain is made on whatever board is in front of you, and the
        // opening one is in front of everybody.
        Location::Chain(_) => Requirement::Always,
        // And so is a match: it is the one thing the game asks a player to do
        // and it asks for no item to do it with. Which sizes a solo run is
        // asked to line up for its own progression is [`worth_using`].
        Location::Match(_) => Requirement::Always,
        // A gem is collected by playing its level, not by beating it, so it
        // asks for what reaching that level asks for and nothing more. Within
        // a level they go in order, because clearing one checks the lowest
        // still unchecked: the second cannot be taken before the first.
        Location::ApGem { level: 0, index: 0 } => Requirement::Always,
        Location::ApGem { level, index: 0 } => {
            Requirement::Reached(Location::LevelClear(level - 1))
        }
        Location::ApGem { level, index } => {
            Requirement::Reached(Location::ApGem { level, index: index - 1 })
        }
    }
}

/// What finishing the game takes, as a rule.
///
/// One rule covering every goal a run can be set to rather than four rules to
/// choose between, because the apworld is generated once and read by
/// everybody: which branch is live is decided when the rule is evaluated, by
/// the setting, on both sides. Each branch is false under any other value, so
/// gathering them under [`Requirement::Any`] makes the list behave as a
/// switch.
pub fn goal(levels: usize) -> Requirement {
    let last = levels.saturating_sub(1);
    let every = |at: fn(usize) -> Location| {
        Requirement::All((0..levels).map(|index| Requirement::Reached(at(index))).collect())
    };
    let when = |goal: Goal, then: Requirement| Requirement::When {
        setting: GOAL_SETTING,
        is: goal.value(),
        then: Box::new(then),
    };
    Requirement::Any(vec![
        when(Goal::GoldOnLastLevel, Requirement::Reached(Location::LevelGold(last))),
        when(Goal::ClearLastLevel, Requirement::Reached(Location::LevelClear(last))),
        when(Goal::ClearEveryLevel, every(Location::LevelClear)),
        when(Goal::GoldOnEveryLevel, every(Location::LevelGold)),
    ])
}

/// Every distinct item in the game, in a stable order.
///
/// Distinct, not the pool: three of a level's move items are three copies of
/// one item. This is the list that becomes Archipelago's item name table, so
/// the order is an identity and appending is the only safe way to change it.
pub fn items(levels: usize) -> Vec<Item> {
    UNLOCKABLE
        .iter()
        .map(|special| Item::Unlock(*special))
        .chain((0..levels).map(|level| Item::Moves { level }))
        // Last, and in the table despite never being in the pool: a run has
        // to be able to name what it was handed, and this is what the
        // leftover locations hold.
        .chain(std::iter::once(Item::Filler))
        .collect()
}

/// What an item is called.
///
/// These strings are the item's identity everywhere outside the engine: in the
/// feed, in a tracker, in a spoiler log. Renaming one silently breaks every
/// seed rolled before the change, so they are worth settling rather than
/// tidying later.
pub fn item_name(item: Item) -> String {
    match item {
        Item::Unlock(special) => match special {
            Special::LineH => "Horizontal Line Clear".to_string(),
            Special::LineV => "Vertical Line Clear".to_string(),
            Special::Cross => "Cross Clear".to_string(),
            Special::Rainbow => "Rainbow".to_string(),
            Special::Rocket => "Rocket".to_string(),
            // Neither is an item anybody is handed: one is the absence of a
            // gem kind and the other is a location wearing a gem's clothes.
            Special::None | Special::Archipelago => "Nothing".to_string(),
        },
        Item::Moves { level } => format!("Level {} Moves Upgrade", level + 1),
        Item::Filler => "Filler".to_string(),
    }
}

/// What a location is called. See [`item_name`] on why these are settled.
pub fn location_name(location: Location) -> String {
    match location {
        Location::LevelClear(index) => format!("Level {} Clear", index + 1),
        Location::LevelSilver(index) => format!("Level {} Silver", index + 1),
        Location::LevelGold(index) => format!("Level {} Gold", index + 1),
        Location::Chain(length) => format!("{length} Chain"),
        // Troy's wording, 2026-09-24. Not "{n} Match", which would read like
        // the chains above it: what these ask for is a thing to go and do,
        // and the verb is what says so.
        Location::Match(gems) => format!("Activate {gems} match"),
        // "AP Gem" rather than the word spelled out: this is what the feed
        // shows while a level is being played, where a line has to be read at
        // a glance and "Archipelago" is most of its width. Every other reader
        // of these names gets the same one, which is the point of there being
        // only one.
        Location::ApGem { level, index } => format!("Level {} AP Gem {}", level + 1, index + 1),
    }
}

/// Every location the game knows of, which is the list that becomes
/// Archipelago's location name table.
///
/// Every level's ten Archipelago gems are here whatever a run asked for, for
/// the same reason every item is in the item table whatever a run holds: the
/// datapackage is one fixed list shared by everybody, and a name that came and
/// went with a yaml setting would not be one.
///
/// What a given run actually plays over is the subset [`in_play`] gives.
pub fn locations(level_count: usize) -> Vec<Location> {
    (0..level_count)
        .map(Location::LevelClear)
        .chain((0..level_count).map(Location::LevelSilver))
        .chain((0..level_count).map(Location::LevelGold))
        .chain((SHORTEST_CHAIN..=LONGEST_CHAIN).map(Location::Chain))
        .chain((0..level_count).flat_map(|level| {
            (0..AP_GEMS_PER_LEVEL).map(move |index| Location::ApGem { level, index })
        }))
        // Last, because appending is the only safe way to change this order:
        // see [`location_index`].
        .chain((SHORTEST_MATCH..=LONGEST_MATCH).map(Location::Match))
        .collect()
}

/// How many Archipelago gems each level carries in a run set up this way.
///
/// The setting is a floor rather than a count. A run's items have to fit
/// somewhere, and the options that decide how many items there are do not
/// know or care how many places there are to put them, so this closes the gap:
/// enough gems per level to hold whatever the rest of the world could not.
///
/// Capped at [`AP_GEMS_PER_LEVEL`], which is as many as the location table
/// has. A run whose pool will not fit even then is one the rules and the pool
/// disagree about, and `the_pool_fits_in_the_locations_there_are` is where
/// that is caught rather than here.
pub fn ap_gems_per_level(levels: usize, options: &Options) -> u32 {
    let needed = ap_gems_needed(levels, item_pool(levels, options).len());
    options.ap_gems.max(needed).min(AP_GEMS_PER_LEVEL)
}

/// How many gems a level has to carry for a pool of `pool` items to have
/// somewhere to go, ignoring what anybody asked for.
///
/// Split out from [`ap_gems_per_level`] so the arithmetic can be put a pool
/// that the game cannot currently produce: this is the machinery that the
/// progressive moves upgrade is waiting on, and it wants checking before the
/// thing that needs it exists.
pub fn ap_gems_needed(levels: usize, pool: usize) -> u32 {
    if levels == 0 {
        return 0;
    }
    // Everywhere but the gems, which is what they are there to top up.
    //
    // Every one of them, including the deep chains the solo fill will not use.
    // This number decides which locations a world has at all, and the solo
    // side and the Archipelago side have to arrive at the same one or a seed's
    // locations and the game's would not match. That the solo fill then
    // declines to put its own items down a twelve-deep chain is a separate
    // matter, and `the_solo_placement_finds_a_home_for_the_whole_pool` is
    // what catches it if that ever leaves the fill short.
    let elsewhere =
        locations(levels).iter().filter(|at| !matches!(at, Location::ApGem { .. })).count();
    // Round up: half a location is no location.
    let short = pool.saturating_sub(elsewhere);
    (short.div_ceil(levels) as u32).min(AP_GEMS_PER_LEVEL)
}

/// Whether this location is one a run set up this way actually plays over.
///
/// The gems past what a run asked for are in the table but not in the world:
/// nothing is hidden in them, and nothing spawns for them.
pub fn in_play(at: Location, levels: usize, options: &Options) -> bool {
    match at {
        Location::ApGem { level, index } => {
            level < levels && index < ap_gems_per_level(levels, options)
        }
        _ => true,
    }
}

/// Where an item sits in [`items`], which is what an event carries instead of
/// the name itself: the stream has no room for text.
///
/// Worked out rather than searched for, because a placement asks this
/// thousands of times. `items_and_locations_are_numbered_as_they_are_listed`
/// holds it to the same order the list gives.
pub fn item_index(item: Item, levels: usize) -> Option<usize> {
    match item {
        Item::Unlock(special) => UNLOCKABLE.iter().position(|other| *other == special),
        Item::Moves { level } => (level < levels).then_some(UNLOCKABLE.len() + level),
        // Last, because appending is the only safe way to change this order:
        // an item's place in the table is what an event carries instead of its
        // name, and everything already numbered has to keep its number.
        Item::Filler => Some(UNLOCKABLE.len() + levels),
    }
}

/// Where a location sits in [`locations`]. See [`item_index`].
pub fn location_index(location: Location, levels: usize) -> Option<usize> {
    match location {
        Location::LevelClear(index) => (index < levels).then_some(index),
        Location::LevelSilver(index) => (index < levels).then_some(levels + index),
        Location::LevelGold(index) => (index < levels).then_some(2 * levels + index),
        Location::Chain(length) => (SHORTEST_CHAIN..=LONGEST_CHAIN)
            .contains(&length)
            .then_some(3 * levels + (length - SHORTEST_CHAIN) as usize),
        Location::ApGem { level, index } => (level < levels
            && index < AP_GEMS_PER_LEVEL)
            .then(|| {
                3 * levels
                    + (LONGEST_CHAIN - SHORTEST_CHAIN + 1) as usize
                    + level * AP_GEMS_PER_LEVEL as usize
                    + index as usize
            }),
        Location::Match(gems) => (SHORTEST_MATCH..=LONGEST_MATCH).contains(&gems).then(|| {
            3 * levels
                + (LONGEST_CHAIN - SHORTEST_CHAIN + 1) as usize
                + levels * AP_GEMS_PER_LEVEL as usize
                + (gems - SHORTEST_MATCH) as usize
        }),
    }
}

/// The five unlocks, in the order a solo run is given them.
///
/// A pacing decision, and re-tunable. Deliberately not the order the item
/// table uses: see [`UNLOCKABLE`].
pub const UNLOCKS: [Special; 5] =
    [Special::LineV, Special::LineH, Special::Rocket, Special::Cross, Special::Rainbow];

/// The same five in the order the item table numbers them, which is the order
/// [`Special`] itself is numbered in.
///
/// Separate from [`UNLOCKS`] on purpose. The item table is an identity that
/// ends up in seeds, so it must not move; the order a run is given them is a
/// judgment about teaching that we should stay free to change.
const UNLOCKABLE: [Special; 5] =
    [Special::LineH, Special::LineV, Special::Cross, Special::Rainbow, Special::Rocket];

/// Everything a run has received.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Inventory {
    specials: SpecialSet,
    /// How many move items have landed on each level, indexed by level. Short
    /// or empty for levels nothing has been found for yet.
    moves: Vec<u32>,
    /// How much of nothing the run has been handed. Kept only so
    /// [`Inventory::count`] can answer honestly.
    filler: u32,
}

impl Inventory {
    /// A run that has been given nothing.
    pub fn empty() -> Self {
        Inventory { specials: SpecialSet::NONE, moves: Vec::new(), filler: 0 }
    }

    /// Takes an item in. Returns whether the run is better off for it, which
    /// is what decides if there is anything to announce.
    ///
    /// An unlock arriving twice is harmless and silent: a multiworld can
    /// resend on a reconnect, and the run has to come out the same. Move items
    /// stack instead, so each one is worth saying.
    pub fn receive(&mut self, item: Item) -> bool {
        match item {
            Item::Unlock(special) => {
                let slot = match special {
                    Special::LineH => &mut self.specials.line_h,
                    Special::LineV => &mut self.specials.line_v,
                    Special::Cross => &mut self.specials.cross,
                    Special::Rainbow => &mut self.specials.rainbow,
                    Special::Rocket => &mut self.specials.rocket,
                    // Neither is an unlock anyone can hold. There is no gem
                    // to gate for one, and the other is never gated at all:
                    // an Archipelago gem is a location, and a run that could
                    // not see its own locations would have nowhere to look.
                    Special::None | Special::Archipelago => return false,
                };
                let is_new = !*slot;
                *slot = true;
                is_new
            }
            Item::Moves { level } => {
                if self.moves.len() <= level {
                    self.moves.resize(level + 1, 0);
                }
                self.moves[level] += 1;
                true
            }
            // Changes nothing about what the run may do, and is still worth
            // announcing: the player checked a location and was handed
            // something, and silence there would read as the check failing.
            Item::Filler => {
                self.filler += 1;
                true
            }
        }
    }

    pub fn has(&self, item: Item) -> bool {
        match item {
            Item::Unlock(_) => {
                let mut probe = self.clone();
                // Already held exactly when taking it in changes nothing.
                !probe.receive(item)
            }
            Item::Moves { level } => self.moves.get(level).copied().unwrap_or(0) > 0,
            Item::Filler => self.filler > 0,
        }
    }

    /// Which specials this run may make.
    pub fn specials(&self) -> SpecialSet {
        self.specials
    }

    /// How many move items have landed on one level.
    pub fn moves_found(&self, level: usize) -> u32 {
        self.moves.get(level).copied().unwrap_or(0)
    }

    /// How many of an item this run holds, which is what a rule asks.
    pub fn count(&self, item: Item) -> u32 {
        match item {
            Item::Unlock(_) => u32::from(self.has(item)),
            Item::Moves { level } => self.moves_found(level),
            // Counted rather than waved away, so this answers truthfully
            // whatever asks. Nothing does: no rule can sensibly be built on
            // how much nothing a run has been handed.
            Item::Filler => self.filler,
        }
    }

    /// Cuts a level down to what this run may do, and opens it up by what the
    /// run has earned.
    ///
    /// Specials only ever come off: a level keeps saying which belong on it, so
    /// one that never wanted rockets does not get them because the player found
    /// the unlock elsewhere. Moves only ever go on, on top of the budget the
    /// level was tuned to be beatable with.
    ///
    /// The upgrade lands whole and lands once. There is one of them per level
    /// in the pool, so a second copy is not something a seed can produce; if a
    /// multiworld sends one anyway it changes nothing, because the level's
    /// declared total is the total however many arrive. Splitting it into
    /// pieces that add up to the same number is what the progressive version
    /// will do, once there are locations enough to hold them all.
    pub fn apply(&self, level: usize, spec: &mut LevelSpec) {
        spec.rules.specials = spec.rules.specials.intersect(self.specials);
        if self.moves_found(level) > 0 {
            spec.moves += spec.moves_upgrade;
        }
    }
}

/// What a solo run finds at a location.
///
/// The fixed placement, which is the whole of "the solo options are fixed":
/// these are the same locations Archipelago will shuffle its own items across,
/// filled here in a set order instead.
///
/// The opening level clears hold the unlocks, in the order of how deliberately
/// a player can go after each shape. Four in a row is the first thing anyone
/// makes on purpose, and the level after the opener is named for it. The
/// rocket's 2x2 turns up by accident long before it is aimed at, the L and T
/// of a cross take looking for, and five in a line is the one you have to
/// build.
///
/// Every clear past those is worth more room on that same level, and a level's
/// two score marks are worth more room on the **next** one, so beating a level
/// well makes the one after it easier. The short chains carry what is left for
/// the levels whose own clear an unlock took.
///
/// **A level's score marks must never hold that level's own moves.** Gold
/// means beating a level as well as it can be beaten, so its rule wants
/// everything that level's progression has to offer; an item for that same
/// level sitting on it would be required to reach the place it is kept. Fill
/// either refuses that or strands it. Handing them to the next level up keeps
/// every mark clear of its own requirements.
///
/// The long chains hold nothing yet. They are hard enough to be worth keeping
/// somewhere to put the traps and the usable items when those exist, and in a
/// multiworld they will be holding somebody else's item anyway.
/// `levels` is how long the ladder is, which the marks on the last level need:
/// there is no next level for them to pay, so they hold nothing.
/// What a run dealt from `seed` finds at one location.
///
/// Builds the whole placement to answer, so it is for a one-off question. Hold
/// a [`solo_placement`] and index it if you are asking repeatedly.
pub fn solo_item_at(
    location: Location,
    levels: usize,
    seed: u64,
    options: &Options,
) -> Option<Item> {
    let placed = solo_placement(levels, seed, options);
    location_index(location, levels).and_then(|at| placed[at])
}

/// The whole pool a run has to find, in the order a solo placement lays it
/// out: the unlocks first, because everything else waits on them, then one
/// moves upgrade per level.
pub fn item_pool(levels: usize, _options: &Options) -> Vec<Item> {
    UNLOCKS
        .iter()
        .map(|special| Item::Unlock(*special))
        .chain(
            // Top of the ladder downward. A level's gold cannot be filled
            // until that level's own upgrade is placed somewhere else, so
            // finishing the deepest level first opens golds early and keeps
            // them opening ahead of the fill. Bottom upward leaves the last
            // few items with nowhere but the chains.
            (0..levels).rev().map(|level| Item::Moves { level }),
        )
        .collect()
}

/// Where a run dealt from `seed` finds each item, as one entry per location.
///
/// Filled by reachability rather than written out by hand. Every item here is
/// progression now that gold asks for a level's moves, and hand-assigning
/// progression is hand-solving a constraint problem: it has to hold for any
/// ladder length and any number of move items per level, and a table that is
/// right today quietly stops being right when either changes. So this does
/// what a generator does, in the plainest possible way.
///
/// Take everything reachable with what has been placed so far, put the next
/// item into the first empty one of them, and go round again. Placing only
/// into somewhere already reachable is what makes it safe: the inventory only
/// grows, so nothing that was reachable stops being so, and nothing can end up
/// behind itself.
///
/// This is not Archipelago's fill and does not try to be. The multiworld
/// shuffles, can place another world's items here, and walks itself back out
/// of corners when it paints itself in. This only has to produce one honest
/// layout, and it produces it the only way that never needs walking back:
/// forwards, out of what is already open.
///
/// The seed is the run's, so every solo run is its own game and the same seed
/// is the same game. That matters beyond variety: a save records the ids of
/// the locations it checked and looks the items back up here, so the run seed
/// has to be saved alongside them, and it is.
pub fn solo_placement(levels: usize, seed: u64, options: &Options) -> Vec<Option<Item>> {
    // The whole table, so an index into `held` is a `location_index` and a
    // save's ids look up straight. The gems a run did not ask for are in it
    // and simply never filled, which `worth_using` sees to.
    let places = locations(levels);
    let gems = ap_gems_per_level(levels, options);
    // The ones a run can actually be asked to check, worked out once. Most of
    // the table is Archipelago gems a run did not ask for, and walking past
    // them on every round of every item is most of what this used to cost.
    let usable: Vec<Location> =
        places.iter().copied().filter(|at| worth_using(*at, gems)).collect();
    let mut held: Vec<Option<Item>> = vec![None; places.len()];
    let mut reached = Reached::none(levels);
    let mut inventory = Inventory::empty();

    let mut rng = Rng::new(seed);
    for item in item_pool(levels, options) {
        expand(&usable, levels, &inventory, &mut reached, options);
        let open: Vec<usize> = usable
            .iter()
            .filter_map(|at| location_index(*at, levels))
            .filter(|index| held[*index].is_none())
            .filter(|index| reached.has(places[*index]))
            .collect();
        let Some(&index) = open.get(rng.below(open.len() as u32) as usize) else {
            // Nowhere left to put it. The rules and the pool disagree, which
            // is a design fault rather than something to paper over, so the
            // leftovers stay unplaced and the sphere walk reports it.
            break;
        };
        held[index] = Some(item);
        inventory.receive(item);
    }

    // Anything still empty gets filler, the way a multiworld would put another
    // world's items there. Without it a run can clear a level and be handed
    // nothing, which reads as a bug rather than as a quiet location.
    //
    // A spare moves upgrade used to go here, back when several of them stacked
    // on one level. They do not any more, so a second copy would change
    // nothing at all: the player would be handed an item they already had the
    // whole of, and told they had found something. Filler says what it is.
    expand(&usable, levels, &inventory, &mut reached, options);
    for at in &usable {
        let Some(index) = location_index(*at, levels) else { continue };
        if held[index].is_none() && reached.has(*at) {
            held[index] = Some(Item::Filler);
        }
    }
    held
}

/// Whether a solo run should be asked to check here for its own items, given
/// that each level carries `gems` Archipelago gems.
///
/// Every location is reachable in principle; the deep chains are just rare.
/// `make balance` measures how often a run gets there, and past
/// [`RELIABLE_CHAIN`] it falls away fast enough that keeping a level's own
/// progression behind one would be asking a solo player to be lucky. They stay
/// locations, and a multiworld will put somebody else's item in them.
///
/// A gem past what the run asked for is a different sort of unusable: it is in
/// the table because the table is fixed, but nothing will ever spawn for it,
/// so an item left there could never be found at all.
///
/// The matches are all usable, and deliberately have no threshold of their own.
/// `make balance` measures a three in 98% of playthroughs, a four in 70%, a
/// five in 37% and a six in 52%: note which one is rare. They are not a ladder
/// the way the chains are, because two separate threes off one swap is six and
/// happens by accident, while an exact five has to be a straight five or an L
/// of three and three. A `<=` cutoff would be the wrong shape for that, and
/// all four turn up often enough over a run to be worth a solo player's own
/// progression.
fn worth_using(location: Location, gems: u32) -> bool {
    match location {
        Location::Chain(length) => length <= RELIABLE_CHAIN,
        Location::ApGem { index, .. } => index < gems,
        _ => true,
    }
}

/// Adds every location the run can now reach, and everything that opens in
/// turn, until nothing more does.
fn expand(
    places: &[Location],
    levels: usize,
    inventory: &Inventory,
    reached: &mut Reached,
    options: &Options,
) {
    loop {
        let opened: Vec<Location> = places
            .iter()
            .copied()
            .filter(|at| !reached.has(*at))
            .filter(|at| requirement(*at, levels).met(inventory, reached, options))
            .collect();
        if opened.is_empty() {
            return;
        }
        for at in opened {
            reached.add(at);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::Objective;
    use crate::options::{setting_index, Kind, SETTINGS};
    use crate::rules::Rules;

    /// A level with a move budget and an upgrade, for the arithmetic below.
    fn spec(moves: u32, moves_upgrade: u32) -> LevelSpec {
        LevelSpec {
            name: "test",
            rules: Rules::default(),
            moves,
            moves_upgrade,
            objectives: vec![Objective::Score(1)],
            silver: 0,
            gold: 0,
            layout: None,
        }
    }

    #[test]
    fn a_new_run_can_make_nothing() {
        let inventory = Inventory::empty();
        assert!(inventory.specials().is_empty());

        let mut level = spec(20, 5);
        assert_eq!(level.rules.specials, SpecialSet::ALL, "a level offers everything by default");
        inventory.apply(0, &mut level);
        assert!(level.rules.specials.is_empty(), "and a run with nothing may make none of it");
        assert_eq!(level.moves, 20, "and it plays on the budget the level was tuned to");
    }

    #[test]
    fn an_unlock_opens_exactly_one_special() {
        let mut inventory = Inventory::empty();
        assert!(inventory.receive(Item::Unlock(Special::Rocket)));
        assert_eq!(
            inventory.specials(),
            SpecialSet { rocket: true, ..SpecialSet::NONE },
            "one unlock should not carry the others in with it",
        );
        assert!(inventory.has(Item::Unlock(Special::Rocket)));
        assert!(!inventory.has(Item::Unlock(Special::Cross)));
    }

    #[test]
    fn receiving_the_same_item_twice_is_quiet_and_harmless() {
        // A multiworld can resend on a reconnect, and the run has to come out
        // the same. Only the first arrival is worth announcing.
        let mut inventory = Inventory::empty();
        assert!(inventory.receive(Item::Unlock(Special::Rainbow)));
        let after_first = inventory.clone();
        assert!(!inventory.receive(Item::Unlock(Special::Rainbow)), "the second is not news");
        assert_eq!(inventory, after_first);
    }

    #[test]
    fn a_count_can_be_asked_of_a_setting_rather_than_written_down() {
        // No rule uses this at the moment: a level's upgrade is one item, so
        // its gold asks for exactly one. It is how an option-dependent rule
        // is expressed, though, and the next few settings all need it, so it
        // is worth holding to its behavior rather than leaving it to rot.
        let mut options = Options::default();
        options.goal = Goal::ClearEveryLevel;
        assert_eq!(Count::Exactly(3).resolve(&options), 3);
        assert_eq!(
            Count::Setting(GOAL_SETTING).resolve(&options),
            Goal::ClearEveryLevel.value(),
            "a count read the wrong setting",
        );
        // A rule is a promise about what is enough, and the safest reading of
        // a setting that is not there is not to gate on it at all.
        assert_eq!(Count::Setting("no_such_setting").resolve(&options), 0);
    }

    #[test]
    fn a_moves_upgrade_lands_on_the_level_it_names() {
        // It tops up that level's budget and no other. Which level it is for
        // is part of the item, so the inventory is a per-level tally rather
        // than a count on the run.
        let mut inventory = Inventory::empty();
        assert!(inventory.receive(Item::Moves { level: 3 }));
        assert_eq!(inventory.moves_found(3), 1);
        assert_eq!(inventory.moves_found(4), 0, "it spilled onto the next level");

        let mut level = spec(20, 7);
        inventory.apply(3, &mut level);
        assert_eq!(level.moves, 27);

        let mut elsewhere = spec(20, 7);
        inventory.apply(4, &mut elsewhere);
        assert_eq!(elsewhere.moves, 20, "another level should be untouched");
    }

    #[test]
    fn an_upgrade_is_worth_what_the_level_declares_and_no_more() {
        // Not a share of the budget: what a level is worth coming back to
        // better equipped is that level's own business, so two levels of the
        // same length can be worth different amounts.
        let mut inventory = Inventory::empty();
        inventory.receive(Item::Moves { level: 0 });

        for (moves, upgrade) in [(20, 5), (20, 12), (3, 4), (40, 1)] {
            let mut level = spec(moves, upgrade);
            inventory.apply(0, &mut level);
            assert_eq!(level.moves, moves + upgrade);
        }

        // And the whole of it arrives at once, so the declared number is the
        // total whatever a multiworld sends over. Splitting it into pieces
        // that add up to the same number is the progressive version's job.
        let mut twice = Inventory::empty();
        twice.receive(Item::Moves { level: 0 });
        twice.receive(Item::Moves { level: 0 });
        let mut level = spec(20, 7);
        twice.apply(0, &mut level);
        assert_eq!(level.moves, 27, "a second copy went past the level's declared total");
    }

    #[test]
    fn applying_an_inventory_only_ever_takes_specials_away() {
        // A level that never wanted rockets must not be handed one because the
        // player found the unlock somewhere else.
        let mut inventory = Inventory::empty();
        for special in UNLOCKS {
            inventory.receive(Item::Unlock(special));
        }
        let mut level = spec(20, 5);
        level.rules.specials = SpecialSet { rocket: false, ..SpecialSet::ALL };
        inventory.apply(0, &mut level);
        assert!(!level.rules.specials.rocket, "the level's own answer is final");
        assert!(level.rules.specials.rainbow, "and the rest are still on offer");
    }

    /// A ladder length to place over. Anything from a handful of levels to a
    /// full one, because the table has to hold at both ends.
    const LADDERS: [usize; 4] = [8, 13, 30, 50];

    /// Runs to deal placements from.
    ///
    /// Every run fills its own now, so a claim about the fill is a claim about
    /// all of them rather than about one lucky layout. These stand in: enough
    /// that a shape which only goes wrong sometimes turns up, few enough that
    /// the suite stays quick. A failure names the seed, so it can be read back
    /// into a single case.
    fn fills() -> impl Iterator<Item = u64> {
        (0..32u64).map(fill_seed)
    }

    /// Every way a run can be set up.
    ///
    /// Built out of the settings table rather than written here, so a setting
    /// added later is swept without anybody remembering to come back. What has
    /// to hold about a placement has to hold for all of these: a fill that
    /// only works at the default is a fill that breaks for the first player
    /// who changes anything.
    /// Every ladder, seed and setup worth walking a placement over.
    ///
    /// One iterator rather than three nested loops in every test, because the
    /// three multiply and the nesting was already two deep. A failure names
    /// all three, which is what makes a case out of a sweep.
    fn every_run() -> impl Iterator<Item = (usize, u64, Options)> {
        LADDERS.into_iter().flat_map(|levels| {
            fills().flat_map(move |seed| {
                setups().into_iter().map(move |options| (levels, seed, options))
            })
        })
    }

    /// Past this many values a choice is sampled rather than swept whole. See
    /// [`setups`].
    const SWEPT_WHOLE: usize = 4;

    fn setups() -> Vec<Options> {
        let mut all = vec![Options::default()];
        for (at, setting) in SETTINGS.iter().enumerate() {
            // These multiply, so anything with more than a handful of values
            // is sampled at its ends and its default rather than swept: the
            // ends are where a setting goes wrong, and a full sweep of every
            // combination turns this from a check into a sit-down. A short
            // choice is swept whole, because the goal's four values each pick
            // a different branch of the completion rule and all four have to
            // be walked.
            let mut values: Vec<u32> = match setting.kind {
                Kind::Range { low, high } => vec![low, setting.default, high],
                Kind::Choice(choices) if choices.len() <= SWEPT_WHOLE => {
                    choices.iter().map(|choice| choice.value).collect()
                }
                Kind::Choice(choices) => vec![
                    choices.first().expect("a choice has values").value,
                    setting.default,
                    choices.last().expect("a choice has values").value,
                ],
            };
            values.sort_unstable();
            values.dedup();
            all = all
                .iter()
                .flat_map(|base| {
                    values.iter().map(move |value| {
                        let mut one = *base;
                        assert!(one.set(at, *value), "the table offers a value it refuses");
                        one
                    })
                })
                .collect();
        }
        all
    }

    #[test]
    fn the_solo_placement_finds_a_home_for_the_whole_pool() {
        // Where each item ends up is the fill's business and changes with the
        // seed. What must hold is that everything gets placed and nothing is
        // invented: an item with nowhere to go is a level that can never be
        // improved as much as the others.
        for (levels, seed, options) in every_run() {
            let placed = solo_placement(levels, seed, &options);
            let mut left = item_pool(levels, &options);
            for held in placed.iter().flatten() {
                if let Some(at) = left.iter().position(|wanted| wanted == held) {
                    left.swap_remove(at);
                    continue;
                }
                // Anything beyond the pool is Filler and nothing else. A
                // second unlock would be a real fault, and so now would a
                // second moves upgrade: a level's upgrade lands whole and
                // once, so a spare is an item that does nothing while
                // announcing itself as a find.
                assert_eq!(
                    *held,
                    Item::Filler,
                    "{} was placed but is not in the pool",
                    item_name(*held),
                );
            }
            assert!(
                left.is_empty(),
                "on a ladder of {levels} dealt from {seed:#x} as {options:?}, {} items had \
                 nowhere to go, starting with {}",
                left.len(),
                item_name(left[0]),
            );
        }
    }

    #[test]
    fn a_levels_gold_never_holds_what_reaching_it_would_need() {
        // Gold asks for that level's move items, so one of them kept there
        // would be required to reach the place it is kept. Silver asks for no
        // such thing, so silver may hold them quite happily.
        for (levels, seed, options) in every_run() {
            let placed = solo_placement(levels, seed, &options);
            for index in 0..levels {
                let gold = Location::LevelGold(index);
                let at = location_index(gold, levels).unwrap();
                assert_ne!(
                    placed[at],
                    Some(Item::Moves { level: index }),
                    "dealt from {seed:#x} as {options:?}, {} holds the very item reaching it \
                     would need",
                    location_name(gold),
                );
            }
        }
    }

    #[test]
    fn every_level_has_its_upgrade_placed_somewhere() {
        // Each level should be worth going back to, whether its own clear
        // carries the item, its neighbor's marks do, or a chain does. A level
        // whose upgrade never made it into the world is a level that can only
        // ever be played on its bare budget.
        for (levels, seed, options) in every_run() {
            let placed = solo_placement(levels, seed, &options);
            for level in 0..levels {
                let found = placed
                    .iter()
                    .filter(|held| **held == Some(Item::Moves { level }))
                    .count();
                // At least one, not exactly one: what is left over once the
                // pool is placed becomes filler, and filler is more upgrades.
                assert!(
                    found >= 1,
                    "dealt from {seed:#x} as {options:?}, level {} of {levels} has no \
                     upgrade anywhere in the world",
                    level + 1,
                );
            }
        }
    }

    #[test]
    fn the_item_table_holds_the_same_unlocks_the_ladder_hands_out() {
        // Two orders of one set. The table is an identity that ends up in
        // seeds; the ladder's order is a judgment about teaching. Letting them
        // fall out of step would leave an unlock nobody can be given, or one
        // handed out that has no name.
        let mut table = UNLOCKABLE;
        let mut given = UNLOCKS;
        table.sort_by_key(|special| special.code());
        given.sort_by_key(|special| special.code());
        assert_eq!(table, given);
        assert_ne!(UNLOCKABLE, UNLOCKS, "the two are meant to be free to differ");
    }

    #[test]
    fn the_item_table_is_numbered_by_something_that_does_not_move() {
        // Item numbers end up in seeds, so the table is ordered by the
        // special's own code rather than by the order a run is given them,
        // which is re-tunable. Reordering the pacing must not renumber items.
        let table = items(13);
        let unlocks: Vec<Special> = table
            .iter()
            .filter_map(|item| match item {
                Item::Unlock(special) => Some(*special),
                Item::Moves { .. } | Item::Filler => None,
            })
            .collect();
        let mut by_code = unlocks.clone();
        by_code.sort_by_key(|special| special.code());
        assert_eq!(unlocks, by_code, "the item table is not in code order");
        assert_eq!(item_index(Item::Unlock(Special::LineH), 13), Some(0));
    }

    /// Walks a placement the way a generator does: take everything reachable,
    /// see what that opens, repeat until nothing new opens. Returns what was
    /// never reached.
    fn unreachable(levels: usize, seed: u64, options: &Options) -> Vec<Location> {
        walk(levels, &solo_placement(levels, seed, options), options, |at| {
            requirement(at, levels)
        })
    }

    /// Walks a placement under whatever rules are handed in, and says what was
    /// never reached.
    fn walk(
        levels: usize,
        placed: &[Option<Item>],
        options: &Options,
        rule: impl Fn(Location) -> Requirement,
    ) -> Vec<Location> {
        let all = locations(levels);
        let mut reached = Reached::none(levels);
        let mut held = Inventory::empty();
        loop {
            let opened: Vec<(usize, Location)> = all
                .iter()
                .copied()
                .enumerate()
                .filter(|(_, at)| !reached.has(*at))
                .filter(|(_, at)| rule(*at).met(&held, &reached, options))
                .collect();
            if opened.is_empty() {
                return all.into_iter().filter(|at| !reached.has(*at)).collect();
            }
            for (index, at) in opened {
                reached.add(at);
                if let Some(item) = placed[index] {
                    held.receive(item);
                }
            }
        }
    }


    #[test]
    fn every_location_in_the_solo_placement_can_be_reached() {
        // The check a generator does, done to every layout the fill can deal:
        // if a location asks for an item that is kept behind it, directly or
        // round a loop of several, nothing ever opens it and everything inside
        // is lost. A fill that is right for one seed and wrong for another is
        // a run somebody cannot finish, so this is the claim that the fill is
        // safe rather than lucky.
        for (levels, seed, options) in every_run() {
            let stuck = unreachable(levels, seed, &options);
            assert!(
                stuck.is_empty(),
                "on a ladder of {levels} dealt from {seed:#x} as {options:?}, {} locations \
                 can never be checked, starting with {}",
                stuck.len(),
                location_name(stuck[0]),
            );
        }
    }

    #[test]
    fn an_unlock_is_never_kept_behind_a_score_mark() {
        // The unlocks go in first, while a run holds nothing, and nothing
        // holding nothing can reach a score mark: every mark asks for all five
        // of them. So an unlock can only ever land somewhere a run holding
        // nothing can already get to, and that is what keeps the fill from
        // having to back out of a corner.
        //
        // A level clear, a chain, a match, or an Archipelago gem: each joined
        // this list when it was added, because none of them asks for an item.
        // Anything claiming every one of these is holding all five unlocks
        // whatever it was dealt, which is how the screenshot runs are set up.
        for (levels, seed, options) in every_run() {
            let placed = solo_placement(levels, seed, &options);
            for (at, held) in locations(levels).into_iter().zip(placed) {
                let Some(Item::Unlock(_)) = held else { continue };
                assert!(
                    matches!(
                        at,
                        Location::LevelClear(_)
                            | Location::Chain(_)
                            | Location::Match(_)
                            | Location::ApGem { .. }
                    ),
                    "on a ladder of {levels} dealt from {seed:#x} as {options:?}, {} is \
                     keeping {}, which reaching it would need",
                    location_name(at),
                    item_name(held.unwrap()),
                );
            }
        }
    }

    #[test]
    fn two_runs_are_dealt_different_progressions() {
        // The point of dealing per run: a second playthrough is a new game,
        // not the same one again. Every seed placing everything in the same
        // order would satisfy every other test here.
        let levels = 13;
        let set_up = Options::default();
        let first = solo_placement(levels, fill_seed(1), &set_up);
        let differs = fills()
            .filter(|seed| solo_placement(levels, *seed, &set_up) != first)
            .count();
        assert!(
            differs >= fills().count() - 1,
            "only {differs} of the seeds dealt something other than the first",
        );
    }

    #[test]
    fn a_run_seed_deals_the_same_progression_every_time() {
        // And the other half of it: a save records which locations it checked
        // and looks the items back up, so the same seed has to mean the same
        // items or a returning run is handed somebody else's.
        for seed in fills() {
            for options in setups() {
                assert_eq!(
                    solo_placement(13, seed, &options),
                    solo_placement(13, seed, &options),
                );
            }
        }
        assert_ne!(fill_seed(7), 7, "the fill should not share the deal's stream");
    }

    #[test]
    fn a_location_that_asked_for_what_it_keeps_would_be_caught() {
        // The sphere walk above is only worth having if it fails on the shape
        // it exists to find, so here is that shape at its smallest: the
        // opening level's clear asking for the very unlock it is holding.
        // Nothing opens it, so nothing opens at all.
        let levels = 13;
        let options = Options::default();
        let placed = solo_placement(levels, fill_seed(3), &options);
        let held = placed[0].expect("the opening clear holds something");
        let circular = |at: Location| match at {
            Location::LevelClear(0) => {
                Requirement::Has { item: held, count: Count::Exactly(1) }
            }
            other => requirement(other, levels),
        };
        let stuck = walk(levels, &placed, &options, circular);
        // The chains are open to anyone, and so is the opening level's own run
        // of gems, since playing it asks for nothing. The ladder and
        // everything hanging off it is what should be lost.
        assert!(
            stuck.contains(&Location::LevelClear(0)),
            "a location asking for the item it holds was reached anyway",
        );
        // Everything that asks for nothing at all: the chains, the matches,
        // and the opening level's gems, since playing it asks for nothing.
        let survivors = (LONGEST_CHAIN - SHORTEST_CHAIN + 1) as usize
            + (LONGEST_MATCH - SHORTEST_MATCH + 1) as usize
            + AP_GEMS_PER_LEVEL as usize;
        assert_eq!(
            stuck.len(),
            locations(levels).len() - survivors,
            "only what asks for nothing should have survived",
        );
    }

    #[test]
    fn nothing_is_asked_to_clear_a_level_beyond_reaching_it() {
        // Every level has to be beatable on its own move budget, which is what
        // lets the ladder be climbed by someone who finds nothing optional.
        for levels in LADDERS {
            for index in 0..levels {
                let asked = requirement(Location::LevelClear(index), levels);
                let ladder_only = match index {
                    0 => asked == Requirement::Always,
                    _ => asked == Requirement::Reached(Location::LevelClear(index - 1)),
                };
                assert!(ladder_only, "clearing level {} asks for more than the ladder", index + 1);
            }
        }
    }

    #[test]
    fn no_item_is_kept_behind_a_chain_hardly_anyone_reaches() {
        // A chain location exists for every length, but the deep ones are
        // rare: `make balance` reports the share of runs that get there, and
        // it falls off fast. Items live only on the short ones, and the rest
        // wait for the traps and usable items, which nobody has to find.
        for (levels, seed, options) in every_run() {
            let placed = solo_placement(levels, seed, &options);
            let deepest = (SHORTEST_CHAIN..=LONGEST_CHAIN)
                .filter(|length| {
                    placed[location_index(Location::Chain(*length), levels).unwrap()].is_some()
                })
                .max();
            if let Some(deepest) = deepest {
                assert!(
                    deepest <= RELIABLE_CHAIN,
                    "on a ladder of {levels} dealt from {seed:#x} as {options:?}, a \
                     {deepest} chain is holding an item, which few runs would ever reach",
                );
            }
        }
    }

    #[test]
    fn a_level_always_has_ten_gem_locations_however_few_are_in_play() {
        // The table is the datapackage: one fixed list of names and numbers,
        // the same for everybody. A run asking for one gem and a run asking
        // for ten read the same list; what differs is which of them hold
        // anything.
        for levels in LADDERS {
            let gems: Vec<Location> = locations(levels)
                .into_iter()
                .filter(|at| matches!(at, Location::ApGem { .. }))
                .collect();
            assert_eq!(gems.len(), levels * AP_GEMS_PER_LEVEL as usize);
            for value in [0, 1, AP_GEMS_PER_LEVEL] {
                let options = Options { ap_gems: value, ..Options::default() };
                assert_eq!(
                    locations(levels).len(),
                    3 * levels
                        + (LONGEST_CHAIN - SHORTEST_CHAIN + 1) as usize
                        + (LONGEST_MATCH - SHORTEST_MATCH + 1) as usize
                        + gems.len(),
                    "the table changed size when the setting did, at {value}",
                );
                let playing = gems.iter().filter(|at| in_play(**at, levels, &options)).count();
                assert_eq!(
                    playing,
                    levels * ap_gems_per_level(levels, &options) as usize,
                    "at {value} gems a level, the wrong number are in play",
                );
            }
        }
    }

    #[test]
    fn the_gem_floor_rises_when_the_pool_has_nowhere_else_to_go() {
        // The whole reason the setting is a floor. The options that decide how
        // many items there are do not know how many places there are to put
        // them, so this is what closes the gap.
        let levels = 13;
        let asked = Options { ap_gems: 0, ..Options::default() };
        assert_eq!(
            ap_gems_per_level(levels, &asked),
            0,
            "today's pool fits without them, so none should be forced",
        );

        // A pool too big for everywhere else has to be met with gems. Sized
        // off the real tables rather than a number written here, so this keeps
        // meaning the same thing as the ladder grows. Counted the way the
        // function counts: every location that is not a gem, deep chains
        // included, because this number decides what locations a world has
        // and both sides of it have to agree exactly.
        let elsewhere =
            locations(levels).iter().filter(|at| !matches!(at, Location::ApGem { .. })).count();
        let over = elsewhere + levels * 3 + 1;
        assert_eq!(
            ap_gems_needed(levels, over),
            4,
            "a pool of {over} against {elsewhere} places should want four gems a level",
        );
        assert_eq!(
            ap_gems_needed(levels, elsewhere),
            0,
            "a pool that already fits asked for gems anyway",
        );
        // And it never asks for more than the table has.
        assert_eq!(ap_gems_needed(levels, 100_000), AP_GEMS_PER_LEVEL);
    }

    #[test]
    fn an_item_never_lands_in_a_gem_the_run_did_not_ask_for() {
        // Nothing spawns for those, so an item left in one could never be
        // found: it would be a seed nobody can finish.
        for (levels, seed, options) in every_run() {
            let gems = ap_gems_per_level(levels, &options);
            let placed = solo_placement(levels, seed, &options);
            for (at, held) in locations(levels).into_iter().zip(placed) {
                let Location::ApGem { index, .. } = at else { continue };
                if index >= gems {
                    assert!(
                        held.is_none(),
                        "dealt from {seed:#x} as {options:?}, {} holds {} and nothing will \
                         ever spawn for it",
                        location_name(at),
                        item_name(held.unwrap()),
                    );
                }
            }
        }
    }

    #[test]
    fn the_pool_fits_in_the_locations_there_are() {
        // Archipelago has to put every item somewhere. More items than places
        // to hide them is a generation that cannot be made.
        //
        // Every setting, not just the defaults: a combination nobody can
        // generate is a combination the yaml should not offer. This is the
        // check that a one-item-per-move upgrade has to pass before it can be
        // offered, and today it would not: the ladder grants 166 moves and
        // there are 50 places to put things.
        for (levels, options) in LADDERS.iter().flat_map(|levels| {
            setups().into_iter().map(move |options| (*levels, options))
        }) {
            let places = locations(levels).len();
            let pool = item_pool(levels, &options).len();
            assert!(
                pool <= places,
                "{levels} levels as {options:?} give {pool} items and only {places} \
                 places to hide them",
            );
        }
    }

    #[test]
    fn the_numbers_already_written_down_still_mean_what_they_meant() {
        // Written out rather than worked out, because working them out the
        // same way twice proves nothing. These numbers are in saves on
        // people's machines and in Archipelago seeds already rolled; what a
        // number means is a promise, and this is the promise.
        //
        // The tempting refactor is to number these by where they sit in
        // `items` and `locations`, since both lists are built and indexed for
        // the event stream already. Those positions move with the length of
        // the ladder, so a thirteen level game and a fifty level one would
        // disagree about what 2013 means.
        //
        // Appending levels is safe and so is adding a kind of item, which is
        // what the thousands are for. Inserting a level in the middle is not,
        // and nothing here can catch it: the numbers follow a level's place in
        // the ladder, so a new level five renumbers every level above it.
        let places = [
            (0, "Level 1 Clear"),
            (12, "Level 13 Clear"),
            (1_002, "2 Chain"),
            (1_012, "12 Chain"),
            (2_000, "Level 1 Silver"),
            (2_012, "Level 13 Silver"),
            (3_000, "Level 1 Gold"),
            (3_049, "Level 50 Gold"),
            (5_003, "Activate 3 match"),
            (5_006, "Activate 6 match"),
        ];
        for (id, name) in places {
            let at = Location::from_id(id).expect("a number in use is a location");
            assert_eq!(location_name(at), name, "location {id} changed meaning");
            assert_eq!(at.id(), id, "and it does not answer to that number any more");
        }

        // Names as well as numbers, because both are the promise. An item's
        // name is what a spoiler log, a tracker and another player's client
        // call it, so renaming one breaks a seed exactly as surely as
        // renumbering it. This half of the table was numbers only, and a
        // rename went straight through it unnoticed.
        let things = [
            (1, "Horizontal Line Clear", Item::Unlock(Special::LineH)),
            (2, "Vertical Line Clear", Item::Unlock(Special::LineV)),
            (3, "Cross Clear", Item::Unlock(Special::Cross)),
            (4, "Rainbow", Item::Unlock(Special::Rainbow)),
            (5, "Rocket", Item::Unlock(Special::Rocket)),
            (1_000, "Level 1 Moves Upgrade", Item::Moves { level: 0 }),
            (1_049, "Level 50 Moves Upgrade", Item::Moves { level: 49 }),
            (2_000, "Filler", Item::Filler),
        ];
        for (id, name) in [
            (4_000, "Level 1 AP Gem 1"),
            (4_009, "Level 1 AP Gem 10"),
            (4_010, "Level 2 AP Gem 1"),
            (4_499, "Level 50 AP Gem 10"),
        ] {
            let at = Location::from_id(id).expect("a number in use is a location");
            assert_eq!(location_name(at), name, "location {id} changed meaning");
            assert_eq!(at.id(), id, "and it does not answer to that number any more");
        }
        for (id, name, item) in things {
            assert_eq!(item.id(), id, "{name} changed its number");
            assert_eq!(item_name(item), name, "item {id} changed meaning");
        }
    }

    #[test]
    fn every_item_has_a_number_of_its_own() {
        // These go into an Archipelago datapackage, where two items sharing a
        // number is two items nobody can tell apart.
        let ids: Vec<u32> = items(50).iter().map(|item| item.id()).collect();
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), ids.len(), "two items share a number");
        // And each kind stays in its own range, so a new kind of item can be
        // added without renumbering any of the others.
        assert!(
            items(50).iter().all(|item| match item {
                Item::Unlock(_) => item.id() < MOVES_ID_BASE,
                Item::Moves { .. } => (MOVES_ID_BASE..FILLER_ID).contains(&item.id()),
                Item::Filler => item.id() == FILLER_ID,
            }),
            "an item is numbered outside its own range",
        );
    }

    #[test]
    fn a_location_survives_being_written_down_and_read_back() {
        // These go into a save. A number that does not come back as the same
        // location hands a returning player somebody else's items.
        for location in locations(50) {
            assert_eq!(Location::from_id(location.id()), Some(location));
        }
        let ids: Vec<u32> = locations(50).iter().map(|at| at.id()).collect();
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), ids.len(), "two locations share a number");
        assert_eq!(Location::from_id(CHAIN_ID_BASE), None, "a chain of none is not a location");
        assert_eq!(Location::from_id(CHAIN_ID_BASE + LONGEST_CHAIN + 1), None);
        assert_eq!(
            Location::from_id(MATCH_ID_BASE + SHORTEST_MATCH - 1),
            None,
            "a match of two is not a match",
        );
        assert_eq!(Location::from_id(MATCH_ID_BASE + LONGEST_MATCH + 1), None);
        // The gems stop at the top of their own thousand now that something
        // sits above them. Before, every number above 4000 read back as a gem
        // on some level far past the end of the ladder.
        assert_eq!(Location::from_id(MATCH_ID_BASE - 1), Some(Location::ApGem {
            level: 99,
            index: 9,
        }));
    }

    #[test]
    fn every_goal_is_a_goal_a_run_can_reach_and_only_under_its_own_setting() {
        // One rule covers all four goals, and which branch is live is decided
        // by the setting when the rule is evaluated rather than when it was
        // written. So two things have to hold: a run that has reached
        // everything has finished, whichever goal it chose, and a run that has
        // reached everything the *other* goals want has not finished if its
        // own is still short.
        let levels = 13;
        let finished = goal(levels);
        let everything = {
            let mut reached = Reached::none(levels);
            for at in locations(levels) {
                reached.add(at);
            }
            reached
        };
        let nothing = Reached::none(levels);
        let held = Inventory::empty();

        for options in setups() {
            assert!(
                finished.met(&held, &everything, &options),
                "{options:?} cannot be finished even having reached every location",
            );
            assert!(
                !finished.met(&held, &nothing, &options),
                "{options:?} counts as finished having reached nothing at all",
            );
        }

        // And the branches are not interchangeable: clearing the last level is
        // not gold on it, whatever the run was set to.
        let mut cleared = Reached::none(levels);
        for index in 0..levels {
            cleared.add(Location::LevelClear(index));
        }
        let goal_at = setting_index(GOAL_SETTING).unwrap();
        let mut options = Options::default();
        assert!(options.set(goal_at, Goal::ClearLastLevel.value()));
        assert!(finished.met(&held, &cleared, &options), "a cleared ladder is not the end of it");
        assert!(options.set(goal_at, Goal::GoldOnLastLevel.value()));
        assert!(
            !finished.met(&held, &cleared, &options),
            "a ladder cleared without golds counted as gold on the last level",
        );
    }

    #[test]
    fn a_run_can_end_up_holding_every_unlock() {
        // Which location holds which unlock is the fill's business, so this
        // walks the whole placement rather than assuming any of them sit on
        // the ladder. Whether the ladder can be climbed to get at them is
        // `make balance`'s gate, which asks every level to be clearable by a
        // run that has found none of this.
        for (levels, seed, options) in every_run() {
            let placed = solo_placement(levels, seed, &options);
            let mut held = Inventory::empty();
            for item in placed.iter().flatten() {
                held.receive(*item);
            }
            assert_eq!(
                held.specials(),
                SpecialSet::ALL,
                "on a ladder of {levels} dealt from {seed:#x} as {options:?}, a run cannot \
                 end up holding every unlock",
            );
        }
    }
}
