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
`SHOT_LEVEL` to photograph a later level. Level 1 has specials switched off, so
it never shows one.

(If you reach for `chrome --screenshot` instead, note that `--virtual-time-budget`
freezes the compositor: `requestAnimationFrame` fires two or three times, the
frame loop never runs, and you get a first frame that never advances. That is why
`shots` drives the browser over CDP in real time.) `make balance` plays every level with two bots and reports how hard each one
turned out to be. `make serve` is the one to use: a `.wasm` module cannot be loaded from a
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
in Rust, which is why the whole of the gameplay is testable without a browser,
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
made the most progress. The first is a floor (a level it clears easily is asking
nothing of anyone), and the second is close to an attentive player, and the only
fair read on positional goals like jelly, which the floor bot can only clear by
accident.

As it stands the floor bot clears level 1 96% of the time and levels 4 and up
almost never, while the attentive bot clears everything, using a tenth of its
moves on the opening levels and half to four fifths of them on the closing ones.
Real players sit somewhere between the two, so these numbers bound the
difficulty rather than fix it: the ladder still wants playtesting.

## The game

Swap two neighboring gems to line up three or more, or to close a 2x2 block. A
square is a match in its own right.

What a match leaves behind:

- **A 2x2 square** leaves a **rocket**. It falls with gravity like any other
  gem and rides out the rest of the cascade; once there is nothing left to
  clear, it flies off and takes out one other gem, picked at random for now and
  by preference later. Neither the cell it leaves nor the cell it is aimed at moves
  until impact, so the two collapse in the same drop. A rocket takes no part in
  matching while it waits: left matchable, a cascade could sweep it away before
  it ever fired, quietly costing you the reward you earned. It wears no gem's
  colors either: it belongs to no color, and tinting it like a gem would
  promise a match it will not make. The rocket is also
  the consolation prize. If the same clump earns a line gem, a cross or a
  rainbow, that is what you get instead.
- **Four in a row** leaves a gem that clears *downward*; **four in a column**
  leaves one that clears *across*. They run against the grain on purpose: you
  finish a row by sliding a gem in from above or below, so the gem you are left
  with clears the way you were moving.
- **An L or a T** leaves a gem that takes a row and a column together.
- **Five in a line** leaves a **rainbow**.

Specials are inert against ordinary gems. A line gem or a cross sits where it is
until a match of its own color sweeps it up; shoving one against a plain gem
achieves nothing.

Two specials swapped together always set each other off, each doing its own job:
a cross takes a row and a column, a line gem takes its line. The one wrinkle is
two gems facing the *same* way, which would otherwise clear the same line twice.
There, the gem the player actually moved turns and clears across its own
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

**Bricks** are the board fighting back. A brick is not a gem and not a wall: it
holds its cell the way a wall does, so nothing swaps with it and nothing falls
through it, but it can be broken. It comes whole and goes cracked, then goes.
What hits a brick is not simply "a gem cleared next to it", it is *how* that gem
came to be cleared:

- a gem taken by a match, in one of the four cells around it: **cracks** it;
- a rainbow sweeping up its color, wherever on the board that color sat, beside
  it: **cracks** it, because that is still a clear the player spent something
  on;
- a line or cross gem's beam passing *through* the brick: **cracks** it, and the
  beam carries on rather than stopping;
- that same beam merely running over a gem *beside* the brick: **nothing**. A
  beam is a line drawn across the board, and what it happens to cross is not
  something anybody matched there.

It takes one hit per clear however many gems went off beside it, since taking
one per neighbor would mean a single ordinary match wiped it out. Being nothing
but an obstacle it has no color, matches nothing, and never moves.

A **seal** is the same obstacle keyed to one color. Only that color going off
beside it counts; every other color may clear right on top of it and leave it
untouched. A beam still shoots it, because being shot is not a question of
color. That turns a blocker from something you have to *reach* into something
you have to bring the right gems to, which is a different kind of problem. It
wears its color rather than being shaped like it: a plain rounded box with rings
drawn inside, so it never reads as the gem of the same color sitting next to it.

Seals are why a level can name its palette. Dealing colors no seal answers to
dilutes the draw and turns a seal into a wall with a lie painted on it, so a
level may list exactly which colors it deals rather than taking the first few of
the palette. A test refuses any layout whose seals ask for a color the level
never deals.

The rocket is the odd one out, and deliberately. It is the only thing that can
be *aimed* at a brick rather than happening to go off near one, so it is the one
answer to a brick standing somewhere awkward. In exchange its strike is only a
strike: it takes the cell it was pointed at and does nothing to the bricks
beside it.

A rocket picks its target in tiers, and takes the best on offer:

1. Anything that moves an objective along: a gem of a color still being counted,
   a gem sitting on jelly, or a brick. Within the tier the choice is even.
2. Any ordinary gem.
3. A special, which the strike **sets off** rather than taking. That is the last
   resort by weight but the best thing that can happen, so a board down to it is
   a board about to go up. Several strikes landing on specials wait for the last
   rocket to be down and then fire together.

A score target does not count as an objective here. Everything on the board
advances score, so counting it would put every cell in the first tier and the
tiers would mean nothing. A rainbow set off this way has no color to answer to,
so it takes the most populous one, drawn at random between colors that tie.

Each level gives a fixed number of moves and one or more objectives: reach a
score, clear a number of gems of one color, peel every layer of jelly, break
every brick, or break every seal of one color. Blockers are counted in hits
rather than in blockers, the way jelly is counted in layers, so the bar moves
when one cracks instead of sitting still until it finally goes.

Per-color seal goals are not the same puzzle as one lumped total. A single count
lets a player finish by breaking whichever seals were easiest to reach; a goal
per color makes the level about bringing each color to its own, which is the
thing a seal is for. A
board with no legal move left reshuffles itself rather than ending the level,
and says so as it does: a line of text swells and fades over the board, with a
riffle to go with it. The same pop-over announces a short move budget, once,
when the count first reaches five or fewer. A level that *opens* there says so
before the first move, since with three moves to spend that is the puzzle
rather than a warning about it. It re-arms if the budget climbs back over the
line, which is what receiving moves as an Archipelago item will look like.
Gems differ in shape as well as color, so the board is readable without relying
on color alone: a blue teardrop, a yellow circle, a red triangle, an orange
headstone, a green star and a purple diamond. Every outline has its corners
rounded off, which is what keeps six different silhouettes looking like one set
rather than a bag of spikes. The palette in
[`web/js/render.js`](web/js/render.js) is the gem set in dealing order, and the
first `colors` of it is what a level plays with.

Tap a gem and then a neighbor, or swipe one toward a neighbor; both work the
same way. Progress is kept in the browser's local storage.

## Timing and effects

Animation is paced to be followed by eye rather than to get out of the way, and
the engine owns all of it: the phase lengths are the constants at the top of
[`src/game.rs`](src/game.rs).

Two of those lengths are worked out per event rather than fixed. A clear runs
until its furthest cell has popped: a blast travels outward from whatever set it
off, a cell at a time, so a row clearer sweeps along its row instead of taking
it all at once. Each cleared cell carries the delay it was given, which the page
reads to hold that cell's debris back until the blast reaches it.

The delay accumulates through a chain. A blast begins when the gem carrying it
pops, so a line gem four cells into another gem's sweep does not fire until the
sweep reaches it, and its own sweep starts from there. A clear really does
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

The landings are announced too, so the fall has a floor to hit. Gems fall on one
shared clock, so everything in a column that drops the same distance arrives at
the same instant, so the engine gathers those and raises one landing per column
per wave, carrying the moment it touches down. A row clear drops most of the
board by a row, and that is three columns settling rather than fifteen separate
impacts; a column emptied in two places lands twice, once for each depth.

A settle is not one movement, because gravity is not only downward. Gems drop
straight down as far as they can, which is one stage and animates as one fall.
Only once nothing can drop any further does anything **spill**: a gem resting on
something slides one step down and sideways into a gap beside it, left for
preference and then right, and that is a stage of its own with its own landings
and its own thuds. Whatever slid has usually opened a drop for something else,
so the two alternate until the board stops moving, and nothing is matched until
it has. A board part way through settling is not a board anybody has finished
looking at.

The column has priority, and the staging is what enforces it rather than any
test inside the spill. By the time anything spills, straight-down dropping is
exhausted, and a cell still free at that point cannot be fed from above: a gem
over it would already have dropped into it, and a clear run up to a refill mouth
would already have spawned into it. What is left is capped by a wall or a brick,
however far up the cap sits. So a gem beside an ordinary hole never dives in
sideways, and nothing built before obstacles existed changes: a property test
settles sixty randomly punched boards and checks that no gem ever leaves its own
column.

Getting there by asking whether the cell directly over a gap is blocked does not
work, and looked right for a while. The cell over a gap in the middle of a brick
shelf is itself an ordinary empty cell, so that test says the column is still
coming when the bricks two rows up mean nothing is, and the pocket never fills.

Fresh gems come from off the top of the board and nowhere else. A column with a
wall over it is not fed at all: whatever is under an overhang gets there by
spilling in from the side, one gem per stage. Before gems could spill, each run
of open cells in a column was fed from its own ceiling, because otherwise a
walled-in column stayed empty forever; that also meant gems appearing out of the
underside of a wall, which spilling makes unnecessary. It is what lets a board
be cut corner to corner and still work, with one mouth at the high end of the
slope and everything else filling by running down it.

This is also why a cell can now be empty and stay empty. A pocket under a brick
shelf fills only from the sides, and if nothing can reach it, it stays a hole.

*Landslide* is the board built to show all of this at once. It is cut corner to
corner, and the half that is cut away is the level: every cell of it is brick.
Play starts in the bottom right triangle, fed by the one cell of the top row
that is not brick, so the whole board fills by running down the slope. The
diagonal face is all that can be reached to begin with; past that it is beams
that do the work, since a beam goes through brick rather than stopping at it.
The top row is whole brick and the rest cracked, so breaking into the top row
opens a new way in, that being where gems enter.

That makes a constraint for anyone drawing a level: a cell is only ever fed from
the three cells above it, so a solid block of wall three wide leaves the middle
of the row beneath it fed by nothing at all. Put jelly there and the level
becomes unwinnable the moment that jelly is first cleared, because nothing can
ever cover it again. Pillars was exactly that until its inner corners were
changed from wall to brick, brick being wall-shaped until something breaks it
and a way through afterwards. A test walks every layout and fails on jelly
nothing can reach.
Holes cannot be swapped with, which falls out of the swap rule already requiring
a gem in both cells, and is the same reason bricks cannot. It is also why walls
are drawn as solid blocks rather than as bare panel: "nothing here" and "nothing
fits here" are now different states, and two shades of dark will not tell them
apart.

When the gems land on a match, the board holds still for a beat before it goes
off. Without the pause the thump and the pop that answers it happen in the same
frame and read as one event rather than as cause and effect. The beat is only
spent when a clear is actually waiting: a fall that ends the chain hands the
board straight back, since there is nothing to separate there and a pause would
only be input the player cannot give yet.

A rocket's flight is worked out from distance rather than given a fixed
duration. It eases up to a top speed and then holds it, so crossing the board
takes longer than going next door instead of covering the extra ground faster.
Without the cap a long shot moves too quickly to follow.

Once the last rocket is down the board holds still for a beat before gravity
answers it. Without that pause the strike and the collapse happen in the same
breath and read as a single event: the target is taken and the column above it
is already moving. The hold lets the impact land as its own thing and gives the
fall something to be a consequence of.

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
  *clipping* a path per cell per frame is what makes a phone struggle; `clip()` in
  particular is among the most expensive things a canvas can be asked to do, and
  it used to run once per gem per frame.
- **The board under the gems is painted once.** The panel and its empty sockets
  never change between resizes, so they live in their own canvas. Only jelly is
  redrawn, and only where there is jelly.
- **A board at rest is not redrawn at all.** If nothing is animating (no phase
  in progress, no particles, no hint or selection pulsing), the frame is
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

No browser will open an audio device until the player has done something, so the
game opens one on the first gesture and **keeps trying until the device is
actually running**. One attempt is not enough: a browser can accept the call and
leave the context suspended anyway. Firefox on Android does not count a gesture
as having happened until it finishes, so a context opened on `pointerdown` alone
never starts, and the game plays in silence with the button still saying the
sound is on. Taking one shot at it made that permanent for the whole session.

Sounds are data, in [`web/js/sounds.js`](web/js/sounds.js). One is a stack of
layers, each an oscillator or a burst of noise shaped by an envelope, and each
carrying its own pitch and its own offset, so a chord is several layers at one
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

A sound may declare a natural `duration`, and be played with a different one:
the holds, decays and glides scale to fit while attacks are left alone, since a
transient that stretches is not one. That is how a rocket's whistle lasts
exactly as long as its flight: the engine works the flight time out from the
distance and sends it along with the launch, so a shot across the board whistles
for longer than one next door. A layer sets `stretch: false` to stay put: an
ignition hiss is the same length however far the rocket is going.

A `scatter` holds each play back by a random moment of its own, in seconds.
Every gem in a clear asks for its shimmer at the same instant; spreading those
starts across a fifth of a second is the difference between a chime and a
twinkle.

A `harmonic` draws a random whole multiple of one root note per play, for a
filter to tune to, via `frequency: 'harmonic'`. It is a filter frequency, not a
pitch. The shimmer left behind by a cleared gem is a sawtooth held at a constant
low F with a high-resonance band-pass picking out one of its overtones, a
different one each time. That distinction is the whole sound: frequencies
scattered freely across a cascade are noise, while overtones of a single
fundamental are a chord, so twenty gems clearing at once ring together instead
of clashing.

A sound may instead be built from `chords`: a list of note lists and a single
`voice`, with a `stage` picking which chord to spread across that voice. The
clear chime is twelve chords in F, and a cascade climbs them one step per match
resolved, so a long chain walks up the scale and you can hear how well you did.
The engine stamps every clear with its place in the chain and resets that when
the board settles, so a fresh chain starts at the bottom on its own. A stage
past the end holds at the top rather than wrapping back down.

The step counts matches, not falls, which matters where the two come apart. A
rocket fires from a fall that found nothing to clear, and its flight and impact
are not a link in the chain; counting that fall would spend a step on the rocket
and land the clear its impact sets off a chord higher than it earned.

A `waver` walks a pitch glide in small steps and pushes each one slightly off,
which is how a firework fails to hold its note. Its `depth` is a fraction of the
frequency, and the useful range is far narrower than it looks: 0.022 is about a
third of a semitone either way and reads as a waver, while 0.16 is two and a
half semitones and is an unmistakable vibrato. A `sweep` on a filter glides its cutoff, so a sound gets *darker* as it fades
rather than merely quieter. That turns out to be most of the difference between
a poof and a click, along with a soft attack, since a sub-millisecond onset is
a click transient however the rest of it is shaped.

`jitter` wobbles a layer per play so that repeats do not phase into one tone,
which matters when twenty of the same sound land together.

Sounds are **scheduled on the audio clock**, not fired from a timer, using the
same per-cell delay the engine hands the renderer. A blast sweeping along a row
keeps its rhythm even if the frame loop stutters, and the pops arrive panned by
the column they came from.

`make audio` renders the sounds through an `OfflineAudioContext` (the same
graph the game plays, but exact and repeatable) and measures them:

```
pop x1       peak 0.265  tail   52ms  bright 6800 -> 2817
pop x20      peak 0.858  tail   52ms  bright 6473 -> 2857
boom         peak 0.464  tail  402ms  bright  508 ->  149
boom x2      peak 0.594  tail  395ms
thud         peak 0.173  tail   96ms  bright  314 ->  220
thud x8      peak 0.581  tail  105ms
rocket .4s   peak 0.280  tail  618ms  bright 4651 -> 4330
rocket 1.4s  peak 0.228  tail 1378ms  bright 4463 -> 1842
glide plain  peak 0.332  tail  873ms                      wobble 0.016
glide waver  peak 0.332  tail  873ms                      wobble 0.013
glide wild   peak 0.333  tail  873ms                      wobble 0.158
```

The last three are a control: the same falling glide plain, with the waver the
rocket actually ships, and with an absurd one. Note that the shipped waver
measures the same as no waver at all. That is not a bug in the waver, it is the
floor of measuring pitch through zero crossings while the frequency is being
automated. So the check proves the *mechanism* works using the exaggerated
control, and says nothing about the shipped depth, which is set by ear. It does
enforce a ceiling, because shipping a wild one is a mistake already made here
once.

It also reports how much of a sound survives a 200Hz high-pass, which is
roughly what a phone speaker throws away. A boom pitched down at 40Hz measures
loud here and arrives as silence on the device most people will play on, so the
bodies of the boom and the landing thud are deliberately kept near 190Hz and
172Hz respectively, and both are checked for what gets through.

It fails the build on several counts. If the stacks ordinary play actually asks
for (three pops, twenty sparkles, two booms, three columns landing) reach the
limiter threshold, because the limiter is a safety
net rather than part of the mix. (Twenty pops is a rainbow taking a color, four
booms at once is already unusual, and every column of the board landing on the
same instant wants a full-width clear; those rows are printed, not asserted.)
If a sound is no darker at its end than at its start, because that
means its filter sweep has stopped working and it has quietly become a click
again. If the boom drifts far from the half second it is meant to run,
or sinks so low that a phone cannot reproduce it. If the thud is not clearly
lower than a pop, since it fires while the pops that caused it are still
ringing and has to sit underneath them rather than beside them. If the low-moves
bell stops being a figure and becomes a single beep, though how many notes it
has and whether they rise or fall is left alone, because that is set by ear. And
if the waver stops wavering, or a rocket's whistle stops tracking its flight
time.

The shuffle's riffle gets a control rather than a count. Its strikes overlap
and are jittered per play, so the strike counter that checks the bell's two
notes reads anywhere from five to ten for the real thing and five for a version
with every strike piled onto one instant: it cannot tell them apart, so it is
not asked to. Instead those same strikes are rendered twice, as they ship and
all at once, and the check is that spreading them out lengthens the sound
several times over. That is what makes a riffle a riffle, and it is the only
difference between the two renders.

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
   emits an event stream (clears, specials made and fired, cascades, shuffles)
   that the page currently reads and drops; that is where sound and particles
   hook in.
