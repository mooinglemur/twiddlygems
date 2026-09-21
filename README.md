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

Loading the page with `?debug` puts the engine and renderer on
`window.twiddlygems`, which is how the animation timings get inspected from the
console or from a screenshot script.

`make smoke` runs two headless checks: one drives the module's ABI directly, and
one boots the actual front end against a stubbed-out browser and plays a move
through it. Neither knows what the board looks like, but between them they catch
the faults that leave a blank page. `make shots` goes further and plays the game
in a real headless Chrome over the DevTools Protocol, at phone and desktop sizes,
writing screenshots to `shots/` and failing if the page threw anything. Set
`SHOT_LEVEL` to photograph a later level — level 1 has specials switched off, so
it never shows one.

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

Swap two neighboring gems to line up three or more, or to close a 2x2 block — a
square is a match in its own right.

What a match leaves behind:

- **A 2x2 square** leaves a **rocket**. It falls with gravity like any other
  gem and rides out the rest of the cascade; once there is nothing left to
  clear, it flies off and takes out one other gem — picked at random for now, by
  preference later. Neither the cell it leaves nor the cell it is aimed at moves
  until impact, so the two collapse in the same drop. A rocket takes no part in
  matching while it waits: left matchable, a cascade could sweep it away before
  it ever fired, quietly costing you the reward you earned. It wears no gem's
  colors either — it belongs to no color, and tinting it like a gem would
  promise a match it will not make. The rocket is also
  the consolation prize — if the same clump earns a line gem, a cross or a
  rainbow, that is what you get instead.
- **Four in a row** leaves a gem that clears *downward*; **four in a column**
  leaves one that clears *across*. They run against the grain on purpose — you
  finish a row by sliding a gem in from above or below, so the gem you are left
  with clears the way you were moving.
- **An L or a T** leaves a gem that takes a row and a column together.
- **Five in a line** leaves a **rainbow**.

Specials are inert against ordinary gems. A line gem or a cross sits where it is
until a match of its own color sweeps it up; shoving one against a plain gem
achieves nothing.

Two specials swapped together always set each other off, each doing its own job:
a cross takes a row and a column, a line gem takes its line. The one wrinkle is
two gems facing the *same* way, which would otherwise clear the same line twice
— there, the gem the player actually moved turns and clears across its own
grain, so the pair still takes a row and a column.

The rainbow answers to anything, because it has no match of its own to wait for.
Like a rocket, it is an item sitting on the board rather than a gem in the pool
of colors: it lines up with nothing, wears no gem's colors, and cannot be swept
up by a match. A swap is its way out, or another special catching it.

- against an ordinary gem it clears that whole color, setting off any clearing
  gems standing in it;
- against a clearing gem, every gem of that color becomes a copy of that gem and
  they all fire at once;
- against another rainbow, the board goes.

Each level gives a fixed number of moves and one or more objectives: reach a
score, clear a number of gems of one color, or peel every layer of jelly. A
board with no legal move left reshuffles itself rather than ending the level.
Gems differ in shape as well as color, so the board is readable without relying
on color alone.

Tap a gem and then a neighbor, or swipe one toward a neighbor; both work the
same way. Progress is kept in the browser's local storage.

## Timing and effects

Animation is paced to be followed by eye rather than to get out of the way, and
the engine owns all of it — the phase lengths are the constants at the top of
[`src/game.rs`](src/game.rs).

Two of those lengths are worked out per event rather than fixed. A clear runs
until its furthest cell has popped: a blast travels outward from whatever set it
off, a cell at a time, so a row clearer sweeps along its row instead of taking
it all at once. Each cleared cell carries the delay it was given, which the page
reads to hold that cell's debris back until the blast reaches it.

The delay accumulates through a chain. A blast begins when the gem carrying it
pops, so a line gem four cells into another gem's sweep does not fire until the
sweep reaches it, and its own sweep starts from there — a clear really does
travel across the board rather than happening everywhere at once. An ordinary
match has no direction to travel in and pops as one.

A rainbow is the exception in the other direction: the color it takes is
scattered all over the board with no path between the cells, so those go off at
random within a window instead of sweeping.

Falling works the same way. Gems accelerate and then stop gaining speed, so a
gem dropping the height of the board takes longer than one dropping a single
row, and the fall lasts as long as the gem with furthest to go needs. Filling a
hole in constant time regardless of depth is the tell-tale sign of a board that
does not have gravity so much as a scheduled animation.

A rocket's flight is worked out from distance rather than given a fixed
duration. It eases up to a top speed and then holds it, so crossing the board
takes longer than going next door instead of covering the extra ground faster —
without the cap a long shot moves too quickly to follow.

Every cleared cell throws off shards in its own color and a puff of smoke. A
rocket strike raises an event of its own on top of the ordinary clear, and gets
a much heavier burst with a blast ring, because it is the loudest thing that
happens on the board. The engine emits the events;
[`web/js/render.js`](web/js/render.js) owns the particles, capped so that
clearing a whole color cannot bury a phone.

## Drawing it quickly

A phone is fill-rate bound long before it is logic bound, and Firefox on mobile
is markedly less forgiving of canvas work than Chromium is. Three things keep
the frame cheap:

- **Gems are cached bitmaps, not paths.** Each color and special is painted once
  into an offscreen canvas and blitted from then on. Filling, stroking and
  *clipping* 64 paths a frame is what makes a phone struggle; `clip()` in
  particular is among the most expensive things a canvas can be asked to do, and
  it used to run once per gem per frame.
- **The board under the gems is painted once.** The panel and its empty sockets
  never change between resizes, so they live in their own canvas. Only jelly is
  redrawn, and only where there is jelly.
- **A board at rest is not redrawn at all.** If nothing is animating — no phase
  in progress, no particles, no hint or selection pulsing — the frame is
  skipped, which is most of what the page was previously being asked to do.

The backing store is also capped at 2x device pixels. Past that the extra pixels
buy nothing visible and cost the square of the ratio.

Measured in headless Chromium with the CPU throttled 8x, as a stand-in for a
slow device: 12.4fps before these changes, 42.4fps after, with per-frame draw
time falling from 3.4ms to 0.9ms.

## Sound

Effects are synthesized in the browser rather than shipped as files: nothing to
download, no codec to negotiate (Safari has never taken Ogg, which narrows the
universal set to MP3, AAC and WAV), and a sound can be pitched per play instead
of shipping variants. A sample set would have been several times the size of the
game.

Sounds are data, in [`web/js/sounds.js`](web/js/sounds.js). One is a stack of
layers, each an oscillator or a burst of noise shaped by an envelope, and each
carrying its own pitch and its own offset — so a chord is several layers at one
moment and an arpeggio is the same layers a few milliseconds apart:

```js
pop: {
  gain: 0.5,
  layers: [
    {
      source: 'noise',
      filters: [
        { type: 'highpass', frequency: 340, q: 0.6 },
        { type: 'lowpass', frequency: 4200, q: 0.9, sweep: { to: 680, time: 0.075 } },
      ],
      env: { attack: 0.007, decay: 0.08 },
      jitter: { frequency: 0.18, gain: 0.25 },
    },
  ],
}
```

A sound may declare a natural `duration`, and be played with a different one —
the holds, decays and glides scale to fit while attacks are left alone, since a
transient that stretches is not one. That is how a rocket's whistle lasts
exactly as long as its flight: the engine works the flight time out from the
distance and sends it along with the launch, so a shot across the board whistles
for longer than one next door. A layer sets `stretch: false` to stay put — an
ignition hiss is the same length however far the rocket is going.

A `waver` walks a pitch glide in small steps and pushes each one slightly off,
which is how a firework fails to hold its note. Its `depth` is a fraction of the
frequency, and the useful range is far narrower than it looks: 0.022 is about a
third of a semitone either way and reads as a waver, while 0.16 is two and a
half semitones and is an unmistakable vibrato. A `sweep` on a filter glides its cutoff, so a sound gets *darker* as it fades
rather than merely quieter. That turns out to be most of the difference between
a poof and a click — along with a soft attack, since a sub-millisecond onset is
a click transient however the rest of it is shaped.

`jitter` wobbles a layer per play so that repeats do not phase into one tone,
which matters when twenty of the same sound land together.

Sounds are **scheduled on the audio clock**, not fired from a timer, using the
same per-cell delay the engine hands the renderer. A blast sweeping along a row
keeps its rhythm even if the frame loop stutters, and the pops arrive panned by
the column they came from.

`make audio` renders the sounds through an `OfflineAudioContext` — the same
graph the game plays, but exact and repeatable — and measures them:

```
pop x1       peak 0.104  tail   61ms  bright 4731 -> 2954
pop x20      peak 0.290  tail   72ms  bright 3628 -> 2787
boom         peak 0.330  tail  505ms  bright  291 ->  149
boom x2      peak 0.325  tail  518ms
rocket .5s   peak 0.221  tail  437ms  bright 4636 -> 1612
rocket 1.4s  peak 0.214  tail 1393ms  bright 4186 -> 1658
glide plain  peak 0.493  tail  888ms                      wobble 0.016
glide waver  peak 0.493  tail  888ms                      wobble 0.016
glide wild   peak 0.493  tail  888ms                      wobble 0.212
```

The last three are a control: the same falling glide plain, with the waver the
rocket actually ships, and with an absurd one. Note that the shipped waver
measures the same as no waver at all. That is not a bug in the waver, it is the
floor of measuring pitch through zero crossings while the frequency is being
automated — so the check proves the *mechanism* works using the exaggerated
control, and says nothing about the shipped depth, which is set by ear. It does
enforce a ceiling, because shipping a wild one is a mistake already made here
once.

It also reports how much of a sound survives a 200Hz high-pass, which is
roughly what a phone speaker throws away. A boom pitched down at 40Hz measures
loud here and arrives as silence on the device most people will play on, so the
body of that one is deliberately kept near 190Hz falling to 72Hz — 89% of it
gets through.

It fails the build on several counts. If the sounds designed to pile up — twenty
pops, two booms — reach the limiter threshold, because the limiter is a safety
net rather than part of the mix. (Four booms at once may well engage it, and
that is what it is for.) If a sound is no darker at its end than at its start, because that
means its filter sweep has stopped working and it has quietly become a click
again. If the boom drifts far from the half second it is meant to run,
or sinks so low that a phone cannot reproduce it. And if the waver stops
wavering, or a rocket's whistle stops tracking its flight time.

What none of this can tell you is whether a sound is any good. Levels,
durations and brightness are measurable; character is not. Listen, then edit
`sounds.js`.

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
