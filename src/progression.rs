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
use crate::rules::{Rules, SpecialSet};

/// Something a run can receive.
///
/// One variant so far. Progressive move counts and the usable items come
/// later, and they arrive here rather than as a second mechanism.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Item {
    /// Matches may create this special again.
    ///
    /// Without it the board still matches and still clears; it simply leaves
    /// nothing behind. That is the state every run starts in.
    Unlock(Special),
}

impl Item {
    /// Which sort of item this is, for the event that announces it. See
    /// [`crate::game::EV_ITEM`].
    pub fn kind(self) -> u8 {
        match self {
            Item::Unlock(_) => 0,
        }
    }

    /// The item's one parameter, alongside its kind.
    pub fn value(self) -> u16 {
        match self {
            Item::Unlock(special) => special.code() as u16,
        }
    }
}

/// The five unlocks, in the order a solo run is given them.
pub const UNLOCKS: [Special; 5] =
    [Special::LineV, Special::LineH, Special::Rocket, Special::Cross, Special::Rainbow];

/// Everything a run has received.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Inventory {
    specials: SpecialSet,
}

impl Inventory {
    /// A run that has been given nothing.
    pub fn empty() -> Self {
        Inventory { specials: SpecialSet::NONE }
    }

    /// Takes an item in. Returns whether it was something the run did not
    /// already hold, which is what decides if there is anything to announce.
    ///
    /// Receiving the same item twice is harmless: a multiworld can send one
    /// again on a reconnect, and this has to be the same run either way.
    pub fn receive(&mut self, item: Item) -> bool {
        let Item::Unlock(special) = item;
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

    pub fn has(&self, item: Item) -> bool {
        let mut probe = *self;
        // Already held exactly when taking it in changes nothing.
        !probe.receive(item)
    }

    /// Which specials this run may make.
    pub fn specials(&self) -> SpecialSet {
        self.specials
    }

    /// Narrows a level's rules to what this run may actually do.
    ///
    /// A level keeps saying which specials belong on it; this only ever takes
    /// away. So a level that never wanted rockets does not get them because
    /// the player found one elsewhere.
    pub fn apply(&self, rules: &mut Rules) {
        rules.specials = rules.specials.intersect(self.specials);
    }
}

/// What a solo run is given for clearing the level at `index`.
///
/// A fixed table, which is the whole of "the solo options are fixed": these
/// are the same locations Archipelago will shuffle its own items across, filled
/// here in a set order instead. Levels past the end of the table hand over
/// nothing, which is the ordinary case once a ladder is longer than the item
/// pool.
///
/// The order follows how deliberately a player can go after each shape. Four
/// in a row is the first thing anyone makes on purpose, and the level after
/// the opener is named for it. The rocket's 2x2 turns up by accident long
/// before it is aimed at, the L and T of a cross take looking for, and five in
/// a line is the one you have to build.
pub fn solo_grant(index: usize) -> Option<Item> {
    UNLOCKS.get(index).copied().map(Item::Unlock)
}

/// What a solo run holds by the time it starts the level at `index`:
/// everything the levels below it handed over.
///
/// This is the logic function. "Can this level be cleared here" is asked of
/// the inventory this returns, and the answer has to be yes for every level on
/// the ladder or the run can dead-end.
pub fn solo_inventory(index: usize) -> Inventory {
    let mut inventory = Inventory::empty();
    for below in 0..index {
        if let Some(item) = solo_grant(below) {
            inventory.receive(item);
        }
    }
    inventory
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_run_can_make_nothing() {
        let inventory = Inventory::empty();
        assert!(inventory.specials().is_empty());

        let mut rules = Rules::default();
        assert_eq!(rules.specials, SpecialSet::ALL, "a level offers everything by default");
        inventory.apply(&mut rules);
        assert!(rules.specials.is_empty(), "and a run with nothing may make none of it");
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
        let after_first = inventory;
        assert!(!inventory.receive(Item::Unlock(Special::Rainbow)), "the second is not news");
        assert_eq!(inventory, after_first);
    }

    #[test]
    fn applying_an_inventory_only_ever_takes_away() {
        // A level that never wanted rockets must not be handed one because the
        // player found the unlock somewhere else.
        let mut inventory = Inventory::empty();
        for special in UNLOCKS {
            inventory.receive(Item::Unlock(special));
        }
        let mut rules = Rules { specials: SpecialSet { rocket: false, ..SpecialSet::ALL }, ..Rules::default() };
        inventory.apply(&mut rules);
        assert!(!rules.specials.rocket, "the level's own answer is final");
        assert!(rules.specials.rainbow, "and the rest are still on offer");
    }

    #[test]
    fn the_solo_table_places_every_unlock_exactly_once() {
        let mut placed: Vec<Special> = Vec::new();
        for index in 0..64 {
            if let Some(Item::Unlock(special)) = solo_grant(index) {
                assert!(!placed.contains(&special), "{special:?} is placed twice");
                placed.push(special);
            }
        }
        assert_eq!(placed.len(), UNLOCKS.len(), "an unlock is missing from the ladder");
        assert!(
            solo_grant(UNLOCKS.len()).is_none(),
            "the table should run out rather than repeat",
        );
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
