//! Writes the Archipelago world out as data, for the Python package to read.
//!
//! The apworld holds no logic of its own. Everything a generator needs to know
//! about this game (what the items are, where they can be found, what each
//! place asks for first) is already settled in [`twiddlygems::progression`],
//! and it has to stay settled, because the solo game answers the same
//! questions from the same tables. A second copy written in Python would be a
//! second answer, and the two would drift the first time either was edited.
//!
//! So the rules go over as rules rather than as prose. Archipelago's own rule
//! builder serializes to dicts and reads them back with `World.rule_from_dict`,
//! which means a [`Requirement`] can be emitted in its vocabulary and arrive as
//! the real thing: `All` is its `And`, `Has` is its `Has`, `Reached` is its
//! `CanReachLocation`, and `Always` is its `True_`. The Python side builds a
//! world out of these files and writes no logic at all.
//!
//!     cargo run --release --bin apworld -- worlds/twiddlygems/data

use std::fs;
use std::path::Path;

use twiddlygems::level::levels;
use twiddlygems::options::{
    Kind, Options, Setting, SETTINGS,
};
use twiddlygems::progression::{
    goal, item_name, item_pool, items, location_name, locations, requirement, Count, Item,
    Location, Requirement, AP_GEMS_PER_LEVEL, AP_ID_BASE, LONGEST_CHAIN, RELIABLE_CHAIN,
    SHORTEST_CHAIN,
};

/// What the game is called wherever Archipelago says its name.
const GAME: &str = "Twiddly Gems";

/// Where the Python package lives, which is how a rule names the option class
/// it depends on.
///
/// Archipelago's option filters and field resolvers carry a dotted import path
/// and import it, so the engine has to know where the classes it is pointing
/// at will be. The package checks this against its own `__name__` when it
/// loads, so a rename fails loudly here rather than subtly at generation.
const AP_MODULE: &str = "worlds.twiddlygems";

fn main() {
    let into = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: apworld <directory>");
        eprintln!("  writes game.json, items.json and locations.json into it");
        std::process::exit(2);
    });
    let into = Path::new(&into);
    fs::create_dir_all(into).expect("the data directory can be made");

    let ladder = levels();
    let count = ladder.len();

    write(into, "game.json", &world(&ladder, count));
    write(into, "items.json", &item_table(count));
    write(into, "locations.json", &location_table(count));
    write(into, "options.json", &option_table());
}

/// Everything a player can set, as the yaml and the solo screen both read it.
///
/// The Python turns each of these into a real `Option` class, which is what
/// the option filters in the rules point at. Nothing here is a Python
/// decision: a setting added to the engine's table appears in the yaml, in the
/// generated documentation and on the solo screen without anybody editing
/// three places.
fn option_table() -> Json {
    Json::Arr(
        SETTINGS
            .iter()
            .map(|setting| {
                let mut fields = vec![
                    ("key", Json::Str(setting.key.to_string())),
                    ("ap_class", Json::Str(ap_class(setting))),
                    ("label", Json::Str(setting.label.to_string())),
                    ("about", Json::Str(setting.about.to_string())),
                    ("default", Json::Num(setting.default)),
                ];
                match setting.kind {
                    Kind::Range { low, high } => {
                        fields.push(("kind", Json::Str("range".to_string())));
                        fields.push(("low", Json::Num(low)));
                        fields.push(("high", Json::Num(high)));
                    }
                    Kind::Choice(choices) => {
                        fields.push(("kind", Json::Str("choice".to_string())));
                        fields.push((
                            "choices",
                            Json::Arr(
                                choices
                                    .iter()
                                    .map(|choice| {
                                        Json::Obj(vec![
                                            ("key", Json::Str(choice.key.to_string())),
                                            ("label", Json::Str(choice.label.to_string())),
                                            ("value", Json::Num(choice.value)),
                                        ])
                                    })
                                    .collect(),
                            ),
                        ));
                    }
                }
                Json::Obj(fields)
            })
            .collect(),
    )
}

/// What the Python will call one setting's option class, fully qualified.
///
/// Worked out here rather than by a convention implemented on both sides: a
/// key of `gem_frequency` becomes `GemFrequency`, and the name travels in the
/// data so the two cannot disagree about it.
fn ap_class(setting: &Setting) -> String {
    let mut name = String::new();
    for word in setting.key.split('_') {
        let mut letters = word.chars();
        if let Some(first) = letters.next() {
            name.extend(first.to_uppercase());
            name.push_str(letters.as_str());
        }
    }
    format!("{AP_MODULE}.{name}")
}

/// What the world is, apart from its items and the places they hide.
fn world(ladder: &[twiddlygems::level::LevelSpec], count: usize) -> Json {
    Json::Obj(vec![
        ("game", Json::Str(GAME.to_string())),
        // Written so the Python side can say where this came from without
        // anyone having to remember to keep a version number in step.
        ("generated_by", Json::Str("cargo run --bin apworld".to_string())),
        (
            "levels",
            Json::Arr(ladder.iter().map(|level| Json::Str(level.name.to_string())).collect()),
        ),
        ("shortest_chain", Json::Num(SHORTEST_CHAIN)),
        ("longest_chain", Json::Num(LONGEST_CHAIN)),
        ("reliable_chain", Json::Num(RELIABLE_CHAIN)),
        // The ceiling on a level's gems, which is how many of them the
        // location table holds. The world needs it to know how far its own
        // floor-raising may go.
        ("ap_gems_per_level", Json::Num(AP_GEMS_PER_LEVEL)),
        ("goal", rule(&goal(count))),
    ])
}

/// Every distinct item, in the order the numbers are settled in.
///
/// How many of each comes from [`item_pool`] rather than being worked out
/// again here, so the apworld and the solo run fill from the same pool by
/// construction. An item named but never placed comes through with a count of
/// zero, which is a thing to see rather than a thing to hide: Archipelago
/// still needs its name and number, since a seed can hand one over from
/// another world.
fn item_table(levels: usize) -> Json {
    // The pool is built here too, at the default settings, only to be checked
    // against: how many of an item there are is written as a rule rather than
    // as a number, because it can depend on a setting, and the two ways of
    // saying it must not drift.
    let fresh = Options::default();
    let pool = item_pool(levels, &fresh);
    Json::Arr(
        items(levels)
            .into_iter()
            .map(|item| {
                let counted = pool.iter().filter(|other| **other == item).count() as u32;
                assert_eq!(
                    copies(item).resolve(&fresh),
                    counted,
                    "the pool holds {counted} of {} at the default settings, which is not \
                     what the table says",
                    item_name(item),
                );
                Json::Obj(vec![
                    ("name", Json::Str(item_name(item))),
                    ("id", Json::Num(AP_ID_BASE + item.id())),
                    ("classification", Json::Str(classification(item).to_string())),
                    ("count", count_json(copies(item))),
                    ("top_up", Json::Bool(tops_up(item))),
                ])
            })
            .collect(),
    )
}

/// Every place an item can be found, and what it asks for first.
fn location_table(levels: usize) -> Json {
    Json::Arr(
        locations(levels)
            .into_iter()
            .map(|at| {
                let mut fields = vec![
                    ("name", Json::Str(location_name(at))),
                    ("id", Json::Num(AP_ID_BASE + at.id())),
                    ("rule", rule(&requirement(at, levels))),
                ];
                // An Archipelago gem says where it sits in its level's
                // sequence, which is what the world reads to decide whether
                // this run has it at all: the names and numbers are the same
                // for everybody, and how many of them are in play is not.
                // Everything else has no such field, and the world takes the
                // absence of one to mean "always".
                if let Location::ApGem { index, .. } = at {
                    fields.push(("gem_index", Json::Num(index)));
                }
                Json::Obj(fields)
            })
            .collect(),
    )
}

/// How many of an item the pool holds.
///
/// One of each today: five unlocks, and one moves upgrade per level carrying
/// the whole of what that level grants. When the upgrade can be split into one
/// item per move this becomes a setting rather than a number, because the
/// apworld is generated once and read by everybody, so a count baked in here
/// would be whatever the engine happened to be built with.
fn copies(item: Item) -> Count {
    match item {
        Item::Unlock(_) => Count::Exactly(1),
        Item::Moves { .. } => Count::Exactly(1),
        // None in the pool. It is named and numbered because a run has to be
        // able to say what it was handed, and it arrives by topping up the
        // leftover locations rather than by being placed.
        Item::Filler => Count::Exactly(0),
        // None yet either, and for a different reason: how many a run carries
        // is going to be a setting, and how they are split between the kinds
        // another. Named and numbered ahead of that, because the datapackage
        // is fixed and adding a name to it later is the thing that breaks
        // seeds already rolled.
        Item::Consumable(_) => Count::Exactly(0),
    }
}

/// How much Archipelago should care about an item going missing.
///
/// Everything that is placed is progression: the unlocks gate every score
/// mark, and a level's moves gate its gold. Filler is the exception and says
/// so in its name. The traps and the usable items, when they exist, are where
/// this stops being two answers.
fn classification(item: Item) -> &'static str {
    match item {
        Item::Unlock(_) => "progression",
        Item::Moves { .. } => "progression",
        Item::Filler => "filler",
        // Worth having and needed by nothing: no location asks for one, so a
        // seed is finishable whether or not any are found, and the fill is
        // free to put them anywhere. That is what "useful" means.
        Item::Consumable(_) => "useful",
    }
}

/// Whether more of this item may be made up to fill the world's empty
/// locations.
///
/// A world submits as many items as it has locations, and this game has more
/// places to look than things to find, so something has to be made up. Only
/// one thing may be: a spare unlock would be a second answer to a question the
/// rules have settled, and a spare moves upgrade is worth nothing at all,
/// since a level's upgrade lands whole and once.
///
/// Said here rather than guessed at from the name on the Python side, like
/// everything else about what an item is.
fn tops_up(item: Item) -> bool {
    match item {
        Item::Unlock(_) => false,
        Item::Moves { .. } => false,
        Item::Filler => true,
        // Not yet. How many of these a world holds is going to be asked for
        // in the yaml, and a leftover location quietly making more of them
        // would answer that question a second time.
        Item::Consumable(_) => false,
    }
}

/// One requirement in the rule builder's own vocabulary.
///
/// The names are Archipelago's class names, because that is what
/// `rule_from_dict` looks up. `options` and `filtered_resolution` are written
/// out at their defaults: they are how a world varies a rule by its yaml
/// settings, and nothing here varies yet.
fn rule(requirement: &Requirement) -> Json {
    match requirement {
        Requirement::Always => ap_rule("True_", ("args", Json::Obj(vec![])), &[]),
        Requirement::All(parts) => {
            ap_rule("And", ("children", Json::Arr(parts.iter().map(rule).collect())), &[])
        }
        Requirement::Any(parts) => {
            ap_rule("Or", ("children", Json::Arr(parts.iter().map(rule).collect())), &[])
        }
        Requirement::Has { item, count } => ap_rule(
            "Has",
            (
                "args",
                Json::Obj(vec![
                    ("item_name", Json::Str(item_name(*item))),
                    ("count", count_json(*count)),
                ]),
            ),
            &[],
        ),
        Requirement::Reached(at) => ap_rule(
            "CanReachLocation",
            (
                "args",
                Json::Obj(vec![("location_name", Json::Str(location_name(*at)))]),
            ),
            &[],
        ),
        // The filter rides on the rule it guards rather than being a rule of
        // its own, which is how Archipelago models this: every rule can carry
        // one, and a rule whose filter does not match resolves to false.
        Requirement::When { setting, is, then } => {
            let mut filtered = rule(then);
            if let Json::Obj(fields) = &mut filtered {
                for (name, value) in fields.iter_mut() {
                    if *name == "options" {
                        *value = Json::Arr(vec![option_filter(setting, *is)]);
                    }
                }
            }
            filtered
        }
    }
}

/// How many of an item a rule asks for: a number, or a pointer at the setting
/// that decides.
fn count_json(count: Count) -> Json {
    match count {
        Count::Exactly(count) => Json::Num(count),
        Count::Setting(key) => Json::Obj(vec![
            ("resolver", Json::Str("FromOption".to_string())),
            ("option", Json::Str(class_of(key))),
            ("field", Json::Str("value".to_string())),
        ]),
    }
}

/// One entry in a rule's `options` list: this rule counts only when that
/// setting has that value.
fn option_filter(setting: &str, is: u32) -> Json {
    Json::Obj(vec![
        ("option", Json::Str(class_of(setting))),
        ("value", Json::Num(is)),
        ("operator", Json::Str("eq".to_string())),
    ])
}

/// The class path for a setting named by key, for the rules to point at.
fn class_of(key: &str) -> String {
    SETTINGS
        .iter()
        .find(|setting| setting.key == key)
        .map(ap_class)
        .unwrap_or_else(|| panic!("a rule names the setting '{key}', which does not exist"))
}

/// The wrapper every serialized rule carries, whatever it is.
fn ap_rule(name: &str, rest: (&'static str, Json), filters: &[Json]) -> Json {
    Json::Obj(vec![
        ("rule", Json::Str(name.to_string())),
        ("options", Json::Arr(filters.to_vec())),
        ("filtered_resolution", Json::Bool(false)),
        rest,
    ])
}

fn write(into: &Path, name: &str, value: &Json) {
    let mut text = String::new();
    value.write(&mut text, 0);
    text.push('\n');
    let path = into.join(name);
    fs::write(&path, &text).unwrap_or_else(|e| panic!("could not write {}: {e}", path.display()));
    println!("wrote {} ({} bytes)", path.display(), text.len());
}

// ---- the smallest JSON writer that will do ----
//
// The engine has no dependencies and this is the only thing in the tree that
// writes JSON, so it writes it by hand. What it has to produce is a handful of
// shapes, and `make apworld-test` parses every one of them with a real parser
// before believing any of it.

#[derive(Clone)]
enum Json {
    Str(String),
    Num(u32),
    Bool(bool),
    Arr(Vec<Json>),
    Obj(Vec<(&'static str, Json)>),
}

/// How far one level of nesting is indented.
const STEP: usize = 2;

impl Json {
    /// Writes this value out, indented, for reading rather than for size.
    ///
    /// Nothing here is big enough for the difference to matter, and these
    /// files are read by people: what a location asks for is a nested rule
    /// several deep, and on one line it is unreadable.
    ///
    /// Empty containers stay on their line, because a `{}` broken over three
    /// lines reads as though something is missing from it.
    fn write(&self, out: &mut String, depth: usize) {
        let pad = |out: &mut String, depth: usize| out.push_str(&" ".repeat(depth * STEP));
        match self {
            Json::Str(text) => out.push_str(&quote(text)),
            Json::Num(value) => out.push_str(&value.to_string()),
            Json::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Json::Arr(values) if values.is_empty() => out.push_str("[]"),
            Json::Arr(values) => {
                out.push_str("[\n");
                for (at, value) in values.iter().enumerate() {
                    pad(out, depth + 1);
                    value.write(out, depth + 1);
                    if at + 1 < values.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                pad(out, depth);
                out.push(']');
            }
            Json::Obj(fields) if fields.is_empty() => out.push_str("{}"),
            Json::Obj(fields) => {
                out.push_str("{\n");
                for (at, (name, value)) in fields.iter().enumerate() {
                    pad(out, depth + 1);
                    out.push_str(&quote(name));
                    out.push_str(": ");
                    value.write(out, depth + 1);
                    if at + 1 < fields.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                pad(out, depth);
                out.push('}');
            }
        }
    }
}

/// A JSON string, with the escapes the spec insists on.
///
/// Item and location names are ours and are plain ASCII words today, but a
/// level named `The "Vault"` should not silently produce a file nothing can
/// parse.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
