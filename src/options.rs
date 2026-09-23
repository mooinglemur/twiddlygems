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
    /// The fewest Archipelago gems a level carries.
    ///
    /// A floor rather than a count: a run whose items will not fit in the
    /// locations it has gets more of these until they do. See
    /// [`crate::progression::ap_gems_per_level`].
    pub ap_gems: u32,
    /// One refilled gem in this many is an Archipelago gem, while the level
    /// still has checks waiting in them.
    pub ap_gem_odds: u32,
}

impl Default for Options {
    fn default() -> Self {
        Options { goal: Goal::GoldOnLastLevel, ap_gems: 1, ap_gem_odds: 256 }
    }
}

/// What finishing means.
///
/// Worth choosing rather than fixing, because the two ends of this are very
/// different games. Clearing the last level asks for no items at all, so a
/// generator sees a run already beatable and places accordingly; gold on every
/// level asks for everything the world has. The default sits between them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Goal {
    /// Beat the last level as well as it can be beaten. The default: it wants
    /// the five unlocks and that level's moves, so a run has to find things.
    GoldOnLastLevel,
    /// Simply get to the end of the ladder. Asks for nothing, so every
    /// location in the world is open from the start.
    ClearLastLevel,
    /// Clear every level on the ladder, not just the last.
    ClearEveryLevel,
    /// Gold on every level, which is the whole game.
    GoldOnEveryLevel,
}

impl Goal {
    pub fn value(self) -> u32 {
        match self {
            Goal::GoldOnLastLevel => 0,
            Goal::ClearLastLevel => 1,
            Goal::ClearEveryLevel => 2,
            Goal::GoldOnEveryLevel => 3,
        }
    }

    pub fn from_value(value: u32) -> Option<Goal> {
        match value {
            0 => Some(Goal::GoldOnLastLevel),
            1 => Some(Goal::ClearLastLevel),
            2 => Some(Goal::ClearEveryLevel),
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
            Kind::Range { low, high } => (low..=high).contains(&value),
            Kind::Choice(choices) => choices.iter().any(|choice| choice.value == value),
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
            Kind::Range { low, high } => {
                let moved = value as i64 + by as i64;
                moved.clamp(low as i64, high as i64) as u32
            }
            Kind::Choice(choices) => {
                let at = choices.iter().position(|choice| choice.value == value).unwrap_or(0);
                let count = choices.len() as i64;
                let moved = (at as i64 + by as i64).rem_euclid(count) as usize;
                choices[moved].value
            }
        }
    }
}

/// What sort of control a setting wants.
pub enum Kind {
    /// A number between two bounds, inclusive.
    Range { low: u32, high: u32 },
    /// One of a list.
    Choice(&'static [Choice]),
}

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
        about: "What finishing the game means.",
        kind: Kind::Choice(&[
            Choice { key: "gold_on_last_level", label: "Gold on the last level", value: 0 },
            Choice { key: "clear_last_level", label: "Clear the last level", value: 1 },
            Choice { key: "clear_every_level", label: "Clear every level", value: 2 },
            Choice { key: "gold_on_every_level", label: "Gold on every level", value: 3 },
        ]),
        default: 0,
    },
    Setting {
        key: AP_GEMS,
        label: "Archipelago gems per level",
        about: "The fewest checks hidden in the gems that fall on each level. \
                A run needing more room for its items gets more of them.",
        // A floor, not a count, which is why zero is allowed: somebody who
        // wants none should get none unless their own options demand them.
        // Ten is the ceiling because ten is what the location table holds, and
        // that number is a datapackage and cannot move.
        kind: Kind::Range { low: 0, high: 10 },
        default: 1,
    },
    Setting {
        key: AP_GEM_ODDS,
        label: "How often a gem falls",
        about: "One refilled gem in this many is an Archipelago gem, while \
                the level still has checks waiting in them.",
        // A list rather than a range, because the useful values span three
        // orders of magnitude and a pair of step buttons walking one at a time
        // from 64 to 16384 is not a control anybody can use. Doubling each
        // step is how a frequency is actually thought about.
        kind: Kind::Choice(&[
            Choice { key: "one_in_64", label: "1 in 64", value: 64 },
            Choice { key: "one_in_128", label: "1 in 128", value: 128 },
            Choice { key: "one_in_256", label: "1 in 256", value: 256 },
            Choice { key: "one_in_512", label: "1 in 512", value: 512 },
            Choice { key: "one_in_1024", label: "1 in 1024", value: 1_024 },
            Choice { key: "one_in_2048", label: "1 in 2048", value: 2_048 },
            Choice { key: "one_in_4096", label: "1 in 4096", value: 4_096 },
            Choice { key: "one_in_8192", label: "1 in 8192", value: 8_192 },
            Choice { key: "one_in_16384", label: "1 in 16384", value: 16_384 },
        ]),
        // Measured rather than guessed: `make balance` plays levels out at
        // each of these and counts how long a gem takes to fall. At one in
        // 256 a gem turns up in about a third of playthroughs, so a level's
        // one check costs two or three runs at it. One in 512 was four times
        // that, which is a grind rather than a surprise, and one in 64 puts
        // one on nearly every board.
        default: 256,
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
            kind: Kind::Range { low: 1, high: 4 },
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
            kind: Kind::Range { low: 1, high: 4 },
            default: 2,
        };
        assert_eq!(span.step(4, 1), 4, "a range walked past its ceiling");
        assert_eq!(span.step(1, -1), 1, "a range walked past its floor");
        assert_eq!(span.step(2, 1), 3, "a range would not move");

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
