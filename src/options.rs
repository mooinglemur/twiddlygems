//! How a run is set up, and what the settings are.
//!
//! The same seam as [`crate::progression`], for the same reason. A solo run is
//! meant to be a solo Archipelago run, so the two cannot have their own ideas
//! about what can be chosen or what the choices mean: the engine holds the
//! table, the solo screen draws it, and the apworld generates its yaml options
//! from it. One list, three readers.
//!
//! Two things live here. [`Options`] is what a run was set to, which is a few
//! numbers. [`SETTINGS`] is what may be set, which is everything needed to put
//! a control on a phone screen and a heading in a yaml: a name, a sentence, a
//! range or a list of choices, and a default.

/// What a run was set to.
///
/// Fixed once the run starts. Changing one is starting a different run, which
/// is why [`crate::session::Session::set_option`] deals the whole thing again
/// rather than trying to apply a change to a game in progress.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Options {
    /// What finishing the game means.
    pub goal: Goal,
    /// The fewest AP gems a level carries.
    ///
    /// A floor rather than a count: a run whose items will not fit in the
    /// locations it has gets more of these until they do. See
    /// [`crate::progression::ap_gems_per_level`].
    pub ap_gems: u32,
    /// One refilled gem in this many is an AP gem, while the level still has
    /// checks waiting in them.
    pub ap_gem_odds: u32,
    /// How many bonus items the run has to find, over all four kinds.
    ///
    /// A total rather than four counts. Which kind each one turns out to be is
    /// drawn as it is added to the pool, at equal chance, so a run leans one
    /// way or another without anybody having to say how.
    pub inventory_items: u32,
    /// Whether the ladder opens by item rather than by clearing.
    ///
    /// Off, clearing a level opens the next one, which is how the game has
    /// always worked. On, nothing past the first level is open until a
    /// Progressive Level Unlock arrives, and each one opens the next.
    pub progressive_levels: u32,
    /// How many spare level unlocks the pool carries, as a percentage.
    ///
    /// On top of the one per level the ladder actually needs. Only counts
    /// while [`Options::progressive_levels`] is on: with the ladder opening by
    /// clearing there is no such item to have spares of.
    pub spare_unlocks: u32,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            goal: Goal::ClearEveryLevel,
            ap_gems: 1,
            ap_gem_odds: 100,
            inventory_items: 10,
            progressive_levels: 1,
            spare_unlocks: 20,
        }
    }
}

/// What finishing means.
///
/// Worth choosing rather than fixing, because the two ends of this are very
/// different games. The order is how much each asks for, easiest first, and
/// the numbers follow it: they are what a save records and what the generated
/// rules switch on, so they and the choices in [`SETTINGS`] are one list said
/// twice and have to stay in step.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Goal {
    /// Simply get to the end of the ladder. On a ladder that opens by
    /// clearing this asks for nothing at all, and every location in the world
    /// is open from the start; on one that opens by item it asks for the
    /// ladder.
    ClearLastLevel,
    /// Clear every level, not just the last. The default.
    ClearEveryLevel,
    /// Beat the last level as well as it can be beaten: it wants the five
    /// unlocks and that level's moves, so a run has to find things.
    GoldOnLastLevel,
    /// Gold on every level, which is the whole game.
    GoldOnEveryLevel,
}

impl Goal {
    pub fn value(self) -> u32 {
        match self {
            Goal::ClearLastLevel => 0,
            Goal::ClearEveryLevel => 1,
            Goal::GoldOnLastLevel => 2,
            Goal::GoldOnEveryLevel => 3,
        }
    }

    pub fn from_value(value: u32) -> Option<Goal> {
        match value {
            0 => Some(Goal::ClearLastLevel),
            1 => Some(Goal::ClearEveryLevel),
            2 => Some(Goal::GoldOnLastLevel),
            3 => Some(Goal::GoldOnEveryLevel),
            _ => None,
        }
    }
}

impl Options {
    /// Reads one setting by its place in [`SETTINGS`].
    ///
    /// By index rather than by name because the ABI carries numbers, and the
    /// solo screen walks the table rather than knowing what is in it. A screen
    /// that named its controls would need editing every time a setting was
    /// added, which is the drift this whole arrangement exists to avoid.
    pub fn get(&self, at: usize) -> Option<u32> {
        match SETTINGS.get(at)?.key {
            GOAL => Some(self.goal.value()),
            AP_GEMS => Some(self.ap_gems),
            AP_GEM_ODDS => Some(self.ap_gem_odds),
            INVENTORY_ITEMS => Some(self.inventory_items),
            PROGRESSIVE_LEVELS => Some(self.progressive_levels),
            SPARE_UNLOCKS => Some(self.spare_unlocks),
            _ => None,
        }
    }

    /// Sets one, if the value is one the setting allows. Returns whether it
    /// took.
    pub fn set(&mut self, at: usize, value: u32) -> bool {
        let Some(setting) = SETTINGS.get(at) else { return false };
        if !setting.allows(value) {
            return false;
        }
        match setting.key {
            AP_GEMS => self.ap_gems = value,
            AP_GEM_ODDS => self.ap_gem_odds = value,
            INVENTORY_ITEMS => self.inventory_items = value,
            PROGRESSIVE_LEVELS => self.progressive_levels = value,
            SPARE_UNLOCKS => self.spare_unlocks = value,
            GOAL => match Goal::from_value(value) {
                Some(goal) => self.goal = goal,
                None => return false,
            },
            _ => return false,
        }
        true
    }
}

/// The key of the setting that decides [`Options::ap_gems`].
pub const AP_GEMS: &str = "ap_gems";
/// The key of the setting that decides [`Options::ap_gem_odds`].
pub const AP_GEM_ODDS: &str = "ap_gem_odds";
/// The key of the setting that decides [`Options::goal`].
pub const GOAL: &str = "goal";
/// The key of the setting that decides [`Options::inventory_items`].
pub const INVENTORY_ITEMS: &str = "inventory_items";
/// The key of the setting that decides [`Options::progressive_levels`].
pub const PROGRESSIVE_LEVELS: &str = "progressive_levels";
/// The key of the setting that decides [`Options::spare_unlocks`].
pub const SPARE_UNLOCKS: &str = "spare_unlocks";

/// One setting: everything needed to show it, check it and write it down.
pub struct Setting {
    /// What it is called in a yaml and in a save. Never shown to anybody.
    pub key: &'static str,
    /// What it is called on the screen.
    pub label: &'static str,
    /// One sentence saying what it does, for the screen and for the yaml's
    /// own documentation.
    pub about: &'static str,
    pub kind: Kind,
    pub default: u32,
}

impl Setting {
    /// Whether this is a value the setting can take.
    pub fn allows(&self, value: u32) -> bool {
        match self.kind {
            Kind::Range { low, high, .. } => (low..=high).contains(&value),
            Kind::Choice(choices) => choices.iter().any(|choice| choice.value == value),
            Kind::Toggle => value <= 1,
        }
    }

    /// The value one step along from `value`, wrapping at the end for a choice
    /// and stopping at the end for a range.
    ///
    /// The screen is a phone screen: a choice is a row of taps that cycles, and
    /// a range is a pair of buttons that stop. Which of those a setting gets is
    /// the setting's business rather than the screen's.
    pub fn step(&self, value: u32, by: i32) -> u32 {
        match self.kind {
            Kind::Range { low, high, step } => {
                let moved = value as i64 + by as i64 * step.max(1) as i64;
                moved.clamp(low as i64, high as i64) as u32
            }
            Kind::Choice(choices) => {
                let at = choices.iter().position(|choice| choice.value == value).unwrap_or(0);
                let count = choices.len() as i64;
                let moved = (at as i64 + by as i64).rem_euclid(count) as usize;
                choices[moved].value
            }
            // Wraps, like the choice it looks like on screen: a step either
            // way off one of two values lands on the other.
            Kind::Toggle => u32::from(value == 0),
        }
    }
}

/// What sort of control a setting wants.
pub enum Kind {
    /// A number between two bounds, inclusive, moved in `step`s.
    ///
    /// The step is what a tap of the button is worth, not a grid the value has
    /// to sit on: a range accepts anything between its bounds, because a yaml
    /// is free to say 137 where the buttons would only ever reach 130 or 140.
    Range { low: u32, high: u32, step: u32 },
    /// One of a list.
    Choice(&'static [Choice]),
    /// Off or on: 0 or 1, and nothing else.
    ///
    /// A choice of two would look the same on the solo screen, and on the
    /// screen it is treated as one. What it buys is the yaml: Archipelago has
    /// a type for this, and a player writing `true` into a file means it. A
    /// two-value choice would only take the words somebody wrote down here,
    /// so `on` would generate and `true` would be an error on a line nobody
    /// could see the fault in.
    Toggle,
}

/// What a toggle's two values are called on screen, since it carries no
/// choices of its own to name them.
pub const TOGGLE_LABELS: [&str; 2] = ["Off", "On"];

/// One of the values a [`Kind::Choice`] setting can take.
pub struct Choice {
    /// What it is called in a yaml. Archipelago's own convention: lower case,
    /// underscores.
    pub key: &'static str,
    /// What it is called on the screen.
    pub label: &'static str,
    pub value: u32,
}

/// The whole table.
///
/// Order matters and should be treated the way the item table is: this is what
/// the ABI indexes by and what a save records against, so settings are added
/// at the end rather than inserted.
pub static SETTINGS: &[Setting] = &[
    Setting {
        key: GOAL,
        label: "Goal",
        about: "What is necessary to finish the game",
        kind: Kind::Choice(&[
            Choice { key: "clear_last_level", label: "Clear the last level", value: 0 },
            Choice { key: "clear_every_level", label: "Clear every level", value: 1 },
            Choice { key: "gold_on_last_level", label: "Gold on the last level", value: 2 },
            Choice { key: "gold_on_every_level", label: "Gold on every level", value: 3 },
        ]),
        default: 1,
    },
    Setting {
        key: AP_GEMS,
        label: "Minimum AP gems per level",
        about: "The minimum number of AP gems that will appear in each level. \
                This is a floor. Options which create too many items in the pool \
                will create more of these locations in order to match the item \
                count.",
        // A floor, not a count, which is why zero is allowed: somebody who
        // wants none should get none unless their own options demand them.
        // Ten is the ceiling because ten is what the location table holds, and
        // that number is a datapackage and cannot move.
        kind: Kind::Range { low: 0, high: 10, step: 1 },
        default: 1,
    },
    Setting {
        key: AP_GEM_ODDS,
        label: "AP gem likelihood",
        about: "Expressed as 1/n chance of dropping in instead of a gem. \
                AP gems are never dealt to a fresh board, but will drop in \
                from the top as other gems are cleared.",
        // A range in tens rather than the doubling list this used to be. The
        // useful band turned out to be narrow enough to walk: outside 50 to
        // 200 a gem is either on every board or on hardly any, and neither end
        // is a setting anybody wants. Ten to a tap crosses it in fifteen.
        //
        // The number is the one in one-in-this-many, which the sentence above
        // is what says: the control shows 100 and the line under it reads it
        // out. `make balance` measures the other end of the same thing, in
        // playthroughs per gem.
        kind: Kind::Range { low: 50, high: 200, step: 10 },
        default: 100,
    },
    Setting {
        key: INVENTORY_ITEMS,
        label: "Inventory items",
        about: "How many total single-use items are placed in the world.",
        // A total and nothing more. Four counts, or four weights beside the
        // total, would put five controls on the setup screen for one idea; an
        // even chance says the same thing in one, and a run still comes out
        // with a mix of its own because the draw is a draw.
        //
        // The ceiling is what the leanest run has room for rather than what
        // the usual one does: the shortest ladder the tests sweep is eight
        // levels, and a run asking for no AP gems leaves that fill a fixed
        // number of places for everything it has to put down.
        // `the_pool_fits_in_the_locations_there_are` and
        // `the_solo_placement_finds_a_home_for_the_whole_pool` sweep this
        // setting at its ends and are what would say so.
        kind: Kind::Range { low: 0, high: 20, step: 1 },
        default: 10,
    },
    Setting {
        key: PROGRESSIVE_LEVELS,
        label: "Progressive level unlock",
        about: "Off: clearing a level opens the next one. On: each level \
                beyond the first one is unlocked by a Progressive Level \
                Unlock item from the item pool.",
        // On by default, which is the game this is meant to be: a ladder that
        // opens by clearing is a game where nothing in the world is needed to
        // finish it, and a run that finds its way up by items is what makes
        // both a solo run and a multiworld worth playing. Off is kept for
        // somebody who wants to play it straight through.
        kind: Kind::Toggle,
        default: 1,
    },
    Setting {
        key: SPARE_UNLOCKS,
        label: "Surplus level unlocks",
        about: "A percentage beyond those which are necessary to open every \
                level. 0 means exactly as many that are needed (the level \
                count minus 1)",
        // A percentage rather than a count, because what it is a percentage of
        // is the ladder, and the ladder grows. Rounded to the nearest whole
        // item: thirteen levels want twelve unlocks, and a fifth of twelve is
        // 2.4, which is two spares.
        //
        // Why have spares at all: the last unlock is the deepest thing in the
        // world, so a fill that has to place exactly as many as the ladder
        // needs has no slack at the bottom of it. Spares also mean a hint
        // pointing at "Progressive Level Unlock" is worth acting on more than
        // once. The ceiling is a doubling, which is past useful and cheap to
        // allow.
        kind: Kind::Range { low: 0, high: 100, step: 5 },
        default: 20,
    },
    // A level's moves upgrade has no setting of its own yet. Each level
    // declares what its upgrade is worth and one item carries the whole of it,
    // so there is nothing to choose. The choice that belongs here is whether
    // to split that total into one item per move, and it is what the setting
    // above exists to make room for.
];

/// Where to find one by key.
pub fn setting_index(key: &str) -> Option<usize> {
    SETTINGS.iter().position(|setting| setting.key == key)
}

impl Options {
    /// What one setting is set to, by key. `None` for a key no setting has,
    /// which is what a rule written against a setting that has since been
    /// removed looks like from here.
    pub fn value_of(&self, key: &str) -> Option<u32> {
        setting_index(key).and_then(|at| self.get(at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_values_the_settings_allow() {
        // A default outside its own range is a run that cannot be started, and
        // in a yaml it is a generation that fails on a file nobody edited.
        for setting in SETTINGS {
            assert!(
                setting.allows(setting.default),
                "{} defaults to {}, which it does not allow",
                setting.key,
                setting.default,
            );
        }
    }

    #[test]
    fn the_defaults_are_what_a_fresh_run_is_set_to() {
        // Two statements of one thing, so they have to agree: the table is
        // what a yaml and the solo screen read, and `Options::default` is what
        // the engine starts with.
        let fresh = Options::default();
        for (at, setting) in SETTINGS.iter().enumerate() {
            assert_eq!(
                fresh.get(at),
                Some(setting.default),
                "{} starts at something other than its default",
                setting.key,
            );
        }
    }

    #[test]
    fn every_setting_can_be_read_and_written_by_its_place_in_the_table() {
        // The ABI and the screen both work by index. A setting the table lists
        // but `get` and `set` have never heard of is a control that does
        // nothing, which is worse than a control that is missing.
        for (at, setting) in SETTINGS.iter().enumerate() {
            let mut options = Options::default();
            let other = setting.step(setting.default, 1);
            assert!(options.set(at, other), "{} refused a value it allows", setting.key);
            assert_eq!(options.get(at), Some(other), "{} did not keep what it was set to", setting.key);
        }
    }

    #[test]
    fn a_value_a_setting_does_not_allow_is_refused() {
        let goal = setting_index(GOAL).unwrap();
        let mut options = Options::default();
        assert!(!options.set(goal, 99), "the goal took a value that is not a goal");
        assert!(!options.set(SETTINGS.len(), 0), "a setting that does not exist was set");
        assert_eq!(options, Options::default(), "a refused setting changed something anyway");

        // Ranges have no instance in the table today, so the refusal at each
        // end is checked against one built here. The next setting to be added
        // is a range, and this is the behavior it will rely on.
        let span = Setting {
            key: "test",
            label: "Test",
            about: "A range, for the check below.",
            kind: Kind::Range { low: 1, high: 4, step: 1 },
            default: 2,
        };
        assert!(!span.allows(0), "a range took a value below its floor");
        assert!(!span.allows(5), "a range took a value past its ceiling");
        assert!(span.allows(1) && span.allows(4), "a range refused its own ends");
    }

    #[test]
    fn a_range_stops_at_its_ends_and_a_choice_goes_round() {
        // Which is the difference between a pair of buttons and a row of
        // chips, and the screen should not have to know which is which.
        //
        // The range is built here rather than taken from the table for the
        // reason above: there is none in it at the moment.
        let span = Setting {
            key: "test",
            label: "Test",
            about: "A range, for the check below.",
            kind: Kind::Range { low: 1, high: 4, step: 1 },
            default: 2,
        };
        assert_eq!(span.step(4, 1), 4, "a range walked past its ceiling");
        assert_eq!(span.step(1, -1), 1, "a range walked past its floor");
        assert_eq!(span.step(2, 1), 3, "a range would not move");

        // A range that moves in more than ones, which is what the gem
        // frequency wants: a band a hundred and fifty wide is not a control
        // anybody can walk one at a time. The ends still stop it dead rather
        // than letting a step overshoot them.
        let tens = Setting {
            key: "test",
            label: "Test",
            about: "A range in tens.",
            kind: Kind::Range { low: 50, high: 200, step: 10 },
            default: 100,
        };
        assert_eq!(tens.step(100, 1), 110, "a range in tens moved by one");
        assert_eq!(tens.step(100, -1), 90);
        assert_eq!(tens.step(195, 1), 200, "a step overshot the ceiling");
        assert_eq!(tens.step(50, -1), 50, "a step walked past the floor");
        assert!(tens.allows(137), "a range refused a value between its bounds");

        let goal = &SETTINGS[setting_index(GOAL).unwrap()];
        assert_eq!(goal.step(3, 1), 0, "a choice did not wrap round");
        assert_eq!(goal.step(0, -1), 3, "a choice did not wrap round backwards");
    }

    #[test]
    fn the_keys_are_unique_and_the_choices_within_a_setting_are_too() {
        // These end up in yaml files and in saves.
        let mut keys: Vec<&str> = SETTINGS.iter().map(|setting| setting.key).collect();
        let count = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), count, "two settings share a key");

        for setting in SETTINGS {
            let Kind::Choice(choices) = setting.kind else { continue };
            let mut values: Vec<u32> = choices.iter().map(|choice| choice.value).collect();
            let listed = values.len();
            values.sort_unstable();
            values.dedup();
            assert_eq!(values.len(), listed, "{} has two choices with one value", setting.key);
        }
    }

    #[test]
    fn every_goal_the_table_offers_is_one_the_engine_knows() {
        // The table is what a yaml is generated from, so a choice the engine
        // cannot turn into a goal is a yaml that generates a run nobody can
        // play.
        let goal = &SETTINGS[setting_index(GOAL).unwrap()];
        let Kind::Choice(choices) = goal.kind else { panic!("the goal is a choice") };
        for choice in choices {
            assert!(
                Goal::from_value(choice.value).is_some(),
                "the table offers {} but the engine has no such goal",
                choice.key,
            );
        }
        assert_eq!(choices.len(), 4, "a goal was added without being offered, or the other way");
    }
}
