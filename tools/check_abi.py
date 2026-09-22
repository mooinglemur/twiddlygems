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


def rust_arms(source: str, function: str) -> dict[str, int]:
    """Pulls `Variant => 3,` arms out of a named function body."""
    match = re.search(rf"fn {function}\b.*?\{{(.*?)\n    \}}", source, re.S)
    if not match:
        problems.append(f"could not find fn {function} to read its codes from")
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
    rust_arms(game_source, "code"),
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
    rust_arms(read(ROOT / "src" / "progression.rs"), "code"),
    js_object(engine_source, "Tier"),
    [
        ("None", "NONE"),
        ("Clear", "CLEAR"),
        ("Silver", "SILVER"),
        ("Gold", "GOLD"),
    ],
)

compare(
    "specials",
    rust_arms(read(ROOT / "src" / "board.rs"), "code"),
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
    rust_arms(ffi_source, "tg_status"),
    js_object(engine_source, "Status"),
    [("Playing", "PLAYING"), ("Won", "WON"), ("Lost", "LOST")],
)

compare(
    "objective kinds",
    rust_arms(read(ROOT / "src" / "level.rs"), "kind_code"),
    js_object(engine_source, "ObjectiveKind"),
    [
        ("Score", "SCORE"),
        ("Color", "COLOR"),
        ("Jelly", "JELLY"),
        ("Brick", "BRICK"),
        ("Seal", "SEAL"),
    ],
)

if problems:
    print("ABI mismatch between the engine and the front end:\n", file=sys.stderr)
    for problem in problems:
        print(f"  - {problem}", file=sys.stderr)
    sys.exit(1)

print(f"ABI ok: {len(called)} of {len(exported)} exports used by the front end")
if unused:
    print(f"  not called from the front end: {', '.join(unused)}")
