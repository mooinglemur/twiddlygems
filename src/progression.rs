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
}

impl Item {
    /// Which sort of item this is, for the event that announces it. See
    /// [`crate::game::EV_ITEM`].
    pub fn kind(self) -> u8 {
        match self {
            Item::Unlock(_) => 0,
            Item::Moves { .. } => 1,
        }
    }

    /// The item's one parameter, alongside its kind.
    pub fn value(self) -> u16 {
        match self {
            Item::Unlock(special) => special.code() as u16,
            Item::Moves { level } => level as u16,
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
        }
    }

    /// The location's one parameter, alongside its kind.
    pub fn param(self) -> u16 {
        match self {
            Location::LevelClear(index)
            | Location::LevelSilver(index)
            | Location::LevelGold(index) => index as u16,
            Location::Chain(length) => length as u16,
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
        }
    }

    /// Reads one back. `None` for a number no location has, which is what a
    /// save written by a later version looks like from here.
    pub fn from_id(id: u32) -> Option<Location> {
        if id < CHAIN_ID_BASE {
            return Some(Location::LevelClear(id as usize));
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

/// The location index standing for no location at all: an item the multiworld
/// sent rather than one this run found.
pub const NO_LOCATION: u16 = u16::MAX;

/// What a run must hold, or have reached, before a location can be checked.
///
/// Shaped to Archipelago's own rule vocabulary rather than to anything of
/// ours, so it goes over to the apworld as the rules its builder already reads
/// back: [`Requirement::All`] is `And`, [`Requirement::Has`] is `Has`, and
/// [`Requirement::Reached`] is `CanReachLocation`. The engine evaluates the
/// same trees for the solo run, so neither side translates the other and
/// neither can drift.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Requirement {
    /// Nothing at all: reachable from the start.
    Always,
    /// Every one of these.
    All(Vec<Requirement>),
    /// At least `count` of an item.
    Has { item: Item, count: u32 },
    /// Somewhere else has to be checkable first. What gates the ladder: a
    /// level can only be played once the one below it has been cleared.
    Reached(Location),
}

impl Requirement {
    /// Whether a run holding `inventory`, having reached `reached`, may check
    /// this.
    pub fn met(&self, inventory: &Inventory, reached: &Reached) -> bool {
        match self {
            Requirement::Always => true,
            Requirement::All(parts) => parts.iter().all(|part| part.met(inventory, reached)),
            Requirement::Has { item, count } => inventory.count(*item) >= *count,
            Requirement::Reached(location) => reached.has(*location),
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
    pub fn none(levels: usize) -> Self {
        Reached { seen: vec![false; 3 * levels + (LONGEST_CHAIN - SHORTEST_CHAIN + 1) as usize], levels }
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
                .map(|special| Requirement::Has { item: Item::Unlock(*special), count: 1 })
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
                count: MOVES_PER_LEVEL as u32,
            },
        ]),
        // A chain is made on whatever board is in front of you, and the
        // opening one is in front of everybody.
        Location::Chain(_) => Requirement::Always,
    }
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
            Special::None => "Nothing".to_string(),
        },
        Item::Moves { level } => format!("Level {} Progressive Moves", level + 1),
    }
}

/// What a location is called. See [`item_name`] on why these are settled.
pub fn location_name(location: Location) -> String {
    match location {
        Location::LevelClear(index) => format!("Level {} Clear", index + 1),
        Location::LevelSilver(index) => format!("Level {} Silver", index + 1),
        Location::LevelGold(index) => format!("Level {} Gold", index + 1),
        Location::Chain(length) => format!("{length} Chain"),
    }
}

/// Every location in the game, which is the list a generator would place over.
pub fn locations(level_count: usize) -> Vec<Location> {
    (0..level_count)
        .map(Location::LevelClear)
        .chain((0..level_count).map(Location::LevelSilver))
        .chain((0..level_count).map(Location::LevelGold))
        .chain((SHORTEST_CHAIN..=LONGEST_CHAIN).map(Location::Chain))
        .collect()
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
    }
}

/// How many times each level can be improved, which is how many of its move
/// items the placement has to find a home for.
///
/// Two, because three has no slack. A ladder of `L` levels offers `3L`
/// locations on the levels themselves and five chains short enough to count
/// on, and three move items each makes a pool of `5 + 3L` against exactly
/// `3L + 5` places to put it. A fill with nothing spare deadlocks on the last
/// item, and there would be nowhere at all for the traps and the usable items
/// to go. Two leaves a level's worth of slack and still makes every level
/// worth returning to twice.
///
/// Going back up wants more locations rather than a cleverer fill: the
/// Archipelago gem is the one that would pay for it.
pub const MOVES_PER_LEVEL: usize = 2;

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
}

impl Inventory {
    /// A run that has been given nothing.
    pub fn empty() -> Self {
        Inventory { specials: SpecialSet::NONE, moves: Vec::new() }
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
                    // Not an unlock anyone can hold; there is no gem to gate.
                    Special::None => return false,
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
        }
    }

    /// Cuts a level down to what this run may do, and opens it up by what the
    /// run has earned.
    ///
    /// Specials only ever come off: a level keeps saying which belong on it, so
    /// one that never wanted rockets does not get them because the player found
    /// the unlock elsewhere. Moves only ever go on, on top of the budget the
    /// level was tuned to be beatable with.
    pub fn apply(&self, level: usize, spec: &mut LevelSpec) {
        spec.rules.specials = spec.rules.specials.intersect(self.specials);
        spec.moves += self.moves_found(level) * move_step(spec.moves);
    }
}

/// What one move item is worth on a level whose own budget is `base`.
///
/// A share of the level's budget rather than a flat number, so an item means
/// about as much on a forty move level as on a sixteen. The floor keeps it
/// worth finding on the shortest levels there could ever be.
pub fn move_step(base: u32) -> u32 {
    (base / 4).max(2)
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
pub fn solo_item_at(location: Location, levels: usize, seed: u64) -> Option<Item> {
    let placed = solo_placement(levels, seed);
    location_index(location, levels).and_then(|at| placed[at])
}

/// The whole pool a run has to find, in the order a solo placement lays it
/// out: the unlocks first, because everything else waits on them, then each
/// level's move items.
pub fn item_pool(levels: usize) -> Vec<Item> {
    UNLOCKS
        .iter()
        .map(|special| Item::Unlock(*special))
        .chain(
            // Top of the ladder downward. A level's gold cannot be filled
            // until that level's own moves are all placed somewhere else, so
            // finishing the deepest level's set first opens golds early and
            // keeps them opening ahead of the fill. Bottom upward leaves the
            // last few items with nowhere but the chains.
            (0..levels).rev().flat_map(|level| {
                std::iter::repeat_n(Item::Moves { level }, MOVES_PER_LEVEL)
            }),
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
pub fn solo_placement(levels: usize, seed: u64) -> Vec<Option<Item>> {
    let places = locations(levels);
    let mut held: Vec<Option<Item>> = vec![None; places.len()];
    let mut reached = Reached::none(levels);
    let mut inventory = Inventory::empty();

    let mut rng = Rng::new(seed);
    for item in item_pool(levels) {
        expand(&places, levels, &inventory, &mut reached);
        let open: Vec<usize> = places
            .iter()
            .enumerate()
            .filter(|(index, at)| held[*index].is_none() && reached.has(**at) && worth_using(**at))
            .map(|(index, _)| index)
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
    // More moves is the filler we have: harmless wherever it lands, and it can
    // only ever open golds rather than close them, so dropping it in after the
    // pool is placed cannot strand anything.
    expand(&places, levels, &inventory, &mut reached);
    for (index, at) in places.iter().enumerate() {
        if held[index].is_none() && reached.has(*at) && worth_using(*at) {
            // Never a gold's own level, even though by now it would be
            // harmless: the location is already open, so it cannot be
            // required to reach itself. It reads as a mistake, and a rule
            // that is sometimes broken is not a rule.
            let mut level = rng.below(levels as u32) as usize;
            if let Location::LevelGold(index) = at {
                if level == *index {
                    level = (level + 1) % levels;
                }
            }
            held[index] = Some(Item::Moves { level });
        }
    }
    held
}

/// Whether a solo run should be asked to check here for its own items.
///
/// Every location is reachable in principle; the deep chains are just rare.
/// `make balance` measures how often a run gets there, and past
/// [`RELIABLE_CHAIN`] it falls away fast enough that keeping a level's own
/// progression behind one would be asking a solo player to be lucky. They stay
/// locations, and a multiworld will put somebody else's item in them.
fn worth_using(location: Location) -> bool {
    match location {
        Location::Chain(length) => length <= RELIABLE_CHAIN,
        _ => true,
    }
}

/// Adds every location the run can now reach, and everything that opens in
/// turn, until nothing more does.
fn expand(places: &[Location], levels: usize, inventory: &Inventory, reached: &mut Reached) {
    loop {
        let opened: Vec<Location> = places
            .iter()
            .copied()
            .filter(|at| !reached.has(*at))
            .filter(|at| requirement(*at, levels).met(inventory, reached))
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
    use crate::rules::Rules;

    /// A level with a move budget, for the arithmetic below.
    fn spec(moves: u32) -> LevelSpec {
        LevelSpec {
            name: "test",
            rules: Rules::default(),
            moves,
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

        let mut level = spec(20);
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
    fn move_items_stack_on_the_level_they_name() {
        // Unlike an unlock, a second one is worth having, and it lands on that
        // level rather than on the run.
        let mut inventory = Inventory::empty();
        assert!(inventory.receive(Item::Moves { level: 3 }));
        assert!(inventory.receive(Item::Moves { level: 3 }), "a second is still news");
        assert_eq!(inventory.moves_found(3), 2);
        assert_eq!(inventory.moves_found(4), 0, "it did not spill onto the next level");

        let mut level = spec(20);
        inventory.apply(3, &mut level);
        assert_eq!(level.moves, 20 + 2 * move_step(20));

        let mut elsewhere = spec(20);
        inventory.apply(4, &mut elsewhere);
        assert_eq!(elsewhere.moves, 20, "another level should be untouched");
    }

    #[test]
    fn a_move_item_is_worth_a_share_of_the_level_rather_than_a_flat_number() {
        // One item should mean about as much on a long level as on a short
        // one, and be worth finding even on the shortest there could be.
        assert_eq!(move_step(40), 10);
        assert_eq!(move_step(20), 5);
        assert!(move_step(4) >= 2, "a tiny level still owes something for one");
    }

    #[test]
    fn applying_an_inventory_only_ever_takes_specials_away() {
        // A level that never wanted rockets must not be handed one because the
        // player found the unlock somewhere else.
        let mut inventory = Inventory::empty();
        for special in UNLOCKS {
            inventory.receive(Item::Unlock(special));
        }
        let mut level = spec(20);
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

    #[test]
    fn the_solo_placement_finds_a_home_for_the_whole_pool() {
        // Where each item ends up is the fill's business and changes with the
        // seed. What must hold is that everything gets placed and nothing is
        // invented: an item with nowhere to go is a level that can never be
        // improved as much as the others.
        for levels in LADDERS {
            for seed in fills() {
                let placed = solo_placement(levels, seed);
                let mut left = item_pool(levels);
                for held in placed.iter().flatten() {
                    if let Some(at) = left.iter().position(|wanted| wanted == held) {
                        left.swap_remove(at);
                        continue;
                    }
                    // Anything beyond the pool is filler, which is only ever
                    // more moves: an unlock turning up twice would be a real
                    // fault.
                    assert!(
                        matches!(held, Item::Moves { .. }),
                        "{} was placed but is not in the pool",
                        item_name(*held),
                    );
                }
                assert!(
                    left.is_empty(),
                    "on a ladder of {levels} dealt from {seed:#x}, {} items had nowhere to go, \
                     starting with {}",
                    left.len(),
                    item_name(left[0]),
                );
            }
        }
    }

    #[test]
    fn a_levels_gold_never_holds_what_reaching_it_would_need() {
        // Gold asks for that level's move items, so one of them kept there
        // would be required to reach the place it is kept. Silver asks for no
        // such thing, so silver may hold them quite happily.
        for levels in LADDERS {
            for seed in fills() {
                let placed = solo_placement(levels, seed);
                for index in 0..levels {
                    let gold = Location::LevelGold(index);
                    let at = location_index(gold, levels).unwrap();
                    assert_ne!(
                        placed[at],
                        Some(Item::Moves { level: index }),
                        "dealt from {seed:#x}, {} holds the very item reaching it would need",
                        location_name(gold),
                    );
                }
            }
        }
    }

    #[test]
    fn every_level_is_improvable_the_same_number_of_times() {
        // Each level should be worth going back to as often as any other,
        // whether its own clear carries the items, its neighbor's marks do, or
        // a chain does.
        for levels in LADDERS {
            for seed in fills() {
                let placed = solo_placement(levels, seed);
                for level in 0..levels {
                    let found = placed
                        .iter()
                        .filter(|held| **held == Some(Item::Moves { level }))
                        .count();
                    // At least, not exactly: what is left over once the pool
                    // is placed becomes filler, and filler is more moves.
                    assert!(
                        found >= MOVES_PER_LEVEL,
                        "dealt from {seed:#x}, level {} of {levels} can be improved \
                         only {found} times",
                        level + 1,
                    );
                }
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
                Item::Moves { .. } => None,
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
    fn unreachable(levels: usize, seed: u64) -> Vec<Location> {
        walk(levels, &solo_placement(levels, seed), |at| requirement(at, levels))
    }

    /// Walks a placement under whatever rules are handed in, and says what was
    /// never reached.
    fn walk(
        levels: usize,
        placed: &[Option<Item>],
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
                .filter(|(_, at)| rule(*at).met(&held, &reached))
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
        for levels in LADDERS {
            for seed in fills() {
                let stuck = unreachable(levels, seed);
                assert!(
                    stuck.is_empty(),
                    "on a ladder of {levels} dealt from {seed:#x}, {} locations can never be \
                     checked, starting with {}",
                    stuck.len(),
                    location_name(stuck[0]),
                );
            }
        }
    }

    #[test]
    fn an_unlock_is_never_kept_behind_a_score_mark() {
        // The unlocks go in first, while a run holds nothing, and nothing
        // holding nothing can reach a score mark: every mark asks for all five
        // of them. So an unlock can only ever land on a level clear or on a
        // chain, and that is what keeps the fill from having to back out of a
        // corner. It also means claiming every clear and every short chain
        // hands a run all five, whatever it was dealt, which is how the
        // screenshot runs are set up.
        for levels in LADDERS {
            for seed in fills() {
                let placed = solo_placement(levels, seed);
                for (at, held) in locations(levels).into_iter().zip(placed) {
                    let Some(Item::Unlock(_)) = held else { continue };
                    assert!(
                        matches!(at, Location::LevelClear(_) | Location::Chain(_)),
                        "on a ladder of {levels} dealt from {seed:#x}, {} is keeping {}, \
                         which reaching it would need",
                        location_name(at),
                        item_name(held.unwrap()),
                    );
                }
            }
        }
    }

    #[test]
    fn two_runs_are_dealt_different_progressions() {
        // The point of dealing per run: a second playthrough is a new game,
        // not the same one again. Every seed placing everything in the same
        // order would satisfy every other test here.
        let levels = 13;
        let first = solo_placement(levels, fill_seed(1));
        let differs = fills()
            .filter(|seed| solo_placement(levels, *seed) != first)
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
            assert_eq!(solo_placement(13, seed), solo_placement(13, seed));
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
        let placed = solo_placement(levels, fill_seed(3));
        let held = placed[0].expect("the opening clear holds something");
        let circular = |at: Location| match at {
            Location::LevelClear(0) => Requirement::Has { item: held, count: 1 },
            other => requirement(other, levels),
        };
        let stuck = walk(levels, &placed, circular);
        // The chains are open to anyone and stay reachable; the ladder and
        // everything hanging off it is what should be lost.
        assert!(
            stuck.contains(&Location::LevelClear(0)),
            "a location asking for the item it holds was reached anyway",
        );
        assert_eq!(stuck.len(), 3 * levels, "only the chains should have survived");
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
        for levels in LADDERS {
            for seed in fills() {
                let placed = solo_placement(levels, seed);
                let deepest = (SHORTEST_CHAIN..=LONGEST_CHAIN)
                    .filter(|length| {
                        placed[location_index(Location::Chain(*length), levels).unwrap()].is_some()
                    })
                    .max();
                if let Some(deepest) = deepest {
                    assert!(
                        deepest <= RELIABLE_CHAIN,
                        "on a ladder of {levels} dealt from {seed:#x}, a {deepest} chain is \
                         holding an item, which few runs would ever reach",
                    );
                }
            }
        }
    }

    #[test]
    fn the_pool_fits_in_the_locations_there_are() {
        // Archipelago has to put every item somewhere. More items than places
        // to hide them is a generation that cannot be made.
        for levels in LADDERS {
            let places = locations(levels).len();
            let pool = UNLOCKS.len() + levels * MOVES_PER_LEVEL;
            assert!(
                pool <= places,
                "{levels} levels give {pool} items and only {places} places to hide them",
            );
        }
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
    }

    #[test]
    fn a_run_can_end_up_holding_every_unlock() {
        // Which location holds which unlock is the fill's business, so this
        // walks the whole placement rather than assuming any of them sit on
        // the ladder. Whether the ladder can be climbed to get at them is
        // `make balance`'s gate, which asks every level to be clearable by a
        // run that has found none of this.
        for levels in LADDERS {
            for seed in fills() {
                let placed = solo_placement(levels, seed);
                let mut held = Inventory::empty();
                for item in placed.iter().flatten() {
                    held.receive(*item);
                }
                assert_eq!(
                    held.specials(),
                    SpecialSet::ALL,
                    "on a ladder of {levels} dealt from {seed:#x}, a run cannot end up \
                     holding every unlock",
                );
            }
        }
    }
}
