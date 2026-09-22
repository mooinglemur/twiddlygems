//! What a run has been given, and where it found it.
//!
//! This is the seam the Archipelago layer slots into. A run receives items;
//! the items decide what the player may do. Nothing here knows the difference
//! between solo and multiworld, because there is only one difference: where
//! the items come from. Solo reads them out of the fixed table below;
//! Archipelago will be handed them by the server. Both walk the same
//! [`Inventory`] through the same [`apply`](Inventory::apply).
//!
//! Keeping both on one path is the point. The rule the multiworld has to hold
//! to, that a level is only ever placed somewhere the run can already clear
//! it, is a claim about this module, so it can be checked here rather than
//! rediscovered on the Python side.

use crate::board::Special;
use crate::level::LevelSpec;
use crate::rules::SpecialSet;

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
    /// Reaching a chain this long: one clear setting off the next, that many
    /// deep, off a single move.
    Chain(u32),
}

/// The shortest chain worth asking for. A single clear is not a chain.
pub const SHORTEST_CHAIN: u32 = 2;
/// The longest chain asked for. Past this a board cannot be relied on to
/// produce one, so a location there would be a location nobody can check.
pub const LONGEST_CHAIN: u32 = 12;

impl Location {
    /// Which sort of location this is, for the event that names it. A kind of
    /// [`NO_LOCATION`] means the item came from nowhere on this board, which
    /// is what a multiworld handing one over looks like.
    pub fn kind(self) -> u8 {
        match self {
            Location::LevelClear(_) => 0,
            Location::Chain(_) => 1,
        }
    }

    /// The location's one parameter, alongside its kind.
    pub fn param(self) -> u16 {
        match self {
            Location::LevelClear(index) => index as u16,
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
        }
    }

    /// Reads one back. `None` for a number no location has, which is what a
    /// save written by a later version looks like from here.
    pub fn from_id(id: u32) -> Option<Location> {
        if id < CHAIN_ID_BASE {
            return Some(Location::LevelClear(id as usize));
        }
        let length = id - CHAIN_ID_BASE;
        (SHORTEST_CHAIN..=LONGEST_CHAIN).contains(&length).then_some(Location::Chain(length))
    }
}

/// Well clear of any ladder length, so the two kinds never collide however
/// many levels there come to be.
const CHAIN_ID_BASE: u32 = 1_000;

/// The location kind for an item that came from no location at all: one the
/// multiworld sent rather than one this run found.
pub const NO_LOCATION: u8 = 255;

/// Every location in the game, which is the list a generator would place over.
pub fn locations(level_count: usize) -> Vec<Location> {
    (0..level_count)
        .map(Location::LevelClear)
        .chain((SHORTEST_CHAIN..=LONGEST_CHAIN).map(Location::Chain))
        .collect()
}

/// The five unlocks, in the order a solo run is given them.
pub const UNLOCKS: [Special; 5] =
    [Special::LineV, Special::LineH, Special::Rocket, Special::Cross, Special::Rainbow];

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
/// Every clear past those is worth more room on that same level, and the short
/// chains carry the move items for the levels the unlocks took, so nothing on
/// the ladder is cleared for nothing. The long chains hold nothing yet: they
/// are hard enough to be worth having somewhere to put, and in a multiworld
/// they will be holding somebody else's item anyway.
pub fn solo_item_at(location: Location) -> Option<Item> {
    match location {
        Location::LevelClear(index) => match UNLOCKS.get(index) {
            Some(special) => Some(Item::Unlock(*special)),
            None => Some(Item::Moves { level: index }),
        },
        Location::Chain(length) => {
            let level = (length - SHORTEST_CHAIN) as usize;
            (level < UNLOCKS.len()).then_some(Item::Moves { level })
        }
    }
}

/// The least a solo run can hold by the time it starts the level at `index`:
/// what the levels below it handed over, and nothing else.
///
/// This is the logic function, and it counts only the level clears on purpose.
/// Getting to level `index` means clearing every level below it, so those are
/// guaranteed. A chain is not: nobody is owed a five long one, so an item
/// sitting on that location cannot be assumed in hand. Logic has to hold for
/// the player who never made one.
pub fn solo_inventory(index: usize) -> Inventory {
    let mut inventory = Inventory::empty();
    for below in 0..index {
        if let Some(item) = solo_item_at(Location::LevelClear(below)) {
            inventory.receive(item);
        }
    }
    inventory
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

    #[test]
    fn the_solo_placement_puts_every_unlock_somewhere_and_leaves_no_clear_empty() {
        let places = locations(13);
        let mut unlocks: Vec<Special> = Vec::new();
        for location in &places {
            if let Some(Item::Unlock(special)) = solo_item_at(*location) {
                assert!(!unlocks.contains(&special), "{special:?} is placed twice");
                unlocks.push(special);
            }
        }
        assert_eq!(unlocks.len(), UNLOCKS.len(), "an unlock has nowhere to be found");

        for index in 0..13 {
            assert!(
                solo_item_at(Location::LevelClear(index)).is_some(),
                "clearing level {} is worth nothing",
                index + 1,
            );
        }
    }

    #[test]
    fn every_level_has_a_move_item_somewhere() {
        // Each level should be improvable, whether its own clear carries the
        // item or a chain does.
        let places = locations(13);
        for level in 0..13 {
            assert!(
                places.iter().any(|at| solo_item_at(*at) == Some(Item::Moves { level })),
                "nothing anywhere adds moves to level {}",
                level + 1,
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
    fn the_opening_level_is_reached_with_nothing_and_the_ladder_fills_up() {
        assert!(
            solo_inventory(0).specials().is_empty(),
            "the first level has to be clearable with no items at all",
        );
        // Each rung holds everything the ones below it handed over, and one
        // more of them until the pool runs out.
        for index in 1..=UNLOCKS.len() {
            let below = solo_inventory(index - 1);
            let here = solo_inventory(index);
            assert_ne!(here, below, "level {index} handed over nothing new");
            assert!(
                here.has(Item::Unlock(UNLOCKS[index - 1])),
                "level {index} did not hand over what the table says",
            );
        }
        let full = solo_inventory(UNLOCKS.len());
        assert_eq!(full.specials(), SpecialSet::ALL, "the pool should end up complete");
    }
}
