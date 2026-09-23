#!/usr/bin/env node
// Boots the real front end against the real wasm module, with the browser
// stubbed out just far enough to run it.
//
// This is not a substitute for opening the page: nothing here knows what the
// board looks like. What it does catch is the whole class of faults that turn
// the page blank (a bad import, a misspelled DOM id, a canvas call that does
// not exist, an exception in the frame loop), none of which the engine's own
// tests can see.
//
//   make page

import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import path from 'node:path';

const WASM = process.argv[2] ?? 'web/twiddlygems.wasm';
const FRAMES = 120;

// ---- the stub browser ----

const calls = new Proxy({}, { get: (target, key) => target[key] ?? 0 });
const context2d = stubContext();
const listeners = new Map();
const elements = new Map();
let canvas;

function stubContext() {
  const ctx = {};
  const methods = [
    'setTransform', 'clearRect', 'save', 'restore', 'beginPath', 'closePath', 'moveTo',
    'lineTo', 'arc', 'arcTo', 'ellipse', 'quadraticCurveTo', 'fill', 'stroke', 'clip',
    'fillRect', 'strokeRect', 'translate', 'scale', 'rotate', 'drawImage', 'fillText',
  ];
  for (const name of methods) {
    ctx[name] = () => {
      calls[name] = (calls[name] ?? 0) + 1;
    };
  }
  ctx.createLinearGradient = () => ({ addColorStop() {} });
  ctx.createRadialGradient = () => ({ addColorStop() {} });
  // Enough for the pop-over to size its plate. The number is nonsense; what is
  // being checked is that the call exists and the drawing runs.
  ctx.measureText = (text) => {
    calls.measureText = (calls.measureText ?? 0) + 1;
    return { width: String(text).length * 8 };
  };
  return ctx;
}

function stubElement(id) {
  const element = {
    id,
    children: [],
    style: {},
    textContent: '',
    disabled: false,
    type: '',
    className: '',
    classList: {
      set: new Set(),
      add(name) { this.set.add(name); },
      remove(name) { this.set.delete(name); },
      contains(name) { return this.set.has(name); },
      toggle(name, force) {
        const on = force ?? !this.set.has(name);
        if (on) { this.set.add(name); } else { this.set.delete(name); }
        return on;
      },
    },
    append(...nodes) {
      for (const node of nodes) {
        if (node && typeof node === 'object') {
          node.parentNode = this;
        }
      }
      this.children.push(...nodes);
    },
    replaceChildren(...nodes) {
      for (const node of nodes) {
        if (node && typeof node === 'object') {
          node.parentNode = this;
        }
      }
      this.children = nodes;
    },
    // Really removes. A stub that quietly did nothing would turn the feed's
    // "drop the oldest line until it fits" loop into a hang.
    remove() {
      const siblings = this.parentNode?.children;
      const at = siblings?.indexOf(this) ?? -1;
      if (at >= 0) {
        siblings.splice(at, 1);
      }
      this.parentNode = null;
    },
    scrollHeight: 0,
    scrollTop: 0,
    // Kept on the element as well as in the global map: every element the page
    // creates shares the id `created:<tag>`, so a button in the overlay can
    // only be clicked on its own rather than by name.
    handlers: [],
    addEventListener(type, handler) {
      const key = `${id}:${type}`;
      listeners.set(key, [...(listeners.get(key) ?? []), handler]);
      element.handlers.push({ type, handler });
    },
    removeEventListener() {},
    getBoundingClientRect() {
      return { left: 0, top: 0, width: 360, height: 360, right: 360, bottom: 360 };
    },
    attributes: {},
    setAttribute(name, value) { this.attributes[name] = String(value); },
    getAttribute(name) { return this.attributes[name] ?? null; },
    removeAttribute(name) { delete this.attributes[name]; },
    setPointerCapture() {},
    releasePointerCapture() {},
    getContext() { return element === canvas ? context2d : stubContext(); },
  };
  return element;
}

for (const id of [
  'app', 'board', 'stage', 'level-number', 'level-name', 'score', 'score-target',
  'moves', 'objectives', 'feed',
  'overlay', 'overlay-title', 'overlay-body', 'overlay-buttons',
  'tracker', 'tracker-items', 'level-list',
  'levels-button', 'hint-button', 'retry-button', 'sound-button',
  'title', 'solo-button', 'solo-note', 'archipelago-button',
  'setup', 'setup-options', 'setup-start', 'setup-back',
]) {
  elements.set(id, stubElement(id));
}
canvas = elements.get('board');
canvas.parentElement = elements.get('stage');
// The overlay starts hidden in the markup; the stub has to agree.
elements.get('overlay').classList.add('hidden');

globalThis.document = {
  getElementById: (id) => {
    const element = elements.get(id);
    assert.ok(element, `the page asked for #${id}, which the markup does not define`);
    return element;
  },
  createElement: (tag) => stubElement(`created:${tag}`),
};

const store = new Map();
const windowListeners = new Map();
/// Acts like someone touching the page. Honors `{ once: true }`, because
/// whether a listener stays registered is exactly what some of this checks.
const gesture = (type) => {
  const list = windowListeners.get(type) ?? [];
  windowListeners.set(
    type,
    list.filter((entry) => !entry.once),
  );
  for (const entry of list) {
    entry.handler({ type });
  }
};
globalThis.window = {
  devicePixelRatio: 2,
  // ?debug only hands the engine and renderer to window.twiddlygems; nothing
  // else about the page changes. It is on here so this can reach past the DOM
  // to the things that draw.
  location: { search: '?debug', href: 'http://localhost/index.html?debug' },
  localStorage: {
    getItem: (key) => store.get(key) ?? null,
    setItem: (key, value) => store.set(key, value),
    removeItem: (key) => store.delete(key),
  },
  // Recorded rather than dropped, so a test can act like someone touching the
  // page. Opening the audio device hangs off these.
  addEventListener(type, handler, options) {
    const once = typeof options === 'object' && options !== null && Boolean(options.once);
    const list = windowListeners.get(type) ?? [];
    list.push({ handler, once });
    windowListeners.set(type, list);
  },
  removeEventListener(type, handler) {
    const list = (windowListeners.get(type) ?? []).filter((e) => e.handler !== handler);
    windowListeners.set(type, list);
  },
};
globalThis.ResizeObserver = class {
  observe() {}
  disconnect() {}
};

let clock = 0;
globalThis.performance = { now: () => clock };

let pending = [];
let framesRun = 0;
globalThis.requestAnimationFrame = (callback) => {
  pending.push(callback);
  return pending.length;
};

const wasmBytes = await readFile(WASM);
globalThis.fetch = async (url) => {
  assert.equal(url, 'twiddlygems.wasm', `the page fetched ${url}, which is not the module`);
  return { ok: true, status: 200, arrayBuffer: async () => wasmBytes };
};

// ---- run the real front end ----

// Pin the seed rather than letting the page deal a random one. Everything the
// engine does follows from it, so this is what makes the run reproducible: one
// of the checks below plays a level out by following the engine's own hints,
// and that bot is a poor player. A test that fails one run in a hundred is
// worse than one that only ever sees one board.
//
// Not the opening level, and not because of the bot alone. First Light is a
// three move puzzle now: a level that has to be solved rather than swiped at,
// which leaves nothing to cash in even when it is won. What the checks below
// want is an ordinary level with room in it, so the run starts on the second
// one, on a seed where following hints wins with ten moves to spare and the
// run ends up holding only the rocket and the rainbow, neither of which the
// flourish can mint.
const SAVE_KEY = 'twiddlygems.save.v1';
const SEED = 11;
const LEVEL = 1;
store.set(SAVE_KEY, JSON.stringify({ seed: SEED, unlocked: LEVEL + 1, level: LEVEL }));

await import(path.resolve('web/js/main.js'));

// boot() is async and pulls in a module graph, so give it real time rather
// than a fixed number of microtask turns, which raced as soon as another
// module was added.
for (let i = 0; i < 400 && pending.length === 0; i += 1) {
  await new Promise((resolve) => setTimeout(resolve, 5));
}
assert.ok(pending.length > 0, 'the page never reached its frame loop');

function pump(steps) {
  for (let i = 0; i < steps; i += 1) {
    const queued = pending;
    pending = [];
    clock += 16;
    for (const callback of queued) {
      callback(clock);
    }
    framesRun += 1;
  }
}

function dispatch(id, type, event) {
  const handlers = listeners.get(`${id}:${type}`) ?? [];
  assert.ok(handlers.length > 0, `nothing is listening for ${type} on #${id}`);
  for (const handler of handlers) {
    handler({ preventDefault() {}, pointerId: 1, ...event });
  }
}

/// Clicks an element the page built rather than one the markup names.
function click(element, why) {
  assert.ok(element, why);
  const handlers = element.handlers.filter((entry) => entry.type === 'click');
  assert.ok(handlers.length > 0, `${why}: nothing is listening for a click on it`);
  for (const { handler } of handlers) {
    handler({ preventDefault() {} });
  }
}

/// Finds a button in the overlay by the words on it.
function overlayButton(label) {
  return elements.get('overlay-buttons').children.find((child) => child.textContent === label);
}

// ---- the title screen ----
//
// The game opens on a menu rather than on a board, which is also what gets a
// tap in before anything has to make a noise: the audio device is opened from
// a gesture, and without one the opening swap plays in silence.
{
  const title = elements.get('title');
  assert.ok(!title.classList.contains('hidden'), 'the game did not open on its title screen');
  // Archipelago is not built yet. The stub does not parse the markup, so the
  // attribute is checked where it is written, and the thing that would
  // actually go wrong (someone wiring the button up early) is checked here.
  const markup = await readFile('web/index.html', 'utf8');
  const tag = markup.match(/<button id="archipelago-button"[^>]*>/)?.[0] ?? '';
  assert.match(tag, /\bdisabled\b/, 'the Archipelago button is not disabled in the markup');
  assert.equal(
    elements.get('archipelago-button').handlers.length,
    0,
    'something is wired to the Archipelago button, which does not work yet',
  );
  assert.equal(
    elements.get('app').getAttribute('aria-hidden'),
    'true',
    'the board behind the title screen is still exposed to assistive tech',
  );

  // The clock is held while the menu is up: the level behind it must not be
  // playing itself out unseen.
  const drawnBefore = calls.clearRect ?? 0;
  pump(30);
  assert.equal(calls.clearRect ?? 0, drawnBefore, 'the board kept running behind the title screen');

  // Dropped first because the run above planted one: what is being checked is
  // that starting a run writes the seed down, not that one was already there.
  store.delete(SAVE_KEY);
  dispatch('solo-button', 'click', {});
  assert.ok(title.classList.contains('hidden'), 'choosing Solo Play left the title screen up');

  // Straight back into it, with no setup screen in the way: this save is a run
  // already under way, and its settings were fixed when it started. Being
  // asked to set them again would be being offered to throw it away. The
  // other half of that, a fresh run being set up first, is checked at the end
  // once this one has been ended.
  assert.ok(
    elements.get('setup').classList.contains('hidden'),
    'a run already under way was asked to set itself up again',
  );
  assert.equal(elements.get('app').getAttribute('aria-hidden'), null);
  assert.ok(store.has(SAVE_KEY), 'starting a run did not pin its seed');
  const saved = JSON.parse(store.get(SAVE_KEY));
  assert.ok(
    saved.options && Object.keys(saved.options).length > 0,
    'the run was saved without what it was set to',
  );
}

pump(FRAMES);

assert.ok(calls.fill > 0, 'nothing was ever filled on the canvas');
assert.ok(calls.drawImage > 0, 'no cached gem was ever blitted');
assert.ok(calls.clearRect > 0, 'the canvas was never cleared between frames');
assert.ok(canvas.width > 0 && canvas.height > 0, 'the canvas was never sized');
assert.equal(
  canvas.width,
  Math.round(parseFloat(canvas.style.width) * 2),
  'the canvas backing store does not match its CSS size at 2x',
);

const objectives = elements.get('objectives');
assert.ok(objectives.children.length > 0, 'the objective chips were never built');
assert.equal(elements.get('level-number').textContent, `Level ${LEVEL + 1}`);
// The mark still out of reach is named beside the score, and the score wears
// no color yet because the level has never been cleared.
assert.match(
  elements.get('score-target').textContent,
  /^silver [\d,]+$/,
  'the next score mark is not named beside the score',
);
assert.equal(
  elements.get('score').classList.set.size,
  0,
  'an uncleared level is already colored',
);
assert.ok(elements.get('level-name').textContent.length > 0, 'the level has no name on screen');
// Read off the ladder rather than written here, so retuning a level's budget
// does not break the front end's test.
assert.equal(
  elements.get('moves').textContent,
  String(window.twiddlygems.engine.movesTotal),
);
assert.ok(elements.get('overlay').classList.contains('hidden'), 'the overlay is covering the board');

// Tap a gem, then its neighbor: the two taps should start a swap, which the
// frame loop then resolves.
const scoreBefore = elements.get('score').textContent;
dispatch('board', 'pointerdown', { clientX: 40, clientY: 40 });
dispatch('board', 'pointerup', { clientX: 40, clientY: 40 });
pump(2);
const strokesAfterSelect = calls.stroke;
assert.ok(strokesAfterSelect > 0, 'selecting a gem drew no selection ring');

// Swipe until one of them lands a match. The board always has a legal move, so
// sweeping it proves the whole path works: gesture, engine, HUD. Without this
// the harness would be happy with an input layer that silently did nothing.
const cell = parseFloat(canvas.style.width) / 8.4;
const pad = cell * 0.2;
let scored = false;
outer: for (let r = 0; r < 8 && !scored; r += 1) {
  for (let c = 0; c < 7; c += 1) {
    const x = pad + (c + 0.5) * cell;
    const y = pad + (r + 0.5) * cell;
    dispatch('board', 'pointerdown', { clientX: x, clientY: y });
    dispatch('board', 'pointermove', { clientX: x + cell, clientY: y });
    dispatch('board', 'pointerup', { clientX: x + cell, clientY: y });
    pump(60);
    if (elements.get('score').textContent !== '0') {
      scored = true;
      break outer;
    }
  }
}
assert.ok(scored, 'no swipe anywhere on the board ever scored a point');
// Spent rather than remaining: a chain along the way can hand the run a moves
// item, which raises the budget and the counter together, so the number on
// screen is not required to have gone down.
assert.ok(
  window.twiddlygems.engine.movesTotal - window.twiddlygems.engine.movesLeft > 0,
  'a scoring swap did not cost a move',
);

// The hint button has to produce a highlight without throwing.
dispatch('hint-button', 'click', {});
pump(2);

// Restart, which reloads the level and rebuilds the HUD.
dispatch('retry-button', 'click', {});
pump(5);
assert.equal(elements.get('score').textContent, '0', 'restarting did not reset the score on screen');
{
  const { engine } = window.twiddlygems;
  assert.equal(engine.movesLeft, engine.movesTotal, 'restarting did not restore the moves');
  assert.equal(
    elements.get('moves').textContent,
    String(engine.movesTotal),
    'the counter on screen disagrees with the engine',
  );
}

// Sound is off by default here (the stub has no AudioContext), but the button
// must still toggle without throwing and must remember the choice.
const soundLabel = elements.get('sound-button').textContent;
dispatch('sound-button', 'click', {});
assert.notEqual(
  elements.get('sound-button').textContent,
  soundLabel,
  'the sound button did not change state',
);
assert.ok(store.has('twiddlygems.sound.v1'), 'the sound setting was not saved');
dispatch('sound-button', 'click', {});

// Opening the audio device survives a browser that refuses the first attempt.
//
// Firefox on Android does not count a gesture as having happened until it
// finishes, so the context opened on `pointerdown` comes back suspended and the
// game plays in silence with the button still claiming sound is on. Modelled
// here as a device that only starts on the second ask.
{
  const { audio } = window.twiddlygems;
  let asked = 0;
  let running = false;
  audio.unlock = () => {
    asked += 1;
    running = asked > 1;
  };
  Object.defineProperty(audio, 'ready', { get: () => running, configurable: true });

  // The same gesture twice over, which is what a player tapping the board
  // does. A listener that fires once per kind of event would look fine against
  // two different kinds and still leave this browser silent forever.
  gesture('pointerdown');
  assert.equal(asked, 1, 'the first gesture should have tried to open the audio device');
  assert.ok(!audio.ready, 'and this browser refuses the first ask');

  gesture('pointerdown');
  assert.equal(asked, 2, 'a second tap should try again rather than giving up');
  assert.ok(audio.ready, 'the second ask is the one that works');

  gesture('pointerdown');
  assert.equal(asked, 2, 'and once it is running, it stops asking');

  // Hand the real object back. Both of these shadow what the class provides,
  // and left in place they leave an Audio that says it is ready with no
  // context behind it, which throws the moment anything later plays a sound.
  delete audio.unlock;
  delete audio.ready;
}

// The pop-over that announces a shuffle or a short move budget.
//
// Worth reaching for deliberately: it is the only thing on the canvas that
// draws text, so it is the only thing that would find fillText or measureText
// missing, and it fires on a board state an ordinary smoke run never reaches.
{
  const { EventKind } = await import(path.resolve('web/js/engine.js'));
  const { renderer } = window.twiddlygems;
  const before = calls.fillText ?? 0;
  const now = performance.now();
  renderer.addEvents([{ kind: EventKind.SHUFFLE, r: 255, c: 255, value: 0 }], now);
  assert.ok(renderer.toast, 'a shuffle raised no pop-over');
  renderer.dirty = true;
  renderer.draw(now + 200);
  assert.ok((calls.fillText ?? 0) > before, 'the pop-over drew no text');

  renderer.addEvents([{ kind: EventKind.LOW_MOVES, r: 255, c: 255, value: 1 }], now);
  assert.equal(renderer.toast.text, '1 move left', 'the last move should read as singular');
  renderer.addEvents([{ kind: EventKind.LOW_MOVES, r: 255, c: 255, value: 5 }], now);
  assert.equal(renderer.toast.text, '5 moves left');

  // And it clears itself once it has run its course, rather than pinning the
  // frame loop awake forever.
  renderer.draw(now + 60_000);
  assert.equal(renderer.toast, null, 'the pop-over outlived its fade');
}

// The level picker builds one row per level, with the locked ones disabled.
dispatch('levels-button', 'click', {});
const list = elements.get('level-list');
const items = elements.get('tracker-items');
assert.ok(list.children.length >= 10, 'the level picker is missing levels');
assert.equal(list.children[0].disabled, false, 'level one is locked');
assert.equal(list.children[9].disabled, true, 'a level nobody has reached is unlocked');
assert.ok(
  list.children[LEVEL].classList.contains('current'),
  'the picker does not mark the level being played',
);
// Every row says which of its three marks have been taken, not just the best
// of them, so a level cleared and a level golded are told apart on sight.
for (const row of list.children) {
  assert.equal(row.children.at(-1).children.length, 3, 'a level row is missing its marks');
}

// The tracker above shows all five unlocks from the start, greyed until they
// turn up, so what a run is still waiting on is as readable as what it holds.
const found = () => items.children.filter((slot) => slot.classList.contains('found'));
assert.equal(items.children.length, 5, 'the tracker is not showing all five unlocks');
{
  const held = window.twiddlygems.engine.unlockedSpecials;
  assert.ok(held.size < 5, 'this run holds everything, so nothing here tests a greyed slot');
  assert.equal(
    found().length,
    held.size,
    `the tracker lit ${found().length} unlocks against the ${held.size} the run holds`,
  );
}
click(overlayButton('Close'), 'the level picker has no way out');

// Play the opening level out with the engine's own hints, which is the only
// way to reach the panel that appears when a level ends.
{
  const { EventKind, Phase, Special, Status, Tier } = await import(path.resolve('web/js/engine.js'));
  const { engine, renderer } = window.twiddlygems;
  // Watched as it goes, because both are gone by the time the level ends: the
  // pop-over fades long before a long run down finishes, and the phases are
  // only passed through.
  const phases = new Set();
  const toasts = new Set();
  const counterWhileSpending = new Set();
  const scoreClassesWhileSpending = new Set();
  // Counted at the source rather than by looking at the particle list, which
  // still holds debris from the clear that won the level: that made the check
  // pass whether or not a single spend threw anything.
  let sparklesWhileSpending = 0;
  const realSparkle = renderer.sparkle.bind(renderer);
  renderer.sparkle = (burst) => {
    if (engine.phase === Phase.CASHING_IN) {
      sparklesWhileSpending += 1;
    }
    realSparkle(burst);
  };
  for (let i = 0; i < 4000 && engine.status === Status.PLAYING; i += 1) {
    if (engine.acceptsInput) {
      const move = engine.hint();
      if (move) {
        engine.swap(...move);
      }
    }
    pump(1);
    phases.add(engine.phase);
    if (renderer.toast) {
      toasts.add(renderer.toast.text);
    }
    if (engine.phase === Phase.CASHING_IN) {
      counterWhileSpending.add(engine.movesLeft);
      for (const cls of elements.get('score').classList.set) {
        scoreClassesWhileSpending.add(cls);
      }
    }
  }
  assert.equal(engine.status, Status.WON, 'following the hints never finished level one');
  // The moves left over when the goal was met are spent on the way out, so a
  // won level always ends on nothing.
  assert.equal(engine.movesLeft, 0, 'the leftover moves were not cashed in');

  assert.ok(phases.has(Phase.CASHING_IN), 'the leftover moves were never spent on screen');
  assert.ok(
    counterWhileSpending.size > 3,
    `the counter went to zero in one step rather than running down: ${[...counterWhileSpending]}`,
  );
  // This run holds none of the three eligible unlocks, so nothing is placed
  // and the board does not change: the motes are the only sign each move was
  // spent, which is exactly why they fire on a spend that placed nothing.
  const counts = [...counterWhileSpending];
  const spent = Math.max(...counts) - Math.min(...counts);
  assert.ok(
    sparklesWhileSpending >= spent,
    `${spent} moves were seen spent but only ${sparklesWhileSpending} threw motes`,
  );
  renderer.sparkle = realSparkle;
  // The score takes the color of what the level has been beaten to, and does
  // so while the flourish is still adding rather than only at the end.
  assert.ok(
    [...scoreClassesWhileSpending].some((cls) => /^tier-/.test(cls)),
    `the score was never colored during the run down: ${[...scoreClassesWhileSpending]}`,
  );
  assert.ok(
    toasts.has('Level cleared'),
    `the level never said it was cleared, only ${JSON.stringify([...toasts])}`,
  );
  assert.ok(
    phases.has(Phase.FINISHING),
    'the board was not held for a beat before the level was declared over',
  );
  pump(3);

  const overlay = elements.get('overlay');
  assert.ok(!overlay.classList.contains('hidden'), 'winning raised no panel');

  // Clearing a level hands over whatever is kept there, and the panel is
  // where the player is told. Which item that is belongs to the placement, so
  // this asks that it was named and said where it came from, not which one it
  // was.
  //
  // Any location, not this level's own: the panel shows the last item the run
  // was handed, and a winning move can pay a chain or an AP gem in the same
  // breath as the clear. That the level's own item was announced is checked
  // against the feed below, which keeps every line rather than the last.
  assert.match(
    elements.get('overlay-body').textContent,
    /Found .+ \((Level \d+ (Clear|Silver|Gold|AP Gem \d+)|\d+ Chain)\)\./,
    'clearing the level announced nothing',
  );

  // The feed is the running record of what the run has been given, and where
  // the Archipelago feed will go. It reads the event stream rather than asking
  // the engine what it holds, so a multiworld item lands the same way.
  const feed = elements.get('feed');
  const lines = feed.children.map((line) =>
    line.children.map((part) => part.textContent ?? part).join(''),
  );
  assert.ok(
    lines.some((line) => line.endsWith(`(Level ${LEVEL + 1} Clear)`)),
    `clearing the level never reached the item feed, which holds ${JSON.stringify(lines)}`,
  );
  // A chain along the way pays too, and so does an AP gem if one fell, and
  // every line has to say where its item came from: the location is what
  // makes the feed readable when a multiworld is sending things in from
  // everywhere.
  assert.ok(
    lines.every((line) =>
      /^Found \S.*\((Level \d+ (Clear|Silver|Gold|AP Gem \d+)|\d+ Chain)\)$/.test(line),
    ),
    `the feed has a line it cannot place: ${JSON.stringify(lines)}`,
  );
  assert.ok(
    lines.some((line) => / Chain\)$/.test(line)),
    `nothing was ever found on a chain, which the placement says it should be: ${JSON.stringify(lines)}`,
  );

  // What the run has found goes in the save. Without it a reload keeps the
  // levels a player unlocked and quietly takes back everything they earned on
  // the way, which is worse than losing both.
  const saved = JSON.parse(store.get(SAVE_KEY));
  assert.ok(Array.isArray(saved.checked), 'the save does not record what was found');
  assert.ok(saved.checked.length > 0, 'clearing a level was not written down');

  // Handing those back rebuilds the run, quietly: restoring is not finding.
  //
  // The same seed, because the seed is what dealt the progression: a run
  // rebuilt from somebody else's seed would look up the saved location ids in
  // a different layout and hand back items that were never found. That is what
  // the save's own seed is for, and what the page reloads with.
  const restored = new (Object.getPrototypeOf(engine).constructor)(engine.wasm, SEED);
  let announced = 0;
  for (const id of saved.checked) {
    restored.restore(id);
    announced += restored.drainEvents().filter((e) => e.kind === EventKind.ITEM).length;
  }
  assert.deepEqual(
    [...restored.unlockedSpecials],
    [...engine.unlockedSpecials],
    'a restored run does not hold what the saved one did',
  );
  assert.equal(announced, 0, 'restoring replayed the finds as news');
  assert.ok(
    elements.get('tracker').classList.contains('hidden'),
    'the finished-level panel is showing the level picker underneath its buttons',
  );

  // From there the picker marks where the player is going, not the level they
  // just finished: it sits beside a button offering to start the next one.
  click(overlayButton('Levels'), 'the finished-level panel offers no way to the picker');
  const marked = list.children.findIndex((row) => row.classList.contains('current'));
  assert.equal(marked, LEVEL + 1, 'the picker marks the level just finished rather than the next one');

  // And the level just beaten wears how well it was beaten, while a level
  // nobody has touched wears nothing.
  const tierOf = (row) => [...row.classList.set].find((cls) => /^tier-/.test(cls)) ?? null;
  assert.ok(tierOf(list.children[LEVEL]), 'the level just cleared is not marked as beaten');
  assert.equal(tierOf(list.children[9]), null, 'a level nobody has played is marked as beaten');
  // Its clear is taken, and the marks it did not reach are not.
  const pips = list.children[LEVEL].children.at(-1).children;
  assert.ok(pips[0].classList.contains('taken'), 'the cleared level has no clear against it');
  assert.equal(
    pips.filter((pip) => pip.classList.contains('taken')).length,
    [Tier.CLEAR, Tier.SILVER, Tier.GOLD].filter((tier) => engine.levelBest(LEVEL) >= tier).length,
    'the marks on a row disagree with what the engine says was beaten',
  );

  // The tracker reads the run rather than counting item lines: it says what is
  // held now, which is what a multiworld handing something over changes.
  const held = engine.unlockedSpecials;
  assert.equal(
    found().length,
    held.size,
    `the tracker shows ${found().length} unlocks against the ${held.size} the run holds`,
  );

  // Coming back to a level already beaten, the score wears the color of the
  // best it was beaten to from the outset rather than starting plain: the
  // achievement belongs to the level, not to the attempt.
  dispatch('retry-button', 'click', {});
  pump(2);
  assert.equal(engine.score, 0, 'the retry did not start the level over');
  assert.ok(
    [...elements.get('score').classList.set].some((cls) => /^tier-/.test(cls)),
    'the score forgot what this level had already been beaten to',
  );
}

// Ending a run from that same menu asks first, then throws the progress away
// and goes back to the title screen.
{
  const { engine } = window.twiddlygems;
  engine.setUnlocked(5);

  // The retry above closed the panel, so the menu is opened afresh.
  dispatch('levels-button', 'click', {});
  click(overlayButton('Title screen'), 'the level menu offers no way back to the title');
  click(overlayButton('Keep playing'), 'ending a run is not confirmed first');
  assert.ok(elements.get('title').classList.contains('hidden'), 'backing out still quit the run');
  assert.equal(engine.unlocked, 5, 'backing out still threw the progress away');

  // Backing out returns to the level list, so the way in is open again.
  click(overlayButton('Title screen'), 'backing out closed the menu instead of reopening it');
  click(overlayButton('End the run'), 'the confirmation has no way to go through with it');
  assert.ok(!elements.get('title').classList.contains('hidden'), 'ending a run left the board up');
  assert.ok(elements.get('overlay').classList.contains('hidden'), 'the menu is still over the title');
  assert.equal(engine.unlocked, 1, 'ending a run kept the levels it had unlocked');
  assert.equal(engine.levelIndex, 0, 'ending a run left us on a later level');
  assert.ok(!store.has(SAVE_KEY), 'ending a run left the save behind');

  // And now there is a fresh run to start, which is set up before it begins,
  // because the settings are fixed for its whole length the way a
  // multiworld's yaml is.
  dispatch('solo-button', 'click', {});
  const setup = elements.get('setup');
  assert.ok(!setup.classList.contains('hidden'), 'a fresh run was not offered a setup screen');
  const rows = elements.get('setup-options').children;
  assert.ok(rows.length > 0, 'the setup screen has no settings on it');

  // Every control is built by walking the engine's table, so each row should
  // have a way to move the setting and something showing where it is. Tapping
  // one has to change what it says, or the control is decoration.
  const steps = rows[0].children.find((child) => child.className === 'setup-controls');
  assert.ok(steps, 'a setting has no controls');
  const reading = steps.children.find((child) => child.className === 'setup-value');
  assert.ok(reading && reading.textContent, 'a setting does not say what it is set to');
  const before = reading.textContent;
  click(
    steps.children.find((child) => child.className === 'setup-step'),
    'a setting has no button to move it',
  );
  assert.notEqual(reading.textContent, before, 'tapping a setting changed nothing');

  dispatch('setup-start', 'click', {});
  assert.ok(setup.classList.contains('hidden'), 'starting the run left the setup screen up');
  assert.ok(store.has(SAVE_KEY), 'starting a fresh run did not write a save');
}

console.log(
  `page ok: ${framesRun} frames, ${calls.drawImage} blits, ${calls.fill} fills, ` +
    `${calls.stroke} strokes, ${objectives.children.length} objective chips, ` +
    `${list.children.length} levels listed, ` +
    `a swipe scored and spent a move (${scoreBefore} -> played -> reset)`,
);
