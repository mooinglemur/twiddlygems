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
//! world out of this file and writes no logic at all.
//!
//!     cargo run --release --bin apworld > worlds/twiddlygems/game.json

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
    let ladder = levels();
    let count = ladder.len();

    let items: Vec<String> = item_table(count)
        .into_iter()
        .map(|(item, copies)| {
            object(&[
                ("name", string(&item_name(item))),
                ("id", number(AP_ID_BASE + item.id())),
                ("classification", string(classification(item))),
                ("count", number(copies)),
                ("top_up", boolean(tops_up(item))),
            ])
        })
        .collect();

    let places: Vec<String> = locations(count)
        .into_iter()
        .map(|at| {
            object(&[
                ("name", string(&location_name(at))),
                ("id", number(AP_ID_BASE + at.id())),
                ("rule", rule(&requirement(at, count))),
            ])
        })
        .collect();

    let names: Vec<String> = ladder.iter().map(|level| string(level.name)).collect();

    println!(
        "{}",
        object(&[
            ("game", string(GAME)),
            // Written so the Python side can say where this came from without
            // anyone having to remember to keep a version number in step.
            ("generated_by", string("cargo run --bin apworld")),
            ("levels", array(&names)),
            ("moves_per_level", number(MOVES_PER_LEVEL as u32)),
            ("shortest_chain", number(SHORTEST_CHAIN)),
            ("longest_chain", number(LONGEST_CHAIN)),
            ("reliable_chain", number(RELIABLE_CHAIN)),
            ("items", array(&items)),
            ("locations", array(&places)),
            ("goal", rule(&goal(count))),
        ]),
    );
}

/// Every distinct item and how many of it the pool holds.
///
/// Read off [`item_pool`] rather than worked out again here, so the apworld
/// and the solo run are filling from the same pool by construction. Anything
/// else would leave the two games subtly different lengths.
/// In the table's order rather than the pool's, because that is the order the
/// numbers are settled in. An item named but never placed would come through
/// with a count of zero, which is a thing to see rather than a thing to hide:
/// Archipelago still needs its name and number, since a seed can hand one over
/// from another world's filler.
fn item_table(levels: usize) -> Vec<(Item, u32)> {
    let pool = item_pool(levels);
    items(levels)
        .into_iter()
        .map(|item| {
            let copies = pool.iter().filter(|other| **other == item).count() as u32;
            (item, copies)
        })
        .collect()
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
fn rule(requirement: &Requirement) -> String {
    match requirement {
        Requirement::Always => ap_rule("True_", &[("args", object(&[]))]),
        Requirement::All(parts) => {
            let children: Vec<String> = parts.iter().map(rule).collect();
            ap_rule("And", &[("children", array(&children))])
        }
        Requirement::Has { item, count } => ap_rule(
            "Has",
            &[(
                "args",
                object(&[
                    ("item_name", string(&item_name(*item))),
                    ("count", number(*count)),
                ]),
            )],
        ),
        Requirement::Reached(at) => ap_rule(
            "CanReachLocation",
            &[("args", object(&[("location_name", string(&location_name(*at)))]))],
        ),
    }
}

/// The wrapper every serialized rule carries, whatever it is.
fn ap_rule(name: &str, rest: &[(&str, String)]) -> String {
    let mut fields = vec![
        ("rule", string(name)),
        ("options", array(&[])),
        ("filtered_resolution", "false".to_string()),
    ];
    fields.extend_from_slice(rest);
    object(&fields)
}

// ---- the smallest JSON writer that will do ----
//
// The engine has no dependencies and this is the only thing in the tree that
// writes JSON, so it writes it by hand. What it has to produce is a handful of
// fixed shapes, none of them nested deeply, and `make apworld-test` parses
// every one of them with a real parser before believing any of it.

/// A JSON string, with the escapes the spec insists on.
///
/// Item and location names are ours and are plain ASCII words today, but a
/// level named `The "Vault"` should not silently produce a file nothing can
/// parse.
fn string(text: &str) -> String {
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

fn number(value: u32) -> String {
    value.to_string()
}

fn boolean(value: bool) -> String {
    value.to_string()
}

fn object(fields: &[(&str, String)]) -> String {
    let body: Vec<String> =
        fields.iter().map(|(name, value)| format!("{}: {}", string(name), value)).collect();
    format!("{{{}}}", body.join(", "))
}

fn array(values: &[String]) -> String {
    format!("[{}]", values.join(", "))
}
