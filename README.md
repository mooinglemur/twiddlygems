# Twiddly Gems

A match-3 puzzle game that is also an Archipelago world. Sixteen levels, played
in a phone or desktop browser, on its own or as a slot in a multiworld.

The engine is a dependency-free Rust crate compiled straight to
`wasm32-unknown-unknown`. There is no wasm-bindgen, no wasm-pack and no npm:
the ABI in [`src/ffi.rs`](src/ffi.rs) is hand-written `extern "C"`, so a build
is a plain `cargo build --target wasm32-unknown-unknown` and a copy. The engine
owns the rules and the clock; the page draws what it is told and sends taps
back.

## How to play

Swap two neighboring gems to line up three or more, or to close a 2x2 square.
Tap a gem and then a neighbor, or swipe one toward it.

A bigger match leaves something behind:

| match | leaves |
|---|---|
| a 2x2 square | a **rocket**, which flies off once the cascade ends and takes one other gem |
| four in a row | a gem that clears **downward** |
| four in a column | a gem that clears **across** |
| an L or a T | a gem that takes a row and a column together |
| five in a line | a **rainbow** |

The two line clearers run against the grain on purpose: you finish a row by
sliding a gem in from above or below, so the gem you are left with clears the
way you were already moving.

A new run can make none of them. Each of the five is an **unlock** to be found,
so the early levels are plain matching and play differently when you come back
to them. Two specials swapped together set each other off. A rainbow has no
color of its own: swapped against a gem it takes that whole color, against a
clearing gem it turns every gem of that color into a copy of it, and against
another rainbow it takes the board.

**What gets in the way.** Jelly has to be cleared off the board, in one layer
or two. A brick holds its cell and never moves, but breaks if something clears
beside it or a beam passes through it. A seal is a brick keyed to one color,
and only that color clearing beside it counts, though a beam still shoots it.
That makes it something to bring the right gems to rather than something to
reach.

Each level gives a fixed number of moves and one or more objectives: reach a
score, clear so many gems of one color, clear every cell of jelly, break every
brick, or break every seal of one color. A board with no legal move on it
reshuffles rather than ending the level.

**Beating a level spends what is left of the moves.** One move per 300ms turns
a gem somewhere into a special, and when the counter reaches zero everything
inert on the board goes off at once, round after round for as long as that
keeps leaving more behind. The score climbs the whole way, which is what makes
finishing a level early worth more than merely finishing it.

Progress is kept in the browser. The hamburger menu lists the levels and offers
the way back to the title, which ends the run.

## What a run is looking for

Items are found at **locations**: clearing each level, clearing it past each of
its two score marks, reaching a chain of each length from two to twelve,
lining up exactly three to six gems in one swap, and collecting the
Archipelago gems that fall in with the refill.

What can be found:

- the **five unlocks** above,
- a **moves upgrade** per level, topping up that level's budget,
- **inventory items** to spend on the board: a rocket, a rainbow, a cross
  clear, a cluster of rockets,
- **named filler**, sixteen items that change nothing and each make one sound,
- **traps**, which happen to the board in front of you: Shuffle, Remove
  Specials, Slow.

A moves upgrade is never needed to clear anything. Every level has to be
beatable on its own budget or the ladder dead-ends, so what an upgrade buys is
a better run at a level rather than a first clear.

Solo deals its own placement from the run's seed, so every run is a different
game and the same seed is the same game. In a multiworld the items are the
room's to send.

## Building and running

The wasm build needs the `wasm32-unknown-unknown` standard library. With rustup
that is `rustup target add wasm32-unknown-unknown`. On Gentoo:

```
echo 'dev-lang/rust rust_targets_WebAssembly' >> /etc/portage/package.use/rust
emerge -1 dev-lang/rust
```

Then:

```
make serve      # build the module and serve web/ on :8080
make check      # engine tests, and an engine/front-end ABI check
make smoke      # drive the built module and the real front end from node
make shots      # play it in a headless browser, write screenshots to shots/
make audio      # render the sounds offline and measure them
make balance    # play every level with two bots and report the difficulty
make site       # the whole game as one static binary, for deployment
```

`make serve` is the way to run it: a `.wasm` module cannot be loaded from a
`file://` page, so opening `web/index.html` directly will not work.

Loading the page with `?debug` puts the engine, renderer, hud and audio on
`window.twiddlygems`. Tapping an objective twenty times in a row opens a
testing menu: unlock the ladder, unlock the specials, 99 moves, a full
inventory, spring each trap, win this level now, and a list of every filler
sound to play one at a time. It is a gesture rather than a query string so that
it works on a phone.

## The Archipelago world

The apworld holds no logic. What the items are, where they can be found and
what each place asks for are settled in [`src/progression.rs`](src/progression.rs),
because the solo game plays by them too and one game answering a question two
ways is the bug this arrangement exists to prevent. `cargo run --bin apworld`
writes them out as data; the Python package reads that file, hangs every
location off one region, and adds nothing. None of it is checked in, so there
is no second copy to go stale.

Rules cross over as rules rather than as a translation: Archipelago's own rule
builder serializes, so a `Requirement` written once in Rust arrives in Python
as the real thing. The same goes for the settings, which are one table in
[`src/options.rs`](src/options.rs) that the solo setup screen, the yaml and the
rules all read.

```
make apworld         # zip it into build/twiddlygems.apworld
make apworld-test    # our tests and Archipelago's own, against a pinned checkout
make apworld-gen     # roll a real seed from the source tree
make apworld-install # install the zip as a player would, and roll one from that
make ap-setup        # clone Archipelago and make the venv those need
```

`make ap-setup` is needed once before the three that want a real Archipelago to
run against. [`tools/twiddlygems.yaml`](tools/twiddlygems.yaml) is a player
file to roll a seed from. A seed carries the number of the generator that built
it, and the game refuses one it does not know rather than quietly playing it
wrong.

To play a slot, pick Archipelago on the title screen and give it the server,
port and slot name. The run is the room's from then on: it deals its
progression from the seed, pays its checks to the room, and keeps playing
through a disconnection, saying so over the board until it is back.

## Where things live

```
src/
  rng.rs          deterministic SplitMix64; a seed reproduces a board exactly
  rules.rs        what a variant may vary: size, colors, specials, refill, swaps
  board.rs        cells, gems, walls, jelly, bricks, and gravity
  matching.rs     run finding, special awards, and chain reactions
  level.rs        objectives and the built-in level ladder
  game.rs         input, the phase state machine, scoring, the snapshot
  progression.rs  items, locations, what holding one lets you do, and the fill
  options.rs      what a run can be set to
  session.rs      the ladder, what is unlocked, and dealing each board
  ffi.rs          the C ABI the page calls
  deflate.rs      gzip, so the static binary can serve compressed
  bin/balance.rs  difficulty measurement
  bin/apworld.rs  writes the Archipelago world out as data
  bin/serve.rs    the whole site as one static binary
web/
  index.html      the page shell
  css/            one stylesheet, phone-first
  js/engine.js    the ABI wrapper
  js/render.js    canvas drawing
  js/input.js     tap and swipe
  js/hud.js       score, objectives, the item feed, overlays
  js/sounds.js    the sounds, as data
  js/audio.js     what plays them
  js/archipelago.js  the multiworld client
  js/main.js      bootstrap and the frame loop
worlds/twiddlygems/
  __init__.py     the apworld, which reads the generated data
  test/           its tests, run against a pinned checkout
  data/           written by bin/apworld, not checked in
tools/
  check_abi.py    engine/front-end ABI consistency
  abi_smoke.mjs   drives the built module from node
  page_smoke.mjs  boots the real front end against a stubbed browser
  ap_smoke.mjs    drives the multiworld client against a scripted server
  ap_live.mjs     plays a real client against Archipelago's own server
  audio_check.mjs renders and measures the sounds
  shoot.mjs       plays the game in a real headless browser
```

Nothing links the two languages, so `make abi` compares the exports and the
shared enum numbering between `src/` and `web/js/` and fails the build on a
mismatch rather than letting it surface as a blank page.
