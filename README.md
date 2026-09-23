# Twiddly Gems

An Archipelago implementation of a set of classic match-3 puzzles.

Right now it is a solo game: thirteen levels, playable in a phone or desktop
browser with nothing to set up. The Archipelago side comes later.

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
`SHOT_LEVEL` to photograph a later level. A fresh run holds none of the special
unlocks, so level 1 never shows one whichever level is photographed.

(If you reach for `chrome --screenshot` instead, note that `--virtual-time-budget`
freezes the compositor: `requestAnimationFrame` fires two or three times, the
frame loop never runs, and you get a first frame that never advances. That is why
`shots` drives the browser over CDP in real time.) `make balance` plays every level with two bots and reports how hard each one
turned out to be. `make serve` is the one to use: a `.wasm` module cannot be loaded from a
`file://` page, so opening `web/index.html` directly will not work.

The Archipelago world has targets of its own, and they want a checkout to work
against: `make ap-setup` once, then `make apworld-test` and `make apworld-gen`.
See [The Archipelago side](#the-archipelago-side).

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

**Boards are dealt, not derived.** A run has one seed, and starting a level
draws the next board seed from it rather than deriving one from the level's
place in the ladder. So coming back to a level gives a different board, and a
layout that happens to be miserable is one restart away from a better one
instead of something to grind against. The run as a whole stays reproducible:
the same seed walked through the same levels in the same order deals the same
boards, which is what the difficulty bots and the screenshot tooling rely on.

**The page opens on a title screen**, where a mode is chosen: Solo Play, or
Archipelago once it exists. Nothing about the mode reaches the engine, which
only ever knows about a session; the frame loop simply holds the clock until a
mode is picked, so a level is not played out behind a menu. The hamburger menu
is the way back: it lists the levels and offers the title screen, which ends the
run, clears the save, and opens a fresh session on a new seed. Opening on a menu
also means the first tap of a visit happens before the board does, which is what
gets the audio device running before anything needs to make a noise.

```
src/
  rng.rs        deterministic SplitMix64; a seed reproduces a board exactly
  rules.rs      what a variant may vary: size, colors, specials, refill, swaps
  board.rs      cells, gems, walls, jelly, and gravity
  matching.rs   run finding, special awards, and chain reactions
  level.rs      objectives and the built-in level ladder
  game.rs       input, the phase state machine, scoring, the snapshot
  progression.rs  items, locations, what holding one lets you do, and where solo finds them
  options.rs    what a run can be set to, and what the settings are
  session.rs    the level ladder, what is unlocked, and dealing each board
  ffi.rs        the C ABI the page calls
web/
  index.html    the page shell
  css/          one stylesheet, phone-first
  js/engine.js  the ABI wrapper
  js/render.js  canvas drawing
  js/input.js   tap and swipe
  js/hud.js     score, objectives, the item feed, overlays
  js/main.js    bootstrap and the frame loop
  bin/balance.rs  difficulty measurement, see below
  bin/apworld.rs  writes the Archipelago world out as data
worlds/twiddlygems/
  __init__.py     the apworld, which reads that data and adds no logic
  test/           Archipelago's own tests, run against a pinned checkout
  data/           game, items, locations, options: written by bin/apworld,
                  not checked in
tools/
  check_abi.py    engine/front-end ABI consistency
  abi_smoke.mjs   drives the built module from node
  page_smoke.mjs  boots the real front end against a stubbed browser
  shoot.mjs       plays the game in a real headless browser, writes screenshots
  twiddlygems.yaml  a player file, for rolling a real seed
```

## Difficulty

Levels are tuned against two bots that bracket the range of players. One takes
the first legal move it finds, with no thought for cascades, specials or where
the jelly is; the other tries every move, plays each one out, and keeps whichever
made the most progress. The first is a floor (a level it clears easily is asking
nothing of anyone), and the second is close to an attentive player, and the only
fair read on positional goals like jelly, which the floor bot can only clear by
accident.

Each table says which run it measures, because a run deals its own
progression and two players on the same level may be holding quite different
things. **Holding nothing** is the floor, and the only state the rules promise
outright: a run can perfectly well reach the top of the ladder having found
every unlock on score marks it never went back for. **Holding everything** is
the ceiling, and it is what a score mark's rule asks for, so it is the state
the silver and gold numbers are read against. Measuring a mark against a run
that happened to hold two unlocks would only measure that run's luck.

As it stands the attentive bot clears everything holding nothing, while the
floor bot clears level 2 two thirds of the time and levels 4 and up almost
never. First Light is the exception at both ends: it is a designed three move
puzzle rather than a board to swipe at, so the attentive bot solves it every
time in two moves and the floor bot gets there 4% of the time. Real players sit
somewhere between the two, so these numbers bound the difficulty rather than
fix it: the ladder still wants playtesting.

The last table reads how far a run that has found nothing gets. A level that
wants its move items before it will go down is a fine level, and with several
move items per level it is the expected shape, so this reports rather than
judges. **The opening level is the exception, and it is a gate.** Its rule is
`Always`: until it goes down, not one location in the game is open and there is
no first item to find, so a seed where a run holding nothing cannot clear it is
a seed nobody can start. `make balance` exits non-zero on that alone.

What the rest of that table costs is a promise. A level that genuinely needs
its moves is only safe once its own rule says it needs them, because the
Archipelago generator believes those rules when it decides what it may place
behind what. Today every level's clear asks for nothing but the level below, so
that is a thing to settle when the ladder is designed rather than a thing to be
quietly wrong about.

## The game

Swap two neighboring gems to line up three or more, or to close a 2x2 block. A
square is a match in its own right.

What a match leaves behind:

- **A 2x2 square** leaves a **rocket**. It falls with gravity like any other
  gem and rides out the rest of the cascade; once there is nothing left to
  clear, it flies off and takes out one other gem, picked at random for now and
  by preference later. Neither the cell it leaves nor the cell it is aimed at moves
  until impact, so the two collapse in the same drop. A rocket waiting to
  launch is durable: it takes no part in matching, a beam crossing it goes
  straight through and clears the far side without touching it, and a rainbow
  sweeping up its color goes around it. Any of those would take away a reward
  already earned, before the rocket ever fired. It can still be spent
  deliberately, by being swapped against another special. It wears no gem's
  colors either: it belongs to no color, and tinting it like a gem would
  promise a match it will not make. The rocket is also
  the consolation prize. If the same clump earns a line gem, a cross or a
  rainbow, that is what you get instead.
- **Four in a row** leaves a gem that clears *downward*; **four in a column**
  leaves one that clears *across*. They run against the grain on purpose: you
  finish a row by sliding a gem in from above or below, so the gem you are left
  with clears the way you were moving. The two are separate switches in
  `SpecialSet`, because they will be separate unlocks. Mind which run makes
  which when reading them: `line_h` is the clearer that fires *across*, and it
  is a run of four down a *column* that earns it.
- **An L or a T** leaves a gem that takes a row and a column together.
- **Five in a line** leaves a **rainbow**.

None of which a new run can do. Each of the five is an **unlock**, and a run
that holds none of them matches and clears normally and leaves nothing behind,
which is how the opening level plays. Solo scatters them across its own run,
wherever that run's fill put them; Archipelago will scatter the same five
across a multiworld. They are separate items, including the two line clearers, so a
run can spend a while able to finish four in a row and not four in a column.
Levels themselves no longer switch specials off: a level says what belongs on
it, the run says what it may make, and a match gets whatever survives both.
That is why coming back to the opening level later plays differently.

The other item is a **moves upgrade**, which tops up one named level's budget.
What it is worth is declared by that level rather than worked out from its
length: what a board is worth coming back to better equipped is a decision
about that board, and a formula that suits a twenty move level is only guessing
at a three move one. They are never needed to clear anything: every level has
to be beatable on its own budget or the ladder dead-ends, so what they buy is a
better run at a level rather than a first clear. One arriving mid level goes
straight on the counter in front of the player, because an item that did
nothing for the level it was sent to would not be much of an item.

One item carries the whole of a level's upgrade today. Splitting it into one
item per move, so that finding four of Pillars' sixteen is worth four moves,
is the version that makes them properly progressive, and it waits on somewhere
to put them: the ladder grants 166 moves across its thirteen levels and has 50
locations besides its gems. The Archipelago gems are that somewhere.

**The Archipelago gem** is a check sitting on the board rather than a gem to
match. It has no color and matches nothing, and what collects it is anything
going off in the four cells beside it, the way a brick is broken. Swapped
against a rainbow or against another of its own kind it takes every one on the
board at once. It falls with gravity, unlike a brick, which is why it is a kind
of gem rather than a property of a cell.

It does swap with an ordinary gem, though it can never match: moving it is how
a gem stuck behind one gets where it is going, and the match the other gem
lands in often clears right beside where the Archipelago gem has just arrived,
which collects it. The one thing that has to be watched here is that every one
of them carries the same absent color, so anything reading colors raw would see
three in a row as a run of three. Both the matcher and the swap that predicts
matches go through the same `match_color`, which is where that absence lives.

They fall in with the refill, one gem in a few dozen, while the level still has
checks waiting in them, and never in the opening deal or a reshuffle: a board
that opened with one would hand over a check before the player had done
anything. A level drops no more of them than it has checks left, over the whole
playthrough rather than merely at once, so there is never one to clear that
pays nothing. Both halves of the rate are settings: how many checks a level
holds, and how often one falls. The first is a floor rather than a count,
because the options that decide how many items a run has do not know how many
places there are to put them, so when the pool outgrows everywhere else the
gems make up the difference. Ten to a level is the ceiling, and every one of
the ten is in the location table whatever a run asked for, because that table
is a datapackage and fixed for everybody.

Which gem on the board was cleared says nothing about which check it pays. A
level's gems go in order and the order belongs to the run: coming back to a
level whose first two are already checked and clearing one there takes the
third.

Items are found at **locations**: clearing each level, clearing it past each of
its two score marks, and reaching a chain of each length from two to twelve.
Which location holds which item is the fill's business rather than a table
anybody wrote, and it is dealt fresh for every run. What holds whatever the
seed decides is that every level ends up improvable at least twice over and
nothing on the ladder is cleared for nothing. A location pays once, which is
why the run writes down which it has checked, and why the save carries that
list alongside the seed: reloading has to leave a run holding what it held, and
still unable to find it again.

**A level's gold never holds that level's own moves**, since gold asks for
them, and no item is ever kept behind a chain longer than six. `make balance`
reports how deep a chain a playthrough reaches holding nothing, which is what a
chain's rule asks for, and it falls from every run at three deep to one in
twenty-five at twelve. Items live on the short ones only.

**What a location asks is a `Requirement`**, shaped to Archipelago's own rule
vocabulary rather than to anything of ours: `All` is its `And`, `Has` is its
`Has`, `Reached` its `CanReachLocation`. The engine evaluates the same trees
the apworld will, so neither side translates the other.

The ladder gates itself, and nothing more is asked to clear a level, which is
the rule that says every level must be beatable on its own budget. A score mark
asks for the five unlocks as well: nearly all of a good score comes from the
flourish, the flourish has nothing to mint without them, and logic should not
depend on a coin landing. Past level six that asks for nothing extra, since
getting there already means clearing the levels the unlocks sit on; it bites
only on the opening levels, which are exactly the ones worth coming back to.

Gold asks for that level's move items on top: it means beating a level as well
as it can be beaten, so everything that level has to offer should be in hand
first. That makes the move items progression, and progression can be found
later than the level it belongs to. Level five's moves behind level ten's clear
is an ordinary shape; what it cannot be is behind level five's own gold, and
that is a constraint on the placement rather than on the rule.

**So the solo placement is filled rather than written out.** Every item is
progression now, and hand-assigning progression is hand-solving a constraint
problem that has to keep holding as the ladder grows and the settings change. A
table that is right today quietly stops being right when either moves. So the
fill does what a generator does, in the plainest way: take everything
reachable, drop the next item into one of those, go round again. Placing only
into somewhere already reachable is what makes it safe, since the inventory
only grows and nothing can end up behind itself. Whatever is left empty
afterwards gets filler, the way a multiworld would put another world's items
there, because clearing a level and being handed nothing reads as a bug.

**It is dealt from the run's own seed**, so a solo run generates its own
progression the way a multiworld does: every run is a different game, and the
same seed is the same game. The seed goes in the save, because the save records
the ids of the locations a run checked and looks the items back up through the
fill; without it those ids would resolve in somebody else's layout. Ending a
run deals a new seed.

It is still not Archipelago's fill and does not try to be: the multiworld
shuffles across worlds and walks itself back out of corners, where this only
ever places into somewhere already open and so never has to.

A test walks the whole placement in spheres the way a generator does and fails
if anything can never be reached; it does that for thirty-two seeds against
four ladder lengths, because a fill that is safe for one seed and not another
is a run somebody cannot finish. Another checks that walk can still fail, by
giving a location a rule its own item would satisfy.

**The pool has to fit in the locations, under every setting.** A ladder of `L`
levels offers `3L` locations on the levels themselves and eleven chains, and a
test walks every combination of settings against every ladder length rather
than the defaults alone: a combination nobody can generate is a combination the
yaml should not offer. It is also the gate a one-item-per-move upgrade has to
pass before it can be offered, and today it would not. More locations, not a
cleverer fill.

**Items and locations are named by the engine**, because those strings are the
item's identity everywhere outside it: in the feed, in a tracker, in a spoiler
log. The page reads the two tables over the ABI and an item event carries a
number into them rather than any text, so there is no second set of names on
the other side to drift. For the same reason the item table is ordered by the
special's own code rather than by the order a run is given them: the numbers
end up in seeds and must not move, while the teaching order should stay free to
re-tune.

**Silver and gold** are read off the bots, and off both ends of the same level.
`make balance` prints what a mark has to clear: the score a run holding nothing
reaches, at its middle and at the far end of its tail, beside what a run
holding everything reaches. A mark is set above the bare tail and inside the
supplied range.

That is not only a matter of taste. Both marks ask for all five unlocks in
logic, so a mark a bare run reaches anyway is a location that pays for nothing
and a rule that is not true. **The opening level is gated on it**: a run with
no specials may reach one of its marks no more than once in a hundred, and
`make balance` exits non-zero otherwise. The rest of the ladder is bracketed
rather than designed, reaches its own marks bare all day, and the same column
says so; widen the gate as levels are redesigned.

First Light is the one designed so far. Three moves on a narrow board makes a
long tail: a bare run scores about 6,800 in the middle and once in a few
hundred cascades into 50,000, so its marks sit at 30,000 and 45,000, which a
supplied run reaches on 60% and 36% of its wins.

**A mark is checked the moment the score crosses it**, from the clear onward
rather than once the board has stopped. The flourish in between is still play:
the score climbs all the way through it, so a mark can be crossed in there, and
an unlock that crossing it pays for is a special the rest of the flourish can
mint. Handing it over at the end would deliver it just after the one thing it
could have changed. The score only ever climbs, so checking every frame reaches
the same marks it would have reached at the end, and reaches them in time to
matter. That is also how the multiworld behaves, which is the reason it has to
be how this behaves: an item arrives when it arrives, and one that lands during
the flourish lands on the board in front of the player.

How well a level has been beaten is read back off the checked locations rather
than recorded separately, so what is shown and what the run has found cannot
drift apart. The score in the top bar wears that color, green then silver then
gold, with the nearest mark still out of reach named beside it and nothing at
all once both are behind. It takes the better of what the level has been beaten
to before and what this attempt has reached, so it only ever moves up: a gold
level replayed for a worse score should not look as though the gold had been
taken away. This attempt counts only from the moment the goals are met, and
climbs with the flourish. The level picker washes each chip in the same color.

Logic counts level clears and nothing else. Reaching a level means clearing
every level below it, so those items are guaranteed; a chain is not, since
nobody is owed a five long one, and neither is a score mark. The claim has to
hold for the player who only ever scrapes a win.

Items arrive as events on the same stream the board raises, and the **item
feed** above the board is what reads them: two lines on a phone, four on a
wider screen, holding its height whether or not anything has arrived so the
board does not jump down the page. Each line names the item and where it came
from, the way Archipelago does, so a run reads as `Found Horizontal Line Clear
(5 Chain)`. Item and location are named separately because to a multiworld they
are separate things: the same unlock can turn up anywhere, and the same
location can be holding anything. Something the run did not find itself is
`Received`, with no location to name.

The feed reads the stream rather than asking the engine what it is holding,
which is why an item sent by a multiworld will land in it the same way one
found by clearing a level does, and the end-of-level panel takes what it says
from the same place: the two should never be able to disagree about what was
found.

**A level does not end the moment its goals are met.** It says `Level cleared`,
once, and then spends what is left: one move per 300ms, the counter visibly
running down, each one turning a gem somewhere into a special with a ring of
motes around it. When the counter reaches zero everything inert on the board
goes off at once. What that clears can leave more specials behind, and the
board coming to rest sets those off too, round after round until there is
nothing left to fire. Then a beat to look at the board before the level is
declared over, rather than a panel sliding straight over it.

The score climbs the whole way, which is the point: it is what makes finishing
a level early worth more than merely finishing it, and what a score tier will
be chased with. `Progress::moves_spare` records how many moves were in hand
when the goals were met, because by the end the counter always reads zero and
how briskly a level was beaten would otherwise be lost.

Only the three clearers that draw a line or a cross can be minted. A rainbow
takes a color off the whole board and a rocket flies somewhere else, so a
boardful of either is a wall of noise rather than a board coming apart in front
of you. And it can only hand out what the run may make, like everything else,
so a run holding none of the three goes through the same motions at the same
pace and simply leaves the board alone: a count that vanished instead would
read as the moves being taken away.

Every spend lands on a cell whether or not it places anything, and that cell
throws its motes and rings its bell either way. The spend is the event; what it
left behind is a detail. A cell that flashes and stays a plain gem says the run
had nothing to give, where a cell that does nothing at all says only that
something is broken.

The rounds reuse the ordinary clear and fall rather than adding a phase,
because a round is a detonation nobody swapped for. The two phases it does add
are for the parts that are new: spending the moves one at a time, and the beat
at the end.

**The whole flourish is one chain.** It starts over once, at the settle that
noticed the goal was met, so it opens at the bottom of the musical progression
rather than partway up whatever the player's last chain reached. Every settle
after that is a boundary between two rounds of the flourish, with nobody moving
in between, so those do not break the chain: the count climbs through to the
end instead of dropping back to the first chord each time the board comes to
rest. The score multiplier reads the same count, so it climbs too.

Most flourishes finish in a single round, which makes this easy to get wrong
and easy to appear to test. A round boundary only happens when the clears leave
new specials behind, so a test of it has to be on a seed that actually takes
several: on a one round board every version of the rule looks correct.

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
score, clear a number of gems of one color, clear every cell of jelly, break
every brick, or break every seal of one color.

**The blockers are counted in cells, not in hits.** A cell of double jelly is
one thing to finish rather than two things to count, and so is a brick that
takes two hits. Softening the one or cracking the other moves nothing on the
counter, because nothing has been finished: what the counter says is how much
is left to do. The work in between shows on the board, which is where it
belongs.

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

**A layout is an ASCII sketch**, one string per row: `.` open and `#` wall,
`o` and `O` for one layer of jelly and two, `=` and `-` for a whole brick and a
cracked one, `A` to `H` and `a` to `h` for seals of a color, and `0` to `7` for
a gem of that color placed before anything is dealt. A digit is the color's own
number, the same one a seal or an objective names, so there is one way to count
colors rather than two. Mind that `0` and `O` are different marks: a ruby and a
double layer of jelly.

A placed gem is an opening arrangement and nothing more. The deal fills around
it, but the first clear refills its cell at random like any other, because a
cell that kept dealing one color would be a different feature wearing the same
mark. Two tests hold the design side of it: a placed color the level never
deals is a gem nothing can ever match, and a layout whose own gems already form
a match opens mid-clear on every seed it will ever be played on. What the deal
cannot control it accepts: if the placed gems leave it no way to avoid an
opening match it takes one, since that resolves itself and the level carries
on. A board with no legal move on it is the one thing it will not accept, and
it deals again until there is one.

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
  redrawn, and only where there is jelly. There are two layers of it, drawn as
  two things rather than as two steps of one: a faint tint for a single layer
  and a nearly solid one for a double. A gem covers most of its cell, so a
  proportional step between them was invisible in play.
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

## The Archipelago side

The apworld holds no logic. What the items are, where they can be found and
what each place asks for first are settled in `progression.rs`, because the
solo game plays by them too, and one game answering a question two ways is the
bug this arrangement exists to prevent. `cargo run --bin apworld` writes them
out; `worlds/twiddlygems/data/` is that output, as three files: the items, the
locations with their rules, and the world itself (its name, its ladder, its
goal). Indented rather than packed, because what a location asks for is a
nested rule several deep and on one line it cannot be read.

**The settings are one table, read three ways.** `options.rs` holds what a run
can be set to; the solo screen builds its controls by walking it, the apworld
turns each entry into a real `Option` class, and the rules point at those
classes. A setting added to the engine appears on the phone, in the yaml and in
the generated documentation together, with nothing to keep in step by hand.

That last part is what makes it work rather than merely tidy. A rule can ask
for "as many as the setting says" instead of a number, and a rule can be true
only under one value of a setting, because Archipelago's `FromOption` and
`OptionFilter` both serialize. So the apworld can be generated once and still
mean something different for each player: gold asks for however many moves that
player chose, and the goal is one rule with a branch per choice, of which
exactly one is live. The engine evaluates the same trees the same way for a
solo run, from its own settings.

**The rules go over as rules.** Archipelago's rule builder serializes to dicts
and reads them back with `rule_from_dict`, so a `Requirement` written once in
Rust arrives in Python as the real thing: `All` is its `And`, `Has` is its
`Has`, `Reached` is its `CanReachLocation`, `Always` is its `True_`. The Python
package reads the file, builds one region, hangs every location off it, and
writes no logic at all. Regenerate the data and the world follows.

None of it is checked in. It is a transformation of the engine and nothing
else, so the engine is the copy worth keeping: a second one in the tree could
only ever be right or stale. Every target that needs the data writes it first,
and `make apworld` puts it in the zip.

```
make apdata          # write worlds/twiddlygems/data/
make apworld         # zip it into build/twiddlygems.apworld
make apworld-test    # Archipelago's own tests, against a pinned checkout
make apworld-gen     # roll a real seed from the source tree
make apworld-install # install the zip as a player would, and roll one from that
make ap-setup        # clone Archipelago 0.6.7 and make the venv those need
```

The last two are not the same check, and the difference bit once already. A
module inside an `.apworld` is inside a zip, with no directory to read a data
file out of, so reading the data by path worked in the checkout, passed
every test, and failed for anybody who installed the zip. It goes through the
loader now. Nothing else here can catch that, so the zip gets installed and
generated from.

`WorldTestBase` brings the three checks every world has to pass: that nothing
is reachable from nowhere, that everything is reachable with everything, and
that a real fill can be made. That last one is the completability gate, done by
Archipelago's generator rather than by ours, against the same rules the solo
placement is filled from. The tests beside them are this game's own: that the
opening level asks for nothing, that a score mark wants the unlocks, that a
gold wants two of that level's moves and is not satisfied by one.

**The goal defaults to gold on the last level**, not clearing it, and the
difference matters more than it looks. Clearing a level asks for nothing but
having reached it, so a goal of "clear the last one" is one the player holds
the moment they connect: the generator sees a game already beatable, the
playthrough comes back with no spheres in it, and every item in the world is
effectively optional. That is not a guess; it is what the first generated seed
did. Gold asks for all five unlocks and that level's own moves, so finishing
means collecting things and the spheres mean something.

Clearing the last level is still on the menu, along with clearing every level
and gold on every level, because somebody may want a relaxed slot on purpose.
What the default should not be is the one that asks for nothing.

A world submits as many items as it has locations, and this game has more
places to look than things to find: fifty locations against eighteen distinct
items. The rest is filler, and the filler is more moves on some level, the only
item here that cannot make a seed easier or harder to finish.

## Where this is going

1. **A playable solo game.** Done: mechanics, levels, objectives, and the browser
   front end.
2. **Solo progression gating and Archipelago.** Started. `progression.rs` holds
   the items, the locations and the placement between them, and `Session` walks
   it; the five specials are unlocks and each level's move budget can be topped
   up, found by clearing levels, beating their score marks and making chains in
   solo, and delivered by the multiworld later. Both sides fill the same
   `Inventory`, so "can this be cleared from here" is one question asked of one
   thing. The apworld is emitted from those same tables and generates real
   seeds. Still to come: the trap and usable items, which need somewhere to
   keep and spend them; options, since everything is fixed in this first pass;
   and the client that connects a run to a server.
3. **Polish.** Particles, sound, music, and the visual pass. The engine already
   emits an event stream (clears, specials made and fired, cascades, shuffles)
   that the page currently reads and drops; that is where sound and particles
   hook in.
