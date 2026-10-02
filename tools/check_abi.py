#!/usr/bin/env python3
"""Check that the browser front end and the engine agree about the ABI.

Nothing links these two sides: a renamed export or a renumbered enum compiles
cleanly on both and fails as a blank page. This reads the numbers out of each
language and compares them, so the mismatch surfaces at build time.
"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
FFI = ROOT / "src" / "ffi.rs"
JS = sorted((ROOT / "web" / "js").glob("*.js"))

problems: list[str] = []


def read(path: pathlib.Path) -> str:
    return path.read_text(encoding="utf-8")


def rust_arms(source: str, function: str, within: str | None) -> dict[str, int]:
    """Pulls `Variant => 3,` arms out of a named function body.

    `within` is the impl block to look in, or None for a free function. It has
    to be said either way rather than defaulted, because most of these
    functions are called `code` and a search of a whole file finds whichever
    type happens to be written first. When a second type in one file grew a
    `code` of its own, this quietly started reading that one and reported the
    first one's variants as missing from the engine: a true sentence about the
    wrong enum, which sends you looking in the wrong place.
    """
    if within is not None:
        block = re.search(rf"\nimpl {within} \{{(.*?)\n\}}", source, re.S)
        if not block:
            problems.append(f"could not find impl {within} to read fn {function} from")
            return {}
        source = block.group(1)
    match = re.search(rf"fn {function}\b.*?\{{(.*?)\n    \}}", source, re.S)
    if not match:
        where = f" on {within}" if within else ""
        problems.append(f"could not find fn {function}{where} to read its codes from")
        return {}
    # Arms read `Enum::Variant => 3,`, with an optional `{ .. }` or `(_)`
    # binding that must not be allowed to run past the end of its own line.
    arm = re.compile(r"^\s*\w+::(\w+)(?:\s*\{[^}\n]*\}|\s*\([^)\n]*\))?\s*=>\s*(\d+)", re.M)
    return {name: int(value) for name, value in arm.findall(match.group(1))}


def js_object(source: str, name: str) -> dict[str, int]:
    match = re.search(rf"export const {name} = \{{(.*?)\}};", source, re.S)
    if not match:
        problems.append(f"could not find exported const {name} in engine.js")
        return {}
    return {key: int(value) for key, value in re.findall(r"(\w+)\s*:\s*(\d+)", match.group(1))}


def compare(label: str, rust: dict[str, int], js: dict[str, int], pairs: list[tuple[str, str]]) -> None:
    for rust_name, js_name in pairs:
        if rust_name not in rust:
            problems.append(f"{label}: the engine no longer defines {rust_name}")
            continue
        if js_name not in js:
            problems.append(f"{label}: the front end no longer defines {js_name}")
            continue
        if rust[rust_name] != js[js_name]:
            problems.append(
                f"{label}: {rust_name} is {rust[rust_name]} in the engine "
                f"but {js[js_name]} in the front end"
            )


ffi_source = read(FFI)
engine_source = read(ROOT / "web" / "js" / "engine.js")

# 1. Every function the front end calls has to be exported.
exported = set(re.findall(r'extern "C" fn (tg_\w+)', ffi_source))
called: dict[str, pathlib.Path] = {}
for path in JS:
    for name in re.findall(r"wasm\.(tg_\w+)", read(path)):
        called.setdefault(name, path)

for name, path in sorted(called.items()):
    if name not in exported:
        problems.append(f"{path.name} calls {name}(), which the engine does not export")

unused = sorted(exported - set(called) - {"tg_create", "tg_destroy"})

# 2. Shared constants have to carry the same numbers.
event_size_rust = re.search(r"pub const EVENT_SIZE: usize = (\d+)", ffi_source)
event_size_js = re.search(r"const EVENT_SIZE = (\d+)", engine_source)
if not event_size_rust or not event_size_js:
    problems.append("could not find EVENT_SIZE on both sides")
elif event_size_rust.group(1) != event_size_js.group(1):
    problems.append(
        f"EVENT_SIZE is {event_size_rust.group(1)} in the engine "
        f"but {event_size_js.group(1)} in the front end"
    )

game_source = read(ROOT / "src" / "game.rs")
rust_flags = {
    name: int(value)
    for name, value in re.findall(r"pub const FLAG_(\w+): u8 = (\d+)", game_source)
}
compare(
    "cell flags",
    rust_flags,
    js_object(engine_source, "Flag"),
    [
        ("WALL", "WALL"),
        ("CLEARING", "CLEARING"),
        ("SELECTED", "SELECTED"),
        ("BRICK", "BRICK"),
        ("CRACKED", "CRACKED"),
        ("SEAL", "SEAL"),
    ],
)

rust_events = {
    name: int(value) for name, value in re.findall(r"pub const EV_(\w+): u8 = (\d+)", game_source)
}
compare(
    "event kinds",
    rust_events,
    js_object(engine_source, "EventKind"),
    [
        ("CLEAR", "CLEAR"),
        ("SPECIAL_MADE", "SPECIAL_MADE"),
        ("SPECIAL_FIRED", "SPECIAL_FIRED"),
        ("SWAP", "SWAP"),
        ("REVERT", "REVERT"),
        ("CASCADE", "CASCADE"),
        ("SHUFFLE", "SHUFFLE"),
        ("WON", "WON"),
        ("LOST", "LOST"),
        ("ROCKET_HIT", "ROCKET_HIT"),
        ("MATCH", "MATCH"),
        ("LAND", "LAND"),
        ("LOW_MOVES", "LOW_MOVES"),
        ("BRICK", "BRICK"),
        ("ITEM", "ITEM"),
        ("CLEARED", "CLEARED"),
        ("CASH_IN", "CASH_IN"),
    ],
)

compare(
    "phases",
    rust_arms(game_source, "code", "Phase"),
    js_object(engine_source, "Phase"),
    [
        ("Idle", "IDLE"),
        ("Swapping", "SWAPPING"),
        ("Clearing", "CLEARING"),
        ("Launching", "LAUNCHING"),
        ("Falling", "FALLING"),
        ("Shuffling", "SHUFFLING"),
        ("Finished", "FINISHED"),
        ("CashingIn", "CASHING_IN"),
        ("Finishing", "FINISHING"),
    ],
)

compare(
    "tiers",
    rust_arms(read(ROOT / "src" / "progression.rs"), "code", "Tier"),
    js_object(engine_source, "Tier"),
    [
        ("None", "NONE"),
        ("Clear", "CLEAR"),
        ("Silver", "SILVER"),
        ("Gold", "GOLD"),
    ],
)

compare(
    "item classes",
    rust_arms(read(ROOT / "src" / "progression.rs"), "code", "Class"),
    js_object(engine_source, "ItemClass"),
    [
        ("Filler", "FILLER"),
        ("Useful", "USEFUL"),
        ("Progression", "PROGRESSION"),
        ("Trap", "TRAP"),
    ],
)

compare(
    "specials",
    rust_arms(read(ROOT / "src" / "board.rs"), "code", "Special"),
    js_object(engine_source, "Special"),
    [
        ("None", "NONE"),
        ("LineH", "LINE_H"),
        ("LineV", "LINE_V"),
        ("Cross", "CROSS"),
        ("Rainbow", "RAINBOW"),
        ("Rocket", "ROCKET"),
    ],
)

compare(
    "level status",
    rust_arms(ffi_source, "tg_status", None),
    js_object(engine_source, "Status"),
    [("Playing", "PLAYING"), ("Won", "WON"), ("Lost", "LOST")],
)

compare(
    "objective kinds",
    rust_arms(read(ROOT / "src" / "level.rs"), "kind_code", "Objective"),
    js_object(engine_source, "ObjectiveKind"),
    [
        ("Score", "SCORE"),
        ("Color", "COLOR"),
        ("Jelly", "JELLY"),
        ("Brick", "BRICK"),
        ("Seal", "SEAL"),
    ],
)

# A consumable's code is its place in one array rather than a match arm, so
# this reads the array. Its order is the order the item ids are built from and
# the order the front end's inventory sits in, which means a reordering would
# hand a saved Rainbow back as a Rocket without either side failing to build.
progression_source = read(ROOT / "src" / "progression.rs")
listed = re.search(r"pub const CONSUMABLES: \[Consumable; \d+\] =\s*\[(.*?)\];", progression_source, re.S)
if not listed:
    problems.append("could not find the CONSUMABLES array to read the consumable codes from")
else:
    rust_consumables = {
        name: at for at, name in enumerate(re.findall(r"Consumable::(\w+)", listed.group(1)))
    }
    compare(
        "consumables",
        rust_consumables,
        js_object(engine_source, "Consumable"),
        [
            ("Rocket", "ROCKET"),
            ("Rainbow", "RAINBOW"),
            ("CrossClear", "CROSS_CLEAR"),
            ("RocketCluster", "ROCKET_CLUSTER"),
        ],
    )

# The named filler is a list of names in the engine and a list of sounds in the
# sound bank, joined by nothing but position: an item event carries the noise's
# code and the page uses it to index `NOISES`. So the two orders have to match
# exactly, and the names are carried in the JS table for no reason except to be
# compared here.
#
# This is worth checking rather than trusting because the failure is silent in
# every direction. A noise inserted in the middle of the engine's list, or a
# sound moved in the sound bank, leaves both sides building, every item still
# arriving, every name in the feed still correct, and every sound belonging to
# the item next door.
# Scoped to `impl Noise`, not searched across the file. `Trap` grew a `name`
# of its own and sits above this one, so an unscoped search found the traps,
# read no `Noise::` arms out of them and said the names could not be read.
# Exactly what `rust_arms` takes a `within` for, and the same mistake it
# already documents having made once with `code`.
noise_impl = re.search(r"\nimpl Noise \{(.*?)\n\}", progression_source, re.S)
rust_noises = (
    re.search(r"pub fn name\(self\) -> &'static str \{(.*?)\n    \}", noise_impl.group(1), re.S)
    if noise_impl
    else None
)
js_noises = re.search(r"export const NOISES = \[(.*?)\n\];", read(ROOT / "web" / "js" / "sounds.js"), re.S)
if not noise_impl:
    problems.append("could not find impl Noise to read the filler item names from")
elif not rust_noises:
    problems.append("could not find Noise::name to read the filler item names from")
elif not js_noises:
    problems.append("could not find the NOISES table in sounds.js")
else:
    engine_names = re.findall(r'Noise::\w+ => "([^"]*)"', rust_noises.group(1))
    # Single-quoted or double-quoted, since one of these has an apostrophe in
    # it and prettier flips the quotes around to suit.
    page_names = re.findall(r"""^\s*(?:\{\s*)?name: (?:'([^']*)'|"([^"]*)")""", js_noises.group(1), re.M)
    page_names = [single or double for single, double in page_names]
    if not engine_names:
        problems.append("Noise::name has no arms, so the filler item names could not be read")
    elif engine_names != page_names:
        extra = [name for name in page_names if name not in engine_names]
        missing = [name for name in engine_names if name not in page_names]
        if extra or missing:
            problems.append(
                f"named filler: the engine has {len(engine_names)} and the sound bank "
                f"{len(page_names)}; the engine is missing {extra or 'nothing'} and the "
                f"sound bank is missing {missing or 'nothing'}"
            )
        else:
            at = next(
                i for i, (a, b) in enumerate(zip(engine_names, page_names)) if a != b
            )
            problems.append(
                f"named filler: the two lists hold the same names in different orders, "
                f"first differing at {at}, where the engine says {engine_names[at]!r} and "
                f"the sound bank says {page_names[at]!r}. Every item from there on would "
                f"play the wrong sound."
            )

if problems:
    print("ABI mismatch between the engine and the front end:\n", file=sys.stderr)
    for problem in problems:
        print(f"  - {problem}", file=sys.stderr)
    sys.exit(1)

print(f"ABI ok: {len(called)} of {len(exported)} exports used by the front end")
if unused:
    print(f"  not called from the front end: {', '.join(unused)}")
