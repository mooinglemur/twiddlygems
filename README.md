# Twiddly Gems

An Archipelago implementation of a set of classic match-3 puzzles.

Right now it is a solo game: ten levels, playable in a phone or desktop browser
with nothing to set up. The Archipelago side comes later.

## Building and playing

The wasm build needs the `wasm32-unknown-unknown` standard library. On Gentoo:

```
echo 'dev-lang/rust rust_targets_WebAssembly' >> /etc/portage/package.use/rust
emerge -1 dev-lang/rust
```

With rustup it is `rustup target add wasm32-unknown-unknown`.

Then:

```
make check    # engine tests, plus an engine/front-end ABI check
make serve    # builds the wasm module and serves web/ on :8080
make smoke    # drives the built module from node, no browser needed
```

`make smoke` runs two headless checks: one drives the module's ABI directly, and
one boots the actual front end against a stubbed-out browser and plays a move
through it. Neither knows what the board looks like, but between them they catch
the faults that leave a blank page. `make shots` goes further and plays the game
in a real headless Chrome over the DevTools Protocol, at phone and desktop sizes,
writing screenshots to `shots/` and failing if the page threw anything.

(If you reach for `chrome --screenshot` instead, note that `--virtual-time-budget`
freezes the compositor: `requestAnimationFrame` fires two or three times, the
frame loop never runs, and you get a first frame that never advances. That is why
`shots` drives the browser over CDP in real time.) `make balance` plays every level with two bots and reports how hard each one
turned out to be. `make serve` is the one to use — a `.wasm` module cannot be loaded from a
`file://` page, so opening `web/index.html` directly will not work.

## How it fits together

The engine is a dependency-free Rust crate compiled straight to
`wasm32-unknown-unknown`. There is no `wasm-bindgen` and no `wasm-pack`: the ABI
in [`src/ffi.rs`](src/ffi.rs) is hand-written `extern "C"`, so a build is a plain
`cargo build --target wasm32-unknown-unknown` with nothing to fetch and no
version pairing between a crate and a CLI tool.

**The engine owns the rules and the clock.** The front end calls `tg_update` with
elapsed milliseconds and then asks what to draw. Every decision about when a swap
lands, when a cascade fires, what a match is worth and when a level ends is made
in Rust, which is why the whole of the gameplay is testable without a browser —
including a bot that plays every level asserting board invariants as it goes.

**The boundary is cheap on purpose.** Board state is written into two buffers the
page reads as typed arrays straight out of wasm memory: four bytes per cell
(color, special, jelly, flags) and three floats per cell (x offset, y offset,
scale). Drawing a frame costs a handful of calls rather than one per cell. The
offsets are the animation: the board is always in its settled logical state, and
a falling gem is simply drawn some distance above where it already is.

Because nothing links the two languages, `make abi` compares the exports and the
shared enum numbering between `src/` and `web/js/` and fails the build on a
mismatch, rather than letting it surface as a blank page.

```
src/
  rng.rs        deterministic SplitMix64; a seed reproduces a board exactly
  rules.rs      what a variant may vary: size, colors, specials, refill, swaps
  board.rs      cells, gems, walls, jelly, and gravity
  matching.rs   run finding, special awards, and chain reactions
  level.rs      objectives and the built-in level ladder
  game.rs       input, the phase state machine, scoring, the snapshot
  session.rs    the level ladder and what is unlocked
  ffi.rs        the C ABI the page calls
web/
  index.html    the page shell
  css/          one stylesheet, phone-first
  js/engine.js  the ABI wrapper
  js/render.js  canvas drawing
  js/input.js   tap and swipe
  js/hud.js     score, objectives, overlays
  js/main.js    bootstrap and the frame loop
  bin/balance.rs  difficulty measurement, see below
tools/
  check_abi.py    engine/front-end ABI consistency
  abi_smoke.mjs   drives the built module from node
  page_smoke.mjs  boots the real front end against a stubbed browser
  shoot.mjs       plays the game in a real headless browser, writes screenshots
```

## Difficulty

Levels are tuned against two bots that bracket the range of players. One takes
the first legal move it finds, with no thought for cascades, specials or where
the jelly is; the other tries every move, plays each one out, and keeps whichever
made the most progress. The first is a floor — a level it clears easily is asking
nothing of anyone — and the second is close to an attentive player, and the only
fair read on positional goals like jelly, which the floor bot can only clear by
accident.

As it stands the floor bot clears level 1 96% of the time and levels 4 and up
almost never, while the attentive bot clears everything, using a tenth of its
moves on the opening levels and half to four fifths of them on the closing ones.
Real players sit somewhere between the two, so these numbers bound the
difficulty rather than fix it: the ladder still wants playtesting.

## The game

Swap two neighboring gems to line up three or more. Runs of four leave a gem that
clears its row or column, an L or a T leaves a bomb that takes out a 3x3 block,
and five in a line leaves a rainbow that clears every gem of whatever color it is
swapped against. Specials caught in a blast set each other off, and swapping two
specials together combines them — two line gems cross, two bombs make a wider
crater, a bomb and a line make a three-wide cross, and two rainbows take the
board.

Each level gives a fixed number of moves and one or more objectives: reach a
score, clear a number of gems of one color, or peel every layer of jelly. A board
with no legal move left reshuffles itself rather than ending the level. Gems
differ in shape as well as color, so the board is readable without relying on
color alone.

Tap a gem and then a neighbor, or swipe one toward a neighbor; both work the same
way. Progress is kept in the browser's local storage.

## Where this is going

1. **A playable solo game.** Done: mechanics, levels, objectives, and the browser
   front end.
2. **Solo progression gating and Archipelago.** `Session` is the seam: today it
   unlocks the next level on a win, and it is where received items will decide
   what may be played instead.
3. **Polish.** Particles, sound, music, and the visual pass. The engine already
   emits an event stream — clears, specials made and fired, cascades, shuffles —
   that the page currently reads and drops; that is where sound and particles
   hook in.
