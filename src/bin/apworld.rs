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
use twiddlygems::progression::{
    item_name, item_pool, items, location_name, locations, requirement, Item, Location,
    Requirement, AP_ID_BASE, LONGEST_CHAIN, MOVES_PER_LEVEL, RELIABLE_CHAIN, SHORTEST_CHAIN,
};

/// What the game is called wherever Archipelago says its name.
const GAME: &str = "Twiddly Gems";

/// The goal, for this first pass: gold on the last level of the ladder.
///
/// Not merely clearing it, and the difference is the whole point. Clearing a
/// level asks for nothing but having reached it, so a goal of "clear the last
/// one" is a goal a player already has in hand the moment they connect: the
/// generator sees a game beatable out of an empty inventory, the playthrough
/// comes back with no spheres in it, and every item in the world is
/// effectively optional.
///
/// Gold on it asks for all five unlocks and that level's own moves, so
/// finishing means actually collecting things, and the spheres mean something.
/// It also reads as the ending it is: beat the last level as well as it can be
/// beaten. Fixed rather than an option, like everything else here so far.
fn goal(levels: usize) -> Requirement {
    Requirement::Reached(Location::LevelGold(levels - 1))
}

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
        ("moves_per_level", Json::Num(MOVES_PER_LEVEL as u32)),
        ("shortest_chain", Json::Num(SHORTEST_CHAIN)),
        ("longest_chain", Json::Num(LONGEST_CHAIN)),
        ("reliable_chain", Json::Num(RELIABLE_CHAIN)),
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
    let pool = item_pool(levels);
    Json::Arr(
        items(levels)
            .into_iter()
            .map(|item| {
                let copies = pool.iter().filter(|other| **other == item).count() as u32;
                Json::Obj(vec![
                    ("name", Json::Str(item_name(item))),
                    ("id", Json::Num(AP_ID_BASE + item.id())),
                    ("classification", Json::Str(classification(item).to_string())),
                    ("count", Json::Num(copies)),
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
                Json::Obj(vec![
                    ("name", Json::Str(location_name(at))),
                    ("id", Json::Num(AP_ID_BASE + at.id())),
                    ("rule", rule(&requirement(at, levels))),
                ])
            })
            .collect(),
    )
}

/// How much Archipelago should care about an item going missing.
///
/// Everything is progression today, which is worth saying plainly rather than
/// leaving to be inferred: the unlocks gate every score mark, and a level's
/// moves gate its gold. The traps and the usable items, when they exist, are
/// where this stops being one answer.
fn classification(item: Item) -> &'static str {
    match item {
        Item::Unlock(_) => "progression",
        Item::Moves { .. } => "progression",
    }
}

/// Whether more of this item may be made up to fill the world's empty
/// locations.
///
/// A world submits as many items as it has locations, and this game has more
/// places to look than things to find, so something has to be made up. Moves
/// are what there is: a copy past the two a level's gold asks for is pure
/// score, so it can land anywhere without making a seed easier or harder to
/// finish. An unlock is the opposite, and a second one would be a second
/// answer to a question the rules have already settled.
///
/// Said here rather than guessed at from the name on the Python side, like
/// everything else about what an item is.
fn tops_up(item: Item) -> bool {
    match item {
        Item::Unlock(_) => false,
        Item::Moves { .. } => true,
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
        Requirement::Always => ap_rule("True_", ("args", Json::Obj(vec![]))),
        Requirement::All(parts) => {
            ap_rule("And", ("children", Json::Arr(parts.iter().map(rule).collect())))
        }
        Requirement::Has { item, count } => ap_rule(
            "Has",
            (
                "args",
                Json::Obj(vec![
                    ("item_name", Json::Str(item_name(*item))),
                    ("count", Json::Num(*count)),
                ]),
            ),
        ),
        Requirement::Reached(at) => ap_rule(
            "CanReachLocation",
            (
                "args",
                Json::Obj(vec![("location_name", Json::Str(location_name(*at)))]),
            ),
        ),
    }
}

/// The wrapper every serialized rule carries, whatever it is.
fn ap_rule(name: &str, rest: (&'static str, Json)) -> Json {
    Json::Obj(vec![
        ("rule", Json::Str(name.to_string())),
        ("options", Json::Arr(vec![])),
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
