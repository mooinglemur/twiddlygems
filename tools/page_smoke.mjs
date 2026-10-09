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
/// The effects layer counts separately from the board, so a check can ask
/// whether something was drawn *there* rather than anywhere. Everything that
/// crosses the page rather than sitting on the board goes here: the motes a
/// clear sends to its goal, and the rockets spent out of the bottom bar.
const fxCalls = new Proxy({}, { get: (target, key) => target[key] ?? 0 });
const context2d = stubContext();
const listeners = new Map();
const elements = new Map();
let canvas;
let fxSurface;
let fxContext;

function stubContext(into = calls) {
  const ctx = {};
  const methods = [
    'setTransform', 'clearRect', 'save', 'restore', 'beginPath', 'closePath', 'moveTo',
    'lineTo', 'arc', 'arcTo', 'ellipse', 'quadraticCurveTo', 'rect', 'fill', 'stroke', 'clip',
    'fillRect', 'strokeRect', 'translate', 'scale', 'rotate', 'drawImage', 'fillText',
  ];
  for (const name of methods) {
    ctx[name] = () => {
      into[name] = (into[name] ?? 0) + 1;
      // The board's tally counts everything, so a check that only cares that
      // something was drawn at all keeps working whichever surface it went to.
      if (into !== calls) {
        calls[name] = (calls[name] ?? 0) + 1;
      }
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
    // Two names for one thing, the way a browser has it. Kept apart, a page
    // could set `className` and then be told by `classList` that the classes
    // it had just replaced were all still there, which is a stub quietly
    // disagreeing with every browser the game runs in.
    get className() {
      return [...this.classList.set].join(' ');
    },
    set className(names) {
      this.classList.set = new Set(String(names).split(/\s+/).filter(Boolean));
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
    // Whether something is inside something else, which is how the page
    // decides that a tap landed on the board, or on the slot an item came out
    // of, rather than somewhere that means "never mind".
    contains(node) {
      if (node === this) {
        return true;
      }
      return this.children.some((child) => child?.contains?.(node) === true);
    },
    scrollHeight: 0,
    scrollTop: 0,
    // No layout here, so these are 0 and equal, which reads as "everything
    // fits". A test that wants the other answer sets them; see the bar's
    // overflow check.
    clientWidth: 0,
    scrollWidth: 0,
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
    focus() {},
    // What a form field holds. Every element gets one because the stub has no
    // idea which of them are inputs, and an element nobody reads it from is
    // not harmed by having it.
    value: '',
    getContext() {
      if (element === canvas) {
        return context2d;
      }
      if (element === fxSurface) {
        fxContext ??= stubContext(fxCalls);
        return fxContext;
      }
      return stubContext();
    },
  };
  return element;
}

// Every id the real page defines, read out of the real page.
//
// This was a list kept here by hand, which made the assertion below a lie: it
// said the markup did not define an id when what had happened was that nobody
// had added it to this list. Reading `index.html` makes the sentence true and
// means markup and script can be changed together without a third file having
// to be remembered.
const markup = await readFile(new URL('../web/index.html', import.meta.url), 'utf8');
const ids = [...markup.matchAll(/\bid="([^"]+)"/g)].map((found) => found[1]);
assert.ok(ids.length > 20, 'the page markup could not be read, so this proves nothing');
for (const id of ids) {
  elements.set(id, stubElement(id));
}
canvas = elements.get('board');
canvas.parentElement = elements.get('stage');
fxSurface = elements.get('fx');
// The overlay starts hidden in the markup; the stub has to agree.
elements.get('overlay').classList.add('hidden');

const documentListeners = new Map();
globalThis.document = {
  getElementById: (id) => {
    const element = elements.get(id);
    assert.ok(element, `the page asked for #${id}, which the markup does not define`);
    return element;
  },
  createElement: (tag) => stubElement(`created:${tag}`),
  // Whether anybody is looking. The multiworld connection reads it to decide
  // whether reconnecting is worth doing, so a stub without it is a stub the
  // page cannot boot against.
  visibilityState: 'visible',
  addEventListener(type, handler) {
    documentListeners.set(type, [...(documentListeners.get(type) ?? []), handler]);
  },
  removeEventListener() {},
};

/// Acts like switching to another tab, or back.
const look = (how) => {
  globalThis.document.visibilityState = how;
  for (const handler of documentListeners.get('visibilitychange') ?? []) {
    handler({ type: 'visibilitychange' });
  }
};

const store = new Map();
const windowListeners = new Map();
/// Acts like someone touching the page. Honors `{ once: true }`, because
/// whether a listener stays registered is exactly what some of this checks.
const gesture = (type, event = {}) => {
  const list = windowListeners.get(type) ?? [];
  windowListeners.set(
    type,
    list.filter((entry) => !entry.once),
  );
  for (const entry of list) {
    entry.handler({ type, ...event });
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
  // Asked for relative to the module that wants it rather than to the page,
  // which makes it an absolute URL by the time it gets here. That is what lets
  // the shipping server put the whole site under a prefix carrying a build
  // fingerprint without the wasm being the one thing left behind at the root:
  // see `WASM_URL` in main.js. So this checks the end of it rather than the
  // whole, and still checks it is the module and not something else.
  assert.match(
    String(url),
    /\/twiddlygems\.wasm$/,
    `the page fetched ${url}, which is not the module`,
  );
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
//
// The ladder is set to open by clearing rather than by item, which is not the
// default any more. A run opening it by item is only as far up as its items
// have carried it, and which location holds the first unlock is the seed's
// business, so a save that simply claims to be on the second level would be a
// save the engine is right to refuse. What is being checked below is the page,
// and the page needs a level with room on it to check anything at all. The
// ladder's own rule is checked where it lives, in the engine's tests and the
// world's.
const SAVE_KEY = 'twiddlygems.save.v1';
const SEED = 11;
// Which level the page is driven through.
//
// Pillars, because it is one of the two a bot following hints can actually
// clear bare. With the budgets measured rather than guessed, the attentive bot
// wins about half of everything and this bot is well short of attentive: it
// takes this one about one attempt in seven and most of the ladder not at all.
//
// One in seven is fine because the seed below is pinned. The boards it is
// dealt are the same every run, so the retry loop further down either gets
// there or never does; it is not a gamble that comes out differently on
// somebody else's machine.
//
// Mid-ladder rather than the opener, which is the other candidate: a save
// sitting on the first level with nothing unlocked is a fresh run, and the
// page would rightly offer to set it up rather than resume it.
const LEVEL = 8;
store.set(
  SAVE_KEY,
  JSON.stringify({
    seed: SEED,
    unlocked: LEVEL + 1,
    level: LEVEL,
    options: { progressive_levels: 0 },
  }),
);

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
  // Archipelago is built now, so the button has to be reachable: enabled in
  // the markup, wired here, and leading somewhere a player can get back out
  // of. It was checked the other way round for as long as it did nothing.
  const tag = markup.match(/<button id="archipelago-button"[^>]*>/)?.[0] ?? '';
  assert.doesNotMatch(tag, /\bdisabled\b/, 'the Archipelago button is still disabled in the markup');
  assert.ok(
    elements.get('archipelago-button').handlers.length > 0,
    'nothing is wired to the Archipelago button',
  );

  dispatch('archipelago-button', 'click');
  assert.ok(
    !elements.get('connect').classList.contains('hidden'),
    'the Archipelago button did not open the connect screen',
  );
  assert.ok(title.classList.contains('hidden'), 'the title screen stayed up over the connect screen');
  // And back out again, without having touched a socket.
  dispatch('connect-back', 'click');
  assert.ok(elements.get('connect').classList.contains('hidden'), 'there is no way back from the connect screen');
  assert.ok(!title.classList.contains('hidden'), 'backing out of the connect screen went nowhere');
  // Nothing typed, so trying to connect must say so rather than reaching for a
  // server that was never named.
  dispatch('archipelago-button', 'click');
  dispatch('connect-form', 'submit');
  assert.match(
    elements.get('connect-status').textContent,
    /server and a slot name/,
    'an empty form tried to connect anyway',
  );
  dispatch('connect-back', 'click');
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

// The stage is shaped to the level rather than taking whatever is left over,
// which is what lets the feed have the rest of the column. The sheet cannot
// know a level's shape, so the renderer writes it on.
assert.equal(
  elements.get('stage').style.aspectRatio,
  `${window.twiddlygems.engine.cols} / ${window.twiddlygems.engine.rows}`,
  'the stage was not shaped to the board it is holding',
);

const objectives = elements.get('objectives');
assert.ok(objectives.children.length > 0, 'the objective chips were never built');

// A goal chip carries a picture of what it wants and how much of it is left,
// and nothing else.
//
// Read off the engine rather than written down here. This used to name the
// level's goals in the test, which made it a test of which level sat at
// `LEVEL` as much as of the chips, and it went stale the moment the ladder was
// redesigned around it.
{
  const { ObjectiveKind } = await import(path.resolve('web/js/engine.js'));
  const goals = window.twiddlygems.engine.objectives();
  assert.ok(goals.length > 0, 'the level has nothing to do');
  assert.equal(objectives.children.length, goals.length, 'a goal has no chip, or a chip no goal');
  for (const chip of objectives.children) {
    assert.equal(chip.children.length, 2, 'a goal chip has more on it than a mark and a number');
    assert.match(
      chip.children[1].textContent,
      /^[\d,]+$/,
      `a goal chip reads ${JSON.stringify(chip.children[1].textContent)}`,
    );
    assert.ok(!chip.classList.contains('met'), 'a goal is met before a move was made');
  }
  // A score goal is not somewhere a clear can fly to: everything on the board
  // feeds the score, so a mote landing there would say nothing about which
  // clear did what, and the score is not in that row anyway. Every other kind
  // of goal gets one.
  assert.equal(
    window.twiddlygems.renderer.goals.length,
    goals.filter((goal) => goal.kind !== ObjectiveKind.SCORE).length,
    'a score goal was offered as somewhere for a clear to fly to, or another kind was not',
  );
}
assert.equal(elements.get('level-number').textContent, `Level ${LEVEL + 1}`);
assert.equal(
  elements.get('score').classList.set.size,
  0,
  'an uncleared level is already colored',
);

// What a level can be beaten to sits behind a tap of the score rather than
// beside it: two numbers nobody is reading most of the time, in the one corner
// where the number being watched lives.
{
  const marks = elements.get('score-marks');
  const box = elements.get('score-box');
  assert.ok(marks.classList.contains('hidden'), 'the score marks are showing before anyone asked');
  assert.equal(box.getAttribute('aria-expanded'), 'false');

  dispatch('score-box', 'click', {});
  assert.ok(!marks.classList.contains('hidden'), 'tapping the score showed nothing');
  assert.equal(box.getAttribute('aria-expanded'), 'true');
  const said = marks.children.map((row) =>
    row.children.map((part) => part.textContent ?? part).join(' '),
  );
  assert.equal(said.length, 3, `the marks popover shows ${JSON.stringify(said)}`);
  assert.match(said[0], /^Silver [\d,]+$/, `the silver mark reads ${JSON.stringify(said[0])}`);
  assert.match(said[1], /^Gold [\d,]+$/, `the gold mark reads ${JSON.stringify(said[1])}`);
  // And what the run has actually done here, which on a level it has never
  // beaten is nothing at all rather than a zero.
  assert.equal(said[2], 'Your best not yet', `the best reads ${JSON.stringify(said[2])}`);
  // Neither mark is behind yet, so neither is checked and neither wears its
  // metal as something already won.
  assert.ok(
    marks.children.every((row) => !row.classList.contains('taken')),
    'a mark is checked off on a level that has never been cleared',
  );

  // A tap anywhere else puts it away, which is a listener on the window rather
  // than a backdrop: a backdrop would swallow the first tap on the board, and
  // putting a popover away is not worth a move.
  gesture('pointerdown');
  assert.ok(marks.classList.contains('hidden'), 'the score marks survived a tap elsewhere');
  assert.equal(box.getAttribute('aria-expanded'), 'false');
}
assert.ok(elements.get('level-name').textContent.length > 0, 'the level has no name on screen');
// Up in the bar with the level and the score, where the eye already is: the
// number that runs out beside the one that climbs. Checked against the markup,
// because the stub has no idea what is inside what.
{
  const markup = await readFile('web/index.html', 'utf8');
  const bar = markup.slice(markup.indexOf('<header id="topbar"'), markup.indexOf('</header>'));
  assert.ok(bar.includes('id="moves"'), 'the moves counter is not in the top bar');
  assert.ok(bar.indexOf('id="moves"') > bar.indexOf('id="level-title"'), 'it is before the level');
  assert.ok(bar.indexOf('id="moves"') < bar.indexOf('id="score"'), 'it is after the score');

  // Who made it, directly under what it is. Order is the whole of the check:
  // both lines exist either way, and the wrong way round reads as the game
  // being called "by MooingLemur".
  const card = markup.slice(markup.indexOf('<div id="title-card"'), markup.indexOf('id="title-buttons"'));
  assert.ok(card.includes('id="title-byline"'), 'the title screen does not say who made it');
  assert.ok(
    card.indexOf('id="title-byline"') > card.indexOf('id="title-tagline"'),
    'the byline is above the tagline rather than under it',
  );

  // Where the source is. Written out rather than assembled, so the one thing
  // that would make it useless, pointing at the wrong repository, is visible
  // here as well as there.
  // From the opening tag rather than to the first `</footer>` in the file:
  // the bar under the board is a footer too, and it closes before this one
  // opens.
  const opens = markup.indexOf('<footer id="colophon"');
  assert.notEqual(opens, -1, 'the page has no colophon');
  const colophon = markup.slice(opens, markup.indexOf('</footer>', opens));
  assert.ok(
    colophon.includes('href="https://github.com/mooinglemur/twiddlygems"'),
    'the GitHub link does not point at this repository',
  );
  assert.ok(colophon.includes('>GitHub<'), 'the link is not called GitHub');
  assert.ok(colophon.includes('rel="noopener"'), 'a new-tab link without rel="noopener"');

  // One line, with the two at opposite ends of it, which is a thing only the
  // stylesheet knows.
  const style = await readFile('web/css/style.css', 'utf8');
  const rule = /#colophon \{[^}]*\}/.exec(style);
  assert.ok(rule, 'the footer has no rule of its own in the stylesheet');
  assert.match(rule[0], /display:\s*flex/, 'the footer is not laid out as one line');
  assert.match(rule[0], /justify-content:\s*space-between/, 'the two are not at opposite ends');
}

// The build, read out of the module rather than written into the page. The
// shape is what matters: a crate version and the commit it was built from, so
// a bug report names one build rather than a version that moved all week.
assert.match(
  elements.get('build-version').textContent,
  /^\d+\.\d+\.\d+\+\S+$/,
  'the footer does not name this build as <semver>+<commit>',
);
assert.equal(
  elements.get('build-version').textContent,
  window.twiddlygems.engine.version,
  'the footer and the module disagree about which build this is',
);

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

// A hint arrives by waiting, which is the only way there is: the button that
// asked for one is gone, and it took a corner of the bar for something that
// happens by itself a few seconds later.
{
  const { renderer } = window.twiddlygems;
  assert.ok(
    !/id="hint-button"/.test(await readFile('web/index.html', 'utf8')),
    'the hint button is still in the markup',
  );
  renderer.hint = null;
  let waited = 0;
  for (; waited < 900 && !renderer.hint; waited += 1) {
    pump(1);
  }
  assert.ok(renderer.hint, 'staring at the board offered no move');
  assert.ok(waited > 60, `the hint arrived after ${waited} frames, which is no wait at all`);
  // Drawn as well as chosen, because a highlight that throws is a blank page.
  renderer.dirty = true;
  renderer.draw(performance.now());

  // Touching the board takes the nudge away and waiting brings it back, and
  // what comes back is the same move. Selecting a gem and letting it go is a
  // whole round trip that costs nothing, so without this a player could tap
  // their way through every move on the board without making one.
  const offered = renderer.hint;
  const movesBefore = window.twiddlygems.engine.movesLeft;
  for (let round = 0; round < 3; round += 1) {
    dispatch('board', 'pointerdown', { clientX: 40, clientY: 40 });
    dispatch('board', 'pointerup', { clientX: 40, clientY: 40 });
    pump(2);
    assert.ok(!renderer.hint, 'touching the board left the nudge up');
    for (let i = 0; i < 900 && !renderer.hint; i += 1) {
      pump(1);
    }
    assert.deepEqual(
      renderer.hint,
      offered,
      `tapping and waiting ${round + 1} times over dealt another hint`,
    );
  }
  // And none of that was a move, or the hint would have been entitled to
  // change and nothing above was being tested.
  assert.equal(
    window.twiddlygems.engine.movesLeft,
    movesBefore,
    'tapping around cost a move, which is not what was being tested',
  );
}

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

// The speaker opens the volume over the bar, and muting is the button inside
// it. Sound is off by default here (the stub has no AudioContext), but all of
// this has to work without throwing and has to be remembered.
{
  const volume = elements.get('volume');
  const speaker = elements.get('sound-button');
  const slider = elements.get('volume-slider');
  const { audio } = window.twiddlygems;

  assert.ok(volume.classList.contains('hidden'), 'the volume is open before anybody asked');
  dispatch('sound-button', 'click', {});
  assert.ok(!volume.classList.contains('hidden'), 'the speaker did not open the volume');
  assert.equal(speaker.getAttribute('aria-expanded'), 'true');

  // Muting, which is the thing the speaker used to do on its own.
  const loud = speaker.textContent;
  dispatch('volume-mute', 'click', {});
  assert.notEqual(speaker.textContent, loud, 'muting did not change the speaker');
  assert.equal(elements.get('volume-mute').getAttribute('aria-pressed'), 'true');
  assert.equal(audio.enabled, false, 'the audio was not muted');
  assert.equal(audio.level(), 0, 'muted, and still at a level');
  assert.ok(store.has('twiddlygems.sound.v1'), 'the sound setting was not saved');

  // Dragging up out of silence turns it back on. Leaving it muted there would
  // be a slider that says one thing while the game says nothing.
  slider.value = '60';
  dispatch('volume-slider', 'input', {});
  assert.equal(audio.enabled, true, 'dragging up from a mute did not unmute');
  assert.equal(audio.volume, 0.6);
  assert.ok(audio.level() > 0, 'turned back on and still silent');

  // Written when the drag ends rather than on every step of it.
  slider.value = '40';
  dispatch('volume-slider', 'input', {});
  assert.equal(audio.volume, 0.4);
  dispatch('volume-slider', 'change', {});
  assert.equal(store.get('twiddlygems.volume.v1'), '0.4', 'the volume was not remembered');

  // The speaker says how loud as well as whether, so a run with the sound
  // dragged most of the way down looks different from one at full.
  const quiet = speaker.textContent;
  slider.value = '100';
  dispatch('volume-slider', 'input', {});
  assert.notEqual(speaker.textContent, quiet, 'the speaker reads the same at 40% as at full');

  // A tap somewhere else puts it away, the way the score's popover goes.
  gesture('pointerdown', { target: elements.get('board') });
  assert.ok(volume.classList.contains('hidden'), 'the volume stayed up after a tap elsewhere');
  assert.equal(speaker.getAttribute('aria-expanded'), 'false');
}

// The slots step aside when the bar cannot hold them. Measured rather than
// guessed at a width, so this drives the measurement: the stub has no layout,
// and 0 against 0 is what "it fits" looks like here.
{
  const bar = elements.get('bottombar');
  const inventory = elements.get('inventory');
  const { hud } = window.twiddlygems;

  inventory.clientWidth = 183;
  inventory.scrollWidth = 202;
  assert.equal(hud.fitInventory(), true, 'an inventory wider than its box was called a fit');
  assert.ok(bar.classList.contains('tight'), 'the bar did not make room');

  // What is behind the button, so a bar that has lost its slots has not lost
  // the one thing they were saying: that there is something to spend.
  const held = window.twiddlygems.engine;
  held.restoreConsumables(0, 2);
  held.restoreConsumables(1, 3);
  pump(1);
  const total = [0, 1, 2, 3].reduce((sum, kind) => sum + held.consumables(kind), 0);
  assert.equal(total, 5, 'the run is not carrying what this just gave it');
  assert.equal(elements.get('inventory-total').textContent, String(total), 'the badge is not the total');
  assert.equal(elements.get('inventory-total').hidden, false, 'the badge is hidden while holding something');
  assert.match(
    elements.get('inventory-more').getAttribute('aria-label'),
    new RegExp(`${total} held`),
    'the button does not say how much is behind it',
  );

  // Opening lifts the same slots over the bar. The same ones: a second set
  // would be a second place to keep the counts right.
  dispatch('inventory-more', 'click', {});
  assert.ok(bar.classList.contains('items-open'), 'the overflow button opened nothing');
  assert.equal(elements.get('inventory-more').getAttribute('aria-expanded'), 'true');
  gesture('pointerdown', { target: elements.get('board') });
  assert.ok(!bar.classList.contains('items-open'), 'the slots stayed up after a tap elsewhere');

  // The hidden inventory was the flexible thing holding the speaker and
  // Restart against the right edge; without something taking that slack they
  // bunch up on the left and the bar looks like it dropped something. Read off
  // the stylesheet, which is the only thing that knows.
  {
    const style = await readFile('web/css/style.css', 'utf8');
    const rule = /#bottombar\.tight #inventory-more \{[^}]*\}/.exec(style);
    assert.ok(rule, 'nothing styles the overflow button when the bar is tight');
    assert.match(
      rule[0],
      /margin-inline-end:\s*auto/,
      'nothing takes the slack the hidden slots were taking',
    );
  }

  // Arming one while the slots are hidden. The popover closes, so without
  // this the screen looks untouched and the next tap means something the
  // player has no way to see.
  {
    const more = elements.get('inventory-more');
    hud.arm(0);
    pump(1);
    assert.ok(more.classList.contains('armed'), 'arming marked nothing in the bar');
    assert.equal(elements.get('inventory-dots').hidden, true, 'the button still reads as a list');
    assert.equal(elements.get('inventory-armed').hidden, false, 'the armed item is not shown');
    // The badge follows what is armed: how many of that one is left, not the
    // total, which is answering a question nobody is asking any more.
    const mine = held.consumables(0);
    assert.notEqual(mine, total, 'this kind is the whole inventory, so the badge proves nothing');
    assert.equal(
      elements.get('inventory-total').textContent,
      String(mine),
      'the badge still counts everything while one item is armed',
    );
    assert.match(
      more.getAttribute('aria-label'),
      new RegExp(`armed, ${mine} left`),
      'the button does not say what is armed and how many are left',
    );

    // Spent, or put away by a tap elsewhere: either way it goes back to
    // standing in for the list.
    hud.disarm();
    pump(1);
    assert.ok(!more.classList.contains('armed'), 'the mark survived putting the item away');
    assert.equal(elements.get('inventory-dots').hidden, false, 'the button never came back');
    assert.equal(elements.get('inventory-armed').hidden, true, 'the armed item is still showing');
    assert.match(
      more.getAttribute('aria-label'),
      new RegExp(`${total} held`),
      'the button did not go back to saying what is behind it',
    );
  }

  // And back, when there is room again. Left tight, the bar would hide the
  // slots behind a button on a screen wide enough to show them.
  inventory.scrollWidth = 150;
  assert.equal(hud.fitInventory(), false, 'an inventory that fits was called an overflow');
  assert.ok(!bar.classList.contains('tight'), 'the bar kept the slots hidden');
  inventory.clientWidth = 0;
  inventory.scrollWidth = 0;
}

// Opening the audio device survives a browser that refuses the first attempt.
//
// Firefox on Android does not count a gesture as having happened until it
// finishes, so the context opened on `pointerdown` comes back suspended and the
// game plays in silence with the button still claiming sound is on. Modeled
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

  // And it keeps asking for the life of the page. A context that has been
  // running can be suspended again long afterwards: the phone locks, the
  // browser is backgrounded, a call arrives. Giving up once it started meant
  // nothing ever brought it back, and the rest of the session played in
  // silence with the button still claiming the sound was on.
  running = false;
  gesture('pointerdown');
  assert.equal(asked, 3, 'a device suspended after it started was never asked to come back');
  assert.ok(audio.ready, 'so it never came back');

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

// The settings, behind the gear in the levels menu.
//
// Both of these are read where they are acted on rather than handed to the
// engine, so what is checked is the behavior and not the flag: hints off has
// to actually stop a hint arriving, and the buzz has to actually be asked for.
{
  const { renderer } = window.twiddlygems;
  const switches = elements.get('overlay-switches');
  const rowNamed = (name) =>
    switches.children.find((row) =>
      row.children.some((part) => part.textContent === name),
    );

  dispatch('levels-button', 'click', {});
  const gear = elements
    .get('overlay-buttons')
    .children.find((child) => child.getAttribute?.('aria-label') === 'Settings');
  assert.ok(gear, 'the levels menu has no way into the settings');
  click(gear, 'the gear does nothing');
  assert.equal(elements.get('overlay-title').textContent, 'Settings');
  assert.ok(!switches.classList.contains('hidden'), 'the settings panel has no switches');

  // Nothing is offered for something this device cannot do. Node has no
  // `navigator.vibrate`, so the haptics row is absent here, which is the same
  // answer an iPhone gets.
  assert.ok(rowNamed('Hints'), 'the settings do not offer the hints switch');
  assert.ok(
    !rowNamed('Haptic feedback'),
    'a device with no vibration API was offered a switch for it anyway',
  );

  // On by default, and turning it off is written down at once rather than on
  // the way out.
  const hints = rowNamed('Hints');
  assert.equal(hints.getAttribute('aria-checked'), 'true', 'hints did not default to on');
  click(hints, 'the hints switch does not respond');
  assert.equal(hints.getAttribute('aria-checked'), 'false');
  assert.equal(
    JSON.parse(store.get('twiddlygems.settings.v1')).hints,
    false,
    'turning hints off was not remembered',
  );

  // Back goes where it came from, not to the board.
  click(overlayButton('Back'), 'the settings have no way back');
  assert.equal(elements.get('overlay-title').textContent, 'Levels');
  click(overlayButton('Close'), 'the level picker has no way out');

  // And the behavior, which is the point. Far longer than the wait that
  // produced one above.
  renderer.hint = null;
  for (let waited = 0; waited < 900; waited += 1) {
    pump(1);
  }
  assert.equal(renderer.hint, null, 'a hint arrived with hints turned off');

  // Back on, and they come back.
  dispatch('levels-button', 'click', {});
  click(
    elements
      .get('overlay-buttons')
      .children.find((child) => child.getAttribute?.('aria-label') === 'Settings'),
    'the gear went away',
  );
  click(rowNamed('Hints'), 'the hints switch does not respond');
  click(overlayButton('Back'), 'the settings have no way back');
  click(overlayButton('Close'), 'the level picker has no way out');
  for (let waited = 0; waited < 900 && !renderer.hint; waited += 1) {
    pump(1);
  }
  assert.ok(renderer.hint, 'turning hints back on did not bring them back');
}

// The haptic click, which only exists where the browser can do it.
//
// `navigator.vibrate` is asked for at the moment of the swap rather than at
// load, so a fake can be put in place here. Without that this whole setting
// would ship untested, because the machine running this has no vibration
// motor and never will.
{
  const buzzes = [];
  const real = Object.getOwnPropertyDescriptor(globalThis, 'navigator');
  Object.defineProperty(globalThis, 'navigator', {
    value: { vibrate: (ms) => buzzes.push(ms) },
    configurable: true,
  });
  try {
    const switches = elements.get('overlay-switches');
    dispatch('levels-button', 'click', {});
    click(
      elements
        .get('overlay-buttons')
        .children.find((child) => child.getAttribute?.('aria-label') === 'Settings'),
      'the gear went away',
    );
    // Now that the device can buzz, the switch for it is offered.
    const haptics = switches.children.find((row) =>
      row.children.some((part) => part.textContent === 'Haptic feedback'),
    );
    assert.ok(haptics, 'a device that can vibrate was not offered the setting');
    // Off unless asked for: a phone buzzing without being asked is a surprise,
    // and this one fires on every swap rather than once in a while.
    assert.equal(haptics.getAttribute('aria-checked'), 'false', 'haptics did not default to off');
    click(haptics, 'the haptics switch does not respond');
    click(overlayButton('Back'), 'the settings have no way back');
    click(overlayButton('Close'), 'the level picker has no way out');

    // A tap that only selects a gem has moved nothing, so it must not buzz.
    dispatch('board', 'pointerdown', { clientX: 40, clientY: 40 });
    dispatch('board', 'pointerup', { clientX: 40, clientY: 40 });
    pump(2);
    assert.equal(buzzes.length, 0, 'selecting a gem buzzed, having moved nothing');

    // A swipe buzzes when the gems land, not when the swipe is accepted.
    //
    // The whole point of this timing: the gems take SWAP_MS to slide past each
    // other, so a click at the gesture is a click at two gems that have not
    // moved yet. Frames here are 16ms and the slide is 190, so a dozen of them
    // pass before anything should be felt.
    const cell = window.twiddlygems.renderer.cell;
    const x = cell * 0.2 + 2.5 * cell;
    const y = cell * 0.2 + 2.5 * cell;
    dispatch('board', 'pointerdown', { clientX: x, clientY: y });
    dispatch('board', 'pointermove', { clientX: x + cell, clientY: y });
    dispatch('board', 'pointerup', { clientX: x + cell, clientY: y });
    assert.equal(buzzes.length, 0, 'the swipe buzzed before a single frame had run');
    pump(6);
    assert.equal(
      buzzes.length,
      0,
      'the swipe buzzed while the gems were still sliding, which is what was too early',
    );
    pump(30);
    assert.equal(buzzes.length, 1, 'the gems landed without a buzz');
    assert.ok(buzzes[0] > 0 && buzzes[0] <= 40, `a ${buzzes[0]}ms buzz is not a click`);

    // And once per swap, not once per gem the clear went on to take.
    pump(120);
    assert.equal(buzzes.length, 1, `one swap buzzed ${buzzes.length} times`);
  } finally {
    Object.defineProperty(globalThis, 'navigator', real);
  }
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

// And, between the name and those marks, what the level still has in it: one
// mark for its AP gems and one for its moves upgrades. They are what makes the
// picker a tracker, so a row without them is a menu again.
{
  const { engine } = window.twiddlygems;
  const { paintGemsIcon } = await import(path.resolve('web/js/render.js'));
  const gems = engine.levelGems(0);
  assert.ok(gems.total > 0, 'this run hides no gems, so the mark would have nothing to say');
  assert.equal(engine.levelMoves(0).total, 1, 'a level carries some other number of upgrades now');

  const status = list.children[0].children.find((c) => c.className === 'level-status');
  assert.ok(status, 'a level row says nothing about what is still in it');
  assert.equal(status.children.length, 2, 'a level row is missing one of its status marks');
  assert.match(
    status.children[0].title ?? '',
    /of \d+ AP gems/,
    'the gem mark does not say what it is a fraction of',
  );

  // Found and not found have to be two different pictures, or the mark is
  // decoration: the same fill runs whatever the run has taken.
  const drawn = () => {
    const art = document.createElement('canvas');
    let clips = 0;
    const ctx = stubContext({});
    const real = ctx.clip;
    ctx.clip = (...args) => { clips += 1; return real(...args); };
    art.getContext = () => ctx;
    return { art, clips: () => clips, ctx };
  };
  const empty = drawn();
  paintGemsIcon(empty.art, 22, 0, gems.total);
  const some = drawn();
  paintGemsIcon(some.art, 22, gems.total, gems.total);
  assert.ok(
    some.clips() > empty.clips(),
    'a mark with everything found is painted exactly like an empty one',
  );
}

// The tracker above shows all five unlocks from the start, grayed until they
// turn up, so what a run is still waiting on is as readable as what it holds.
const found = () => items.children.filter((slot) => slot.classList.contains('found'));
assert.equal(items.children.length, 5, 'the tracker is not showing all five unlocks');
{
  const held = window.twiddlygems.engine.unlockedSpecials;
  assert.ok(held.size < 5, 'this run holds everything, so nothing here tests a grayed slot');
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
  const { audio, engine, renderer } = window.twiddlygems;
  // Watched as it goes, because both are gone by the time the level ends: the
  // pop-over fades long before a long run down finishes, and the phases are
  // only passed through.
  const phases = new Set();
  const toasts = new Set();
  // Which frame the level said it was cleared on, in words and in sound. They
  // come off the same event and have to stay on it: the two are meant to land
  // together, and nothing about either one would look wrong on its own if one
  // of them moved to some other moment.
  let toastFrame = null;
  let fanfareFrame = null;
  let frame = 0;
  const realPlay = audio.play.bind(audio);
  audio.play = (name, options) => {
    if (name === 'fanfare' && fanfareFrame === null) {
      fanfareFrame = frame;
    }
    return realPlay(name, options);
  };
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

  // Played until it goes down, rather than in a single attempt.
  //
  // The ladder is tuned so an attentive player clears a level about half the
  // time. This bot follows the hints, which is a good deal worse than
  // attentive, so one attempt was a coin flip the moment the budgets were
  // measured rather than guessed. It retries the way a player would, through
  // the page's own button, so the reset is the real one. What this is here to
  // exercise is the winning flow and not the odds of reaching it.
  let attempts = 1;
  // The frame budget has to cover every attempt, not one of them. At forty
  // attempts on a forty-odd move level it takes well over a hundred thousand
  // frames, and the old sixty thousand ran out on the seventeenth: the failure
  // then reads "never finished the level in 17 attempts", which looks like the
  // bot being unlucky rather than the loop being cut off before it had used
  // the attempts it was given.
  for (let i = 0; i < 250_000 && engine.status !== Status.WON; i += 1) {
    if (engine.status === Status.LOST) {
      if (attempts >= 40) {
        break;
      }
      attempts += 1;
      dispatch('retry-button', 'click');
    }
    if (engine.acceptsInput) {
      const move = engine.hint();
      if (move) {
        engine.swap(...move);
      }
    }
    // Counted before the frame is run, not after: the fanfare is played from
    // inside it and would otherwise be stamped with the number of the frame
    // before its own.
    frame += 1;
    pump(1);
    phases.add(engine.phase);
    if (renderer.toast) {
      toasts.add(renderer.toast.text);
      if (renderer.toast.text === 'Objective met' && toastFrame === null) {
        toastFrame = frame;
      }
    }
    if (engine.phase === Phase.CASHING_IN) {
      counterWhileSpending.add(engine.movesLeft);
      for (const cls of elements.get('score').classList.set) {
        scoreClassesWhileSpending.add(cls);
      }
    }
  }
  assert.equal(
    engine.status,
    Status.WON,
    `following the hints never finished the level in ${attempts} attempts`,
  );
  // A met goal goes green, which is the only thing that says so on a chip
  // whose words do not change: a score goal names its target once and counts
  // nothing down, so the color is all there is to read.
  assert.ok(
    objectives.children.every((chip) => chip.classList.contains('met')),
    'the level was won with a goal still unmet on screen',
  );
  // The moves left over when the goal was met are spent on the way out, so a
  // won level always ends on nothing.
  assert.equal(engine.movesLeft, 0, 'the leftover moves were not cashed in');

  assert.ok(phases.has(Phase.CASHING_IN), 'the leftover moves were never spent on screen');
  // And before any of that, a beat with the board still, so the clear that
  // won the level can be seen reaching the goals it counted for. The engine
  // holds it; how long is the page's business, which is why an engine told
  // nothing holds not at all.
  assert.ok(
    phases.has(Phase.TALLYING),
    'the flourish started without waiting for the goals to finish showing',
  );
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
    toasts.has('Objective met'),
    `the level never said it was cleared, only ${JSON.stringify([...toasts])}`,
  );
  // And the fanfare sounded with it, on the same frame. Nothing here has an
  // opinion about what it sounds like; what is checked is that the two are
  // still one moment.
  audio.play = realPlay;
  assert.notEqual(fanfareFrame, null, 'the level was cleared in silence');
  assert.equal(
    fanfareFrame,
    toastFrame,
    `the toast came on frame ${toastFrame} and the fanfare on frame ${fanfareFrame}`,
  );
  assert.ok(
    phases.has(Phase.FINISHING),
    'the board was not held for a beat before the level was declared over',
  );
  pump(3);

  const overlay = elements.get('overlay');
  assert.ok(!overlay.classList.contains('hidden'), 'winning raised no panel');

  // The panel says how the level went and nothing else. What it turned up is
  // in the feed a few inches up the same screen, and saying it twice meant
  // saying it once somewhere that disappears.
  assert.equal(
    elements.get('overlay-title').textContent,
    `Level ${LEVEL + 1} Cleared`,
    'the panel does not say which level was beaten, or how well',
  );
  assert.match(
    elements.get('overlay-body').textContent,
    /^[\d,]+ points on .+\./,
    'the panel does not say what the level was beaten with',
  );
  assert.doesNotMatch(
    elements.get('overlay-body').textContent,
    /Found |Received /,
    'the panel is still announcing an item the feed already holds',
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
      /^Found \S.*\((Level \d+ (Clear|Silver|Gold|AP Gem \d+)|\d+ Chain|Activate \d match)\)$/
        .test(line),
    ),
    `the feed has a line it cannot place: ${JSON.stringify(lines)}`,
  );
  assert.ok(
    lines.some((line) => / Chain\)$/.test(line)),
    `nothing was ever found on a chain, which the placement says it should be: ${JSON.stringify(lines)}`,
  );
  // And a match pays too. Three in a row is the game itself, so a level
  // played out has certainly made one: a run that never found anything there
  // would mean the swap is not being weighed at all.
  assert.ok(
    lines.some((line) => /\(Activate 3 match\)$/.test(line)),
    `lining up three never paid anything: ${JSON.stringify(lines)}`,
  );

  // Each name is colored by what the world makes of that item, the way an
  // Archipelago client colors one. Read off the class rather than the color,
  // since the four colors live in the stylesheet.
  {
    const named = feed.children
      .map((line) => line.children.find((part) => part.className?.startsWith?.('what')))
      .filter(Boolean);
    assert.ok(named.length > 0, 'no line in the feed picks its item out at all');
    const worths = new Set(named.map((part) => part.className));
    assert.ok(
      [...worths].every((className) => /^what (filler|useful|progression|trap)$/.test(className)),
      `the feed named an item without saying what it is worth: ${[...worths].join(', ')}`,
    );
    // The unlocks a level clear pays are progression and the bonus items are
    // useful, so a level played out has found at least two sorts. One sort
    // everywhere would pass every check above and still be a feed that had
    // stopped asking the engine.
    assert.ok(
      worths.size > 1,
      `every item in the feed came out the same color: ${[...worths].join(', ')}`,
    );
  }

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
  // The settings first, exactly as the page does on boot, and before anything
  // is restored: they decide what the fill put where, so a location id looked
  // up in a run dealt under different settings comes back as somebody else's
  // item. Setting one deals the run again, which is why nothing may be handed
  // back before this.
  //
  // This used to be left out, and passed for as long as the two fills happened
  // to agree about the handful of places these ids name. They stopped agreeing
  // the moment the pool was reordered, and the failure read as the save being
  // broken rather than the test asking the wrong run.
  for (const option of restored.options) {
    const value = saved.options?.[option.key];
    if (Number.isInteger(value)) {
      restored.setOption(option.index, value);
    }
  }
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

  // Close gets out of the way and leaves the finished board on screen, which
  // is the whole of what it does. Checked on the real panel rather than on a
  // stand-in, because what is being checked is what the page wired it to.
  click(overlayButton('Close'), 'the finished-level panel cannot be closed');
  assert.ok(
    elements.get('overlay').classList.contains('hidden'),
    'closing the finished-level panel left it on screen',
  );

  // From there the picker is reached the ordinary way, and marks where the
  // player is rather than where they were.
  dispatch('levels-button', 'click', {});
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

  // The popover says what the level was beaten with as well as what it is
  // being measured against, and that number survives a restart the same way
  // the color does: it belongs to the level, not to the attempt.
  const beaten = engine.levelBestScore(LEVEL);
  assert.ok(beaten > 0, 'a level this run has won has no best score against it');
  dispatch('score-box', 'click', {});
  const best = elements.get('score-marks').children.at(-1);
  assert.ok(best.classList.contains('best'), 'the popover has no row for what the run has done');
  assert.equal(
    best.children.map((part) => part.textContent).join(' '),
    `Your best ${beaten.toLocaleString()}`,
  );
  // And the marks it reached are checked off rather than left as targets,
  // however many that turned out to be: which items this run was dealt is the
  // seed's business, and how well it did here follows from them.
  const past = [Tier.SILVER, Tier.GOLD].filter((tier) => engine.levelBest(LEVEL) >= tier).length;
  assert.equal(
    elements.get('score-marks').children.filter((row) => row.classList.contains('taken')).length,
    past,
    'the popover disagrees with the engine about which marks are behind',
  );
  gesture('pointerdown');

  // It goes in the save, because no location records it: a level cleared
  // below its silver checks the same one whatever it scored.
  const record = JSON.parse(store.get(SAVE_KEY));
  assert.ok(Array.isArray(record.bestScores), 'the save does not record what levels were beaten with');
  assert.equal(record.bestScores[LEVEL], beaten, 'the save disagrees with the run');

  // And comes back on reload, without being handed back twice over.
  const rebuilt = new (Object.getPrototypeOf(engine).constructor)(engine.wasm, SEED);
  record.bestScores.forEach((score, at) => rebuilt.restoreBestScore(at, score));
  record.bestScores.forEach((score, at) => rebuilt.restoreBestScore(at, score));
  assert.equal(rebuilt.levelBestScore(LEVEL), beaten, 'a reloaded run forgot its best score');
}

// The panel offers three ways out and no fourth. Where to go next is the
// level select's business: with the ladder opening by item there is not always
// an onward to offer, and a button that sometimes does nothing is worse than
// no button. Close is the one that simply gets out of the way.
{
  const { hud } = window.twiddlygems;
  const { Status } = await import(path.resolve('web/js/engine.js'));
  const real = hud.engine;
  // A stand-in rather than the run itself, so the marks can be moved around
  // underneath it: what is being checked is what the panel makes of them.
  const stub = {
    levelIndex: 3,
    levelCount: 13,
    score: 9_000,
    tiers: { silver: 5_000, gold: 20_000 },
    levelName: 'Test Level',
  };
  let closed = 0;
  const actions = { onRetry() {}, onLevels() {}, onClose: () => { closed += 1; } };
  hud.engine = stub;

  hud.showResult(Status.WON, actions);
  for (const label of ['Levels', 'Replay', 'Close']) {
    assert.ok(overlayButton(label), `the finished-level panel has no ${label} button`);
  }
  assert.ok(!overlayButton('Next level'), 'the panel still offers to go on to the next level');
  assert.equal(
    elements.get('overlay-title').textContent,
    'Level 4 Silver',
    'the panel does not name the mark this attempt reached',
  );

  // Past the gold, and the heading says so rather than saying it again in the
  // body: the heading is the result.
  stub.score = 25_000;
  hud.showResult(Status.WON, actions);
  assert.equal(elements.get('overlay-title').textContent, 'Level 4 Gold');

  // And a clear that reached neither is still a clear.
  stub.score = 1_000;
  hud.showResult(Status.WON, actions);
  assert.equal(elements.get('overlay-title').textContent, 'Level 4 Cleared');

  // Each button calls the action it was handed. That the page hands Close
  // something that hides the panel is checked against the real one, above.
  click(overlayButton('Close'), 'the panel cannot be closed');
  assert.equal(closed, 1, 'closing the panel did nothing');

  // The victory panel, which replaces the clear panel on the one clear that
  // meets the run's goal.
  const quiet = { onLevels() {}, onClose() {} };
  hud.showVictory(quiet);
  assert.equal(
    elements.get('overlay-title').textContent,
    'You win!',
    'the victory panel does not announce the win',
  );
  assert.ok(
    elements.get('overlay-card').classList.contains('victory'),
    'the victory panel is dressed exactly like an ordinary level result',
  );
  for (const label of ['Levels', 'Close']) {
    assert.ok(overlayButton(label), `the victory panel has no ${label} button`);
  }
  // Replaying the level the goal fell on is not a thing anybody wants offered
  // at the end of a run, and the panel has no business suggesting it.
  assert.ok(!overlayButton('Replay'), 'the victory panel offers to replay the level');
  // The wording is free to change. What it must do is name the level and the
  // score, because that is the only thing on the panel a player cannot read
  // off the board behind it. Matched loosely on purpose: this pinned the exact
  // sentence once and failed the moment the wording was improved, which is a
  // test complaining about being right.
  assert.match(
    elements.get('overlay-body').textContent,
    /[\d,]+ points/,
    'the victory panel does not say what the run was won with',
  );

  // And the dress comes off again. The panels replace one another in place
  // without the overlay being hidden in between, so a card left wearing it
  // would turn the next level select gold and call it a win.
  hud.showResult(Status.WON, actions);
  assert.ok(
    !elements.get('overlay-card').classList.contains('victory'),
    'the victory dress outlived the victory panel',
  );

  hud.engine = real;
  hud.hideOverlay();
}

// The testing menu, and the gesture that reaches it.
//
// Reachable in the shipped game, so it is worth knowing both that twenty taps
// open it and that nineteen do not: a gesture that fires early would put a
// cheat menu in front of somebody poking at the goals.
{
  const { engine } = window.twiddlygems;
  const objectives = elements.get('objectives');
  const overlay = elements.get('overlay');
  const options = elements.get('overlay-switches');
  // The chips themselves, since the listener asks whether the tap landed
  // inside the list rather than on the list itself.
  const chip = objectives.children[0];
  assert.ok(chip, 'the level in play has no objective to tap on');

  const tap = (target) => gesture('pointerdown', { target });
  for (let i = 0; i < 19; i += 1) {
    tap(chip);
  }
  assert.ok(
    overlay.classList.contains('hidden'),
    'nineteen taps already opened the testing menu',
  );

  // And the count is consecutive taps, not taps ever made: one anywhere else
  // puts it back to nothing, which is what stops a session accumulating its
  // way in.
  tap(elements.get('score-box'));
  for (let i = 0; i < 19; i += 1) {
    tap(chip);
  }
  assert.ok(
    overlay.classList.contains('hidden'),
    'a tap somewhere else did not reset the count toward the testing menu',
  );

  tap(chip);
  assert.ok(!overlay.classList.contains('hidden'), 'twenty taps did not open the testing menu');
  assert.ok(!options.classList.contains('hidden'), 'the testing menu has no switches');
  assert.equal(
    options.children.length,
    8,
    'the testing menu does not offer the eight shortcuts it is meant to',
  );

  // Nothing is applied until it is closed, so a tester can change their mind.
  const before = engine.unlocked;
  const rowNamed = (name) =>
    options.children.find((row) => row.children.some((part) => part.textContent === name));
  const unlockAll = rowNamed('Unlock all levels');
  assert.ok(unlockAll, 'the testing menu cannot unlock the ladder');
  click(unlockAll, 'the switch does not respond');
  assert.equal(engine.unlocked, before, 'the testing menu applied a switch before it was closed');

  // The sound list, which is the other way out of this menu and the only way
  // the filler sounds can be heard on purpose. One row per noise, named by the
  // engine's own list rather than by anything written here, so a noise added
  // to the engine shows up without this check being edited.
  {
    const { audio } = window.twiddlygems;
    const { NOISES } = await import(path.resolve('web/js/sounds.js'));
    click(overlayButton('Sounds'), 'the testing menu has no way to the sounds');
    assert.equal(elements.get('overlay-title').textContent, 'Filler sounds');
    assert.equal(
      options.children.length,
      NOISES.length,
      'the sound list does not offer one row per noise',
    );
    assert.equal(
      options.children[0].children[0].textContent,
      NOISES[0].name,
      'the sound list is not in the engine order',
    );
    // Pressing one has to start something. Audio is off in this harness, so
    // what is checked is that the press reaches the audio layer at all rather
    // than that anything was heard: the levels are `make audio`'s business.
    const asked = [];
    const real = audio.play;
    audio.play = (name, opts) => {
      asked.push(name);
      return real.call(audio, name, opts);
    };
    const sequenced = [];
    const realSequence = audio.sequence;
    audio.sequence = (parts, opts) => {
      sequenced.push(parts);
      return realSequence.call(audio, parts, opts);
    };
    try {
      for (const row of options.children) {
        click(row, 'a sound row does not respond');
      }
    } finally {
      audio.play = real;
      audio.sequence = realSequence;
    }
    // Three of the sixteen are written figures and go the other way, so both
    // halves of `playNoiseSound` are covered by pressing the whole list.
    assert.ok(asked.length > 0, 'pressing every sound row played nothing');
    assert.ok(
      sequenced.length > 0,
      'none of the sound rows went through the sequencer, so no figure was played',
    );
    assert.equal(
      asked.length + sequenced.length >= NOISES.length,
      true,
      `${NOISES.length} rows pressed produced only ${asked.length + sequenced.length} sounds`,
    );

    // And back, with the switch that was set on the way in still set: a tester
    // going to listen to something should not lose what they were setting up.
    click(overlayButton('Back'), 'the sound list has no way back');
    assert.equal(elements.get('overlay-title').textContent, 'Testing');
    assert.equal(
      rowNamed('Unlock all levels')?.getAttribute('aria-checked'),
      'true',
      'a trip to the sound list forgot what was already switched on',
    );
  }

  click(overlayButton('Close'), 'the testing menu has no way out');
  assert.equal(
    engine.unlocked,
    engine.levelCount,
    'closing the testing menu did not open the ladder',
  );
  assert.ok(overlay.classList.contains('hidden'), 'the testing menu stayed up after closing');
  assert.ok(options.classList.contains('hidden'), 'the switches outlived the menu');

  // The Shuffle Trap, which the testing menu is currently the only way to
  // reach: traps are in the datapackage and in nobody's pool until the yaml
  // percentage exists. Driven from the switch rather than by calling the
  // engine, because the thing not yet covered anywhere is the wiring from a
  // row in that menu through to the board.
  {
    const { Phase } = await import(path.resolve('web/js/engine.js'));
    const board = () => Array.from(engine.snapshot().cells);
    const settled = board();
    // Twenty again from scratch: closing the menu was a tap somewhere else,
    // which is what resets the count.
    for (let i = 0; i < 20; i += 1) {
      tap(chip);
    }
    const shuffle = rowNamed('Spring a Shuffle Trap');
    assert.ok(shuffle, 'the testing menu cannot spring a Shuffle Trap');
    click(shuffle, 'the shuffle switch does not respond');
    click(overlayButton('Close'), 'the testing menu has no way out');
    assert.deepEqual(board(), settled, 'the board moved before a single frame had run');

    // One frame to take the flag, which is what puts the board into the
    // shuffle phase, and then run that phase out: the gems are not rearranged
    // until it ends, since the player watches the board churn and then sees
    // where everything landed.
    //
    // Driven until idle rather than for a fixed count, so a change to
    // `SHUFFLE_MS` cannot quietly turn this into a test that asserts the board
    // was still mid-shuffle. The first pump has to be unconditional: the board
    // is idle before the flag is taken, so a loop that checks first exits
    // without running anything.
    pump(1);
    assert.equal(engine.phase, Phase.SHUFFLING, 'the trap did not start a shuffle');
    for (let i = 0; i < 400 && engine.phase !== Phase.IDLE; i += 1) {
      pump(1);
    }
    assert.notDeepEqual(
      board(),
      settled,
      'the trap was sprung from the menu and the board came out identical',
    );
  }
}

// The seam the victory screen hangs on: the engine deciding the run is over.
//
// The panel is checked above and the trigger reads `goalMet` each frame, so
// between them the one thing neither proves is that the flag ever turns over.
// A screen watching a flag stuck at false never appears, and nothing else in
// the game asks the question, so nothing else would notice.
//
// On a run of its own, because this hands a run its whole ladder and the run
// being played still has assertions of its own to come.
{
  const { engine } = window.twiddlygems;
  const fresh = new (Object.getPrototypeOf(engine).constructor)(engine.wasm, SEED);
  assert.ok(!fresh.goalMet, 'a run that has done nothing already counts as finished');

  // `Location::LevelClear(index)` is numbered by its own index, which is the
  // engine's stable id scheme rather than an accident: the ids go into saves,
  // so they cannot be renumbered. Everything but the last, first, because the
  // goal this asks about is every level and a run one level short of it has
  // not finished.
  for (let index = 0; index < fresh.levelCount - 1; index += 1) {
    fresh.restore(index);
  }
  assert.ok(
    !fresh.goalMet,
    'a run one level short of the whole ladder already counts as finished',
  );

  fresh.restore(fresh.levelCount - 1);
  assert.ok(fresh.goalMet, 'clearing every level does not finish the run');
}

// Ending a run from that same menu asks first, then throws the progress away
// and goes back to the title screen.
{
  const { engine } = window.twiddlygems;
  // However far the run has got by now, read rather than set. `setUnlocked`
  // only ever raises, so a number written in here would have to be above
  // wherever the level above happened to leave it, and this is checking that
  // backing out changes nothing rather than any particular number.
  const opened = engine.unlocked;
  assert.ok(opened > 1, 'the run has opened nothing, so there is no progress to keep or lose');

  // The retry closed the panel, so the menu is opened afresh.
  dispatch('levels-button', 'click', {});
  click(overlayButton('Quit game'), 'the level menu offers no way back to the title');
  click(overlayButton('Keep playing'), 'ending a run is not confirmed first');
  assert.ok(elements.get('title').classList.contains('hidden'), 'backing out still quit the run');
  assert.equal(engine.unlocked, opened, 'backing out still threw the progress away');

  // Backing out returns to the level list, so the way in is open again.
  click(overlayButton('Quit game'), 'backing out closed the menu instead of reopening it');
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

  // Not every setting, though: the four weights are a yaml's business, since
  // shares of a total are no use on a phone. A run gets the ones it would have
  // chosen by leaving them alone, and their places in the table are untouched,
  // which is what the engine sets them by.
  //
  // Counted off `onScreen` rather than off the kind, so a setting hidden for a
  // new reason needs nothing done here.
  {
    const { engine } = window.twiddlygems;
    const hidden = engine.options.filter((option) => !option.onScreen);
    assert.ok(hidden.length > 0, 'nothing in the table is hidden, so this checks nothing');
    assert.equal(
      rows.length,
      engine.options.length - hidden.length,
      'the setup screen is drawing settings it was meant to leave out',
    );
    for (const option of hidden) {
      assert.equal(
        engine.optionValue(option.index),
        option.default,
        `${option.key} is not at its default, so the screen is not the only way to move it`,
      );
    }

    // And a setting that names its values shows the name, not the number. A
    // toggle carries Off and On across the ABI for exactly this: a row
    // reading 1 tells a player nothing about what it is set to.
    const drawn = engine.options.filter((option) => option.onScreen);
    for (const [at, option] of drawn.entries()) {
      if (!option.choices) {
        continue;
      }
      const controls = rows[at].children.find((child) => child.className === 'setup-controls');
      const reading = controls.children.find((child) => child.className === 'setup-value');
      const named = option.choices.map((choice) => choice.label);
      assert.ok(
        named.includes(reading.textContent),
        `${option.key} reads as ${JSON.stringify(reading.textContent)}, `
          + `which is none of ${JSON.stringify(named)}`,
      );
    }

    // The trap percentage is one of the drawn ones, and it is the only
    // setting here that can make a run worse. Worth naming rather than
    // trusting the count: it has been on both sides of this line.
    const traps = drawn.find((option) => option.key === 'trap_percent');
    assert.ok(traps, 'the setup screen does not offer the trap percentage');
    assert.equal(engine.optionValue(traps.index), 0, 'traps did not default to none');
  }

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

// Clearing something that counts toward a goal sends part of it to the chip
// that counts it, instead of only throwing debris where it stood.
//
// Last, because reaching the levels below means opening the whole ladder and
// `setUnlocked` is a floor rather than a setting: a run left holding thirteen
// levels would be the wrong run for everything that came after.
//
// Every kind of goal a level can set, because each is counted differently and
// each is a different chance to send a clear to the wrong chip.
{
  const { EventKind, ObjectiveKind, Status } = await import(path.resolve('web/js/engine.js'));
  const { engine, renderer } = window.twiddlygems;

  // This run was dealt fresh a moment ago, so it is opening the ladder by
  // item, and a floor on how many levels are unlocked means nothing to a run
  // playing that way. What follows is about goals and chips and has no opinion
  // about the ladder, so it is turned back to opening by clearing and then
  // opened. Setting an option deals the run again, which is why it comes
  // first.
  const ladder = engine.options.find((option) => option.key === 'progressive_levels');
  assert.ok(ladder, 'the settings table has no progressive level unlock');
  engine.setOption(ladder.index, 0);
  engine.setUnlocked(engine.levelCount);

  /// Moves to a level by the route a player takes and leaves the air clear, so
  /// what is in flight afterwards is only what this test puts there. The pump
  /// is long enough for anything the deal started to finish.
  ///
  /// Through the picker rather than by calling the engine, because handing the
  /// renderer the new chips is part of what is being checked: a level change
  /// that rebuilt the HUD and forgot to say so would leave every mote flying
  /// at the last level's goals.
  const pickLevel = (index, why) => {
    dispatch('levels-button', 'click', {});
    click(list.children[index], `the picker has no row for level ${index + 1}, ${why}`);
    pump(90);
    renderer.tributes.length = 0;
    renderer.goalFlash.fill(0);
  };

  /// Moves to the first level that satisfies `wanted`, and stays there.
  ///
  /// By what a level is rather than by where it sits. Every one of these used
  /// to be a written-down index, and the ladder has since been reordered and
  /// grown: an index that goes stale does not fail, it quietly exercises a
  /// different level than the one the assertions below are about, which is
  /// worse than failing. `Sticky Middle has no single jelly` was that,
  /// complaining about a level called Donowall.
  const pickLevelWhere = (wanted, why) => {
    for (let index = 0; index < engine.levelCount; index += 1) {
      pickLevel(index, why);
      if (wanted(engine.objectives(), renderer.goals)) {
        return index;
      }
    }
    assert.fail(`no level on the ladder is ${why}`);
    return -1;
  };

  const asks = (kind) => (goals) => goals.some((goal) => goal.kind === kind);
  const onlyAsks = (kind) => (goals) =>
    goals.length === 1 && goals[0].kind === kind;

  /// A cell carrying exactly this much jelly, as the board last stood.
  const jellyCell = (layers) => {
    const at = renderer.jellySeen.findIndex((depth) => depth === layers);
    return at < 0 ? null : { r: Math.floor(at / engine.cols), c: at % engine.cols };
  };

  const clear = (event) => {
    renderer.tributes.length = 0;
    renderer.pendingBursts.length = 0;
    renderer.addEvents([{ value: 0, ...event }], performance.now());
    pump(1);
  };

  // A level with two color goals, so a renderer that sent every clear to the
  // first goal it had would not pass by accident.
  pickLevelWhere(
    (objectives) =>
      objectives.length === 2 && objectives.every((it) => it.kind === ObjectiveKind.COLOR),
    'a level asking for exactly two colors',
  );
  const goals = renderer.goals;
  assert.equal(goals.length, 2, `two color goals did not reach the renderer: ${goals.length}`);
  assert.ok(
    goals.every((goal) => goal.kind === ObjectiveKind.COLOR && goal.el && goal.icon),
    'a goal reached the renderer without a chip to fly to',
  );

  // The second of the two, so a renderer that sent everything to the first
  // goal it had would not pass by accident.
  const target = 1;
  const wanted = goals.map((goal) => goal.color);
  const spare = [0, 1, 2, 3, 4, 5, 6, 7].find((color) => !wanted.includes(color));

  // A synthetic clear rather than a real one: which gem falls where is the
  // seed's business, and what is being checked is where its motes go.
  clear({ kind: EventKind.CLEAR, r: 3, c: 3, color: wanted[target] });
  assert.ok(
    renderer.tributes.length > 0,
    'a gem the level asked for sent nothing to the goal counting it',
  );
  assert.ok(
    renderer.tributes.every((mote) => mote.goal === target),
    'a cleared gem sent motes to a goal it does not count toward',
  );

  // Followed to the end: they have to arrive, light the goal they arrived at
  // and nothing else, and then be gone rather than pinning the layer awake.
  const lit = goals.map(() => 0);
  for (let i = 0; i < 120 && renderer.tributes.length > 0; i += 1) {
    pump(1);
    for (let g = 0; g < lit.length; g += 1) {
      lit[g] = Math.max(lit[g], renderer.goalFlash[g]);
    }
  }
  assert.equal(renderer.tributes.length, 0, 'the motes were still in flight two seconds later');
  assert.ok(lit[target] > 0, 'the goal never lit up as the motes landed');
  assert.equal(lit[1 - target], 0, 'a goal nothing was sent to lit up anyway');

  // And a color this level does not ask for has nowhere to go, so it throws
  // its debris and that is all.
  clear({ kind: EventKind.CLEAR, r: 3, c: 3, color: spare });
  assert.equal(
    renderer.tributes.length,
    0,
    'a gem no goal asks for still sent motes to one of them',
  );

  // The chips themselves count down rather than up: what is left to do is the
  // number being played toward, and the total was never the player's to move.
  const chips = objectives.children;
  assert.equal(chips.length, 2, 'the opening level did not get a chip per goal');
  for (let i = 0; i < chips.length; i += 1) {
    const count = chips[i].children.at(-1).textContent;
    assert.equal(
      count,
      engine.objectives()[i].need.toLocaleString(),
      `a goal nothing has been cleared toward reads ${JSON.stringify(count)}`,
    );
    // And the chip has already made room for that number, so losing a digit
    // on the way down does not narrow it and slide every chip to its right
    // along to take up the slack.
    assert.equal(
      chips[i].children.at(-1).style.minWidth,
      `${String(engine.objectives()[i].need).length}ch`,
      'a counting chip reserved no room for the number it starts at',
    );
  }

  // A jelly goal counts cells rather than layers, so what pays it is the clear
  // that takes the last layer off a cell and not one that only softens it.
  // Sticky Middle is a patch of single layers, so a gem cleared on it finishes
  // it and a gem cleared beside it does nothing.
  const jellyLevel = pickLevelWhere(onlyAsks(ObjectiveKind.JELLY), 'a level whose only goal is jelly');
  assert.equal(renderer.goals.length, 1, 'the jelly level did not get its jelly goal');
  // A patch of jelly runs to two digits, which the opening level's goals do
  // not: a chip that always reserved one digit would look right there and
  // narrow here on the way from ten to nine.
  {
    const wants = engine.objectives()[0].need;
    assert.ok(wants > 9, `this level asks for ${wants}, so its chip needs only one digit`);
    assert.equal(
      objectives.children[0].children.at(-1).style.minWidth,
      `${String(wants).length}ch`,
      'the chip reserved the wrong amount of room for a two digit goal',
    );
  }

  // Played rather than staged, because jelly is the one goal whose answer is
  // not in the event: what a clear was worth depends on what was under the
  // gem, and the engine has already peeled it by the time the event is read.
  // The board has to have been asked a frame earlier, and only a move that
  // really peels one can tell whether it was.
  //
  // The same run watches the chip's number, which has to fall because the
  // motes reached it rather than a second before they set off: the engine
  // counts the cell the moment the gem goes, so a chip reading that counter
  // alone would be done with the clear before anything crossed the screen.
  {
    let sent = 0;
    const realTribute = renderer.tribute.bind(renderer);
    renderer.tribute = (burst) => {
      sent += burst.goals?.length ?? 0;
      realTribute(burst);
    };

    const shown = () => Number(objectives.children[0].children[1].textContent.replace(/,/g, ''));
    const counted = () => {
      const goal = engine.objectives()[0];
      return Math.max(0, goal.need - goal.have);
    };
    let lagged = false;
    let dipped = false;
    let rose = false;

    // Each attempt plays the level right out rather than stopping at the first
    // mote, because what is being watched is the number moving, and it has not
    // moved yet when the first one sets off.
    for (let attempt = 0; attempt < 3 && sent === 0; attempt += 1) {
      // Per attempt, because dealing the level again puts the jelly back.
      let before = shown();
      for (let i = 0; i < 900 && engine.status === Status.PLAYING; i += 1) {
        if (engine.acceptsInput) {
          const move = engine.hint();
          if (move) {
            engine.swap(...move);
          }
        }
        pump(1);
        // The chip is never ahead of the engine, and at some point it is
        // behind: that gap is the motes still crossing the screen.
        lagged = lagged || shown() > counted();
        dipped = dipped || shown() < counted();
        // And it only ever goes down. A burst is held back until the clear
        // reaches its cell, so a chip that stopped counting the wait as owed
        // would drop its number in that gap and take it back when the motes
        // finally set off.
        rose = rose || shown() > before;
        before = shown();
      }
      if (sent === 0) {
        pickLevel(jellyLevel, 'to try the jelly again');
      }
    }
    renderer.tribute = realTribute;
    assert.ok(sent > 0, 'playing the jelly level never sent anything to the goal counting jelly');
    assert.ok(
      lagged,
      'the goal counted a cleared cell before a single mote had reached it',
    );
    assert.ok(
      !dipped,
      'the goal went below what the engine had counted, so it took something twice',
    );
    assert.ok(!rose, 'the number left to do went up, which nothing in a jelly level can do');

    // And it catches up: once the air is clear the chip and the engine agree,
    // which is what stops a dropped mote leaving a level short forever.
    for (let i = 0; i < 200 && shown() !== counted(); i += 1) {
      pump(1);
    }
    assert.equal(shown(), counted(), 'the motes landed and the goal never took them');

    // A mote that never lands must not leave the chip a number short for the
    // rest of the level. One can go missing: the ceiling on how many may be in
    // the air turns a board-wide clear away, and a level change drops whatever
    // was crossing it. So a debt with nothing left to pay it off is written
    // off rather than carried.
    renderer.goalInFlight[0] = 9;
    renderer.tributes.length = 0;
    renderer.pendingBursts.length = 0;
    pump(2);
    assert.equal(shown(), counted(), 'a mote that never arrived left the goal owing forever');
  }

  // A clean board again for the rest, since that one was played on.
  pickLevel(jellyLevel, 'which is the jelly one');
  const sticky = jellyCell(1);
  const bare = jellyCell(0);
  assert.ok(sticky && bare, 'Sticky Middle has no single jelly, or no cell without any');

  clear({ kind: EventKind.CLEAR, ...sticky, color: 0 });
  assert.ok(
    renderer.tributes.length > 0,
    'the gem that took the last jelly layer sent nothing to the goal counting it',
  );
  clear({ kind: EventKind.CLEAR, ...bare, color: 0 });
  assert.equal(renderer.tributes.length, 0, 'a gem cleared off the jelly paid the jelly goal');

  // The other side of that: Hourglass is laid out in double layers, so the
  // first clear on one leaves the cell still jellied and the count unmoved.
  pickLevelWhere(
    (objectives) => asks(ObjectiveKind.JELLY)(objectives) && jellyCell(2) !== null,
    'a level laid out in double jelly',
  );
  const doubled = jellyCell(2);
  assert.ok(doubled, 'no level has double jelly left to soften');
  clear({ kind: EventKind.CLEAR, ...doubled, color: 0 });
  assert.equal(
    renderer.tributes.length,
    0,
    'softening a double layer paid a goal that counts jellied cells',
  );

  // Bricks go the same way: cracking one leaves it in the way, and only the
  // hit that breaks it moves the count.
  pickLevelWhere(onlyAsks(ObjectiveKind.BRICK), 'a level whose only goal is brick');
  assert.equal(renderer.goals.length, 1, 'the brick level did not get its brick goal');
  clear({ kind: EventKind.BRICK, r: 5, c: 4, color: 255, value: 1 });
  assert.equal(renderer.tributes.length, 0, 'a brick that only cracked paid the brick goal');
  clear({ kind: EventKind.BRICK, r: 5, c: 4, color: 255, value: 0 });
  assert.ok(renderer.tributes.length > 0, 'a broken brick sent nothing to the goal counting them');

  // And a seal pays the goal for its own color, of which The Vault has four:
  // the level is about bringing each color to its own seals, so a mote going
  // to the wrong one would be telling the player the opposite of the truth.
  pickLevelWhere(
    (objectives) =>
      objectives.length === 4 && objectives.every((it) => it.kind === ObjectiveKind.SEAL),
    'a level asking for four colors of seal',
  );
  assert.equal(renderer.goals.length, 4, 'the seal level did not get a goal per seal color');
  const seal = 2;
  clear({ kind: EventKind.BRICK, r: 1, c: 1, color: renderer.goals[seal].color, value: 0 });
  assert.ok(renderer.tributes.length > 0, 'a broken seal sent nothing to the goal counting it');
  assert.ok(
    renderer.tributes.every((mote) => mote.goal === seal),
    'a broken seal paid a goal for a color it was not',
  );

  // A goal already met takes nothing more: it is done, and a stream still
  // running into it would say that clearing more of that color was worth
  // something, which is the one thing it is not.
  //
  // Crowded House, because its first goal is a score and its second is a
  // color. The renderer's goals are not the engine's: the score is a chip but
  // not a destination, so the two lists stop agreeing here, and a renderer
  // reading its own index into the engine's objectives would be watching the
  // score to decide whether the color goal was done.
  const scoreThenColor = pickLevelWhere(
    (objectives) =>
      objectives.length === 2 &&
      objectives[0].kind === ObjectiveKind.SCORE &&
      objectives[1].kind === ObjectiveKind.COLOR,
    'a level whose first goal is a score and whose second is a color',
  );
  assert.equal(renderer.goals.length, 1, 'the score goal was offered as a destination');
  assert.equal(
    renderer.goals[0].at,
    1,
    "the color goal is the engine's second, and the renderer thinks otherwise",
  );

  // Played a while, so the counters have moved and the check below is against
  // a board in the middle of something rather than a fresh one.
  for (let i = 0; i < 400 && engine.status === Status.PLAYING; i += 1) {
    if (engine.acceptsInput) {
      const move = engine.hint();
      if (move) {
        engine.swap(...move);
      }
    }
    pump(1);
  }
  pump(2);
  const objective = engine.objectives()[renderer.goals[0].at];
  assert.equal(
    renderer.goalsLeft[0],
    objective.need - objective.have,
    'the renderer disagrees with the engine about how much the goal has left',
  );

  // Set rather than played to: following hints does not meet a goal on any
  // level in the ladder, so a test that waited for one would never run. What
  // the line above checks is that the number is read off the engine at all.
  pickLevel(scoreThenColor, 'again, for a board with moves left on it');
  assert.ok(renderer.goalsLeft[0] > 1, 'a fresh level starts with its goal all but done');
  const asked = renderer.goals[0].color;
  clear({ kind: EventKind.CLEAR, r: 3, c: 3, color: asked });
  assert.ok(renderer.tributes.length > 0, 'the goal took nothing while it still wanted some');

  // A blast spreads outward, so a cell's burst is held back until the clear
  // reaches it. What that burst will deliver counts as on its way from the
  // moment it is queued, not from the moment its motes leave: the engine moved
  // its counter when the gem went, so a chip that waited for the launch would
  // drop its number in the gap and take it back a third of a second later.
  renderer.tributes.length = 0;
  renderer.pendingBursts.length = 0;
  renderer.goalInFlight.fill(0);
  renderer.addEvents(
    [{ kind: EventKind.CLEAR, r: 3, c: 3, color: asked, value: 400 }],
    performance.now(),
  );
  // A frame goes by, which is where the gap would show: the burst is not due
  // for another third of a second, so nothing has set off yet.
  pump(1);
  assert.equal(renderer.tributes.length, 0, 'a burst held back for its blast set off at once');
  assert.ok(
    renderer.unitsInFlight(renderer.goals[0].at) > 0,
    'a clear on its way to a goal was not owed while it waited for its blast',
  );

  renderer.goalsLeft[0] = 0;
  renderer.owedThisFrame[0] = 0;
  clear({ kind: EventKind.CLEAR, r: 3, c: 3, color: asked });
  assert.equal(renderer.tributes.length, 0, 'a goal already met was still being fed');

  // And a goal takes only what it still has to take. The engine stops counting
  // at the total, so clearing three of a color it wanted two more of counts as
  // two: a third cell paying anyway would leave the chip owed three against a
  // goal with two outstanding, and it would climb to three before falling.
  pickLevel(scoreThenColor, 'once more, for a goal with a known amount left');
  renderer.goalsLeft[0] = 2;
  renderer.owedThisFrame[0] = 0;
  renderer.tributes.length = 0;
  renderer.pendingBursts.length = 0;
  renderer.goalInFlight.fill(0);
  const at = renderer.goals[0].at;
  renderer.addEvents(
    [3, 4, 5].map((c) => ({ kind: EventKind.CLEAR, r: 3, c, color: asked, value: 0 })),
    performance.now(),
  );
  assert.equal(
    renderer.unitsInFlight(at),
    2,
    `three gems cleared against two outstanding owed ${renderer.unitsInFlight(at)}`,
  );
}

// A rocket turns the short way round.
//
// Half of its rotation, and the piece whose failure is the loudest: a rocket
// asked to turn ten degrees anticlockwise going the other three hundred and
// fifty instead. The easing that carries it there is traced further down, on
// a rocket spent out of the inventory, which is the only way a rocket gets
// into the air on this run: no board here ever mints one.
{
  const { shortestTurn } = await import(path.resolve('web/js/render.js'));
  const TAU = Math.PI * 2;
  const near = (a, b) => Math.abs(a - b) < 1e-9;
  assert.ok(near(shortestTurn(0.4), 0.4), 'a short turn was made longer');
  assert.ok(near(shortestTurn(-0.4), -0.4));
  // Past half a turn one way is short of half a turn the other.
  assert.ok(near(shortestTurn(Math.PI + 0.4), -Math.PI + 0.4), 'it took the long way round');
  assert.ok(near(shortestTurn(-Math.PI - 0.4), Math.PI - 0.4));
  // And winding up any number of whole turns changes nothing.
  for (const laps of [-3, -1, 1, 5]) {
    assert.ok(
      near(shortestTurn(0.4 + laps * TAU), 0.4),
      `${laps} whole turns either way should come to the same heading`,
    );
  }
}

// ---- what the run has to spend ----
//
// The bottom bar's four slots: what they show, what arming one does, and what
// happens to the board when one is spent.
let flightFrames = 0;
{
  const { Consumable, EMPTY_CELL, Flag, Special } = await import(path.resolve('web/js/engine.js'));
  const { engine, renderer, hud } = window.twiddlygems;
  const inventory = elements.get('inventory');
  const kinds = Object.values(Consumable);

  /// Puts the level up again and waits for it to settle.
  ///
  /// Before each thing spent below, rather than once at the top, because
  /// spending one of these can end the level outright: a rainbow takes a whole
  /// color off the board, which on a color goal is most of what the level was
  /// asking for, and a board that has been won accepts no more input. Reloading
  /// costs nothing here and leaves what the run is carrying alone, which is run
  /// state rather than the level's.
  const freshBoard = (why) => {
    dispatch('retry-button', 'click', {});
    for (let i = 0; i < 900 && !engine.acceptsInput; i += 1) {
      pump(1);
    }
    assert.ok(engine.acceptsInput, `the board never came back to rest for ${why}`);
  };

  /// A cell holding an ordinary gem, and where it is on screen.
  ///
  /// Read off the board rather than aimed at a fixed pixel. What is under any
  /// given point changes with the level and with whatever the last thing spent
  /// took off the board, and two of the cells there are refuse on purpose: a
  /// rainbow pointed at an Archipelago gem is declined rather than wasted, and
  /// a brick holds no gem at all.
  const plainCell = (why) => {
    const { cells } = engine.snapshot();
    for (let i = 0; i < cells.length / 4; i += 1) {
      const colorless = cells[i * 4] === EMPTY_CELL;
      const blocked = (cells[i * 4 + 3] & (Flag.BRICK | Flag.WALL)) !== 0;
      if (!colorless && !blocked && cells[i * 4 + 1] === Special.NONE) {
        const cell = { r: Math.floor(i / engine.cols), c: i % engine.cols };
        return {
          ...cell,
          clientX: renderer.pad + (cell.c + 0.5) * renderer.cell,
          clientY: renderer.pad + (cell.r + 0.5) * renderer.cell,
        };
      }
    }
    throw new assert.AssertionError({ message: `no ordinary gem on the board to aim ${why} at` });
  };

  // Somewhere with room to shoot at, reached the way a player reaches it.
  dispatch('levels-button', 'click', {});
  click(list.children[1], 'the picker has no second row to land on');
  pump(120);
  assert.ok(engine.acceptsInput, 'the board is busy, so nothing below could be spent on it');

  const slots = inventory.children.map((item) => item.children[0]);
  assert.equal(slots.length, kinds.length, `the bar has ${slots.length} slots for ${kinds.length} things`);

  // Emptied first, because by now this run has played several levels and
  // found some of these: what is being checked below is how a slot with
  // nothing in it looks, not what the fill happened to hand over.
  for (const kind of kinds) {
    engine.restoreConsumables(kind, 0);
  }
  pump(1);

  // Every kind has a slot from the start, whether or not the run has any.
  // What a run is out of is worth knowing, and a slot appearing later would
  // shove the rest along under a thumb already coming down.
  for (const slot of slots) {
    assert.ok(slot.children[0], 'a slot was built with no art in it');
    assert.ok(slot.disabled, 'an empty slot can still be tapped');
    assert.ok(slot.classList.contains('empty'), 'an empty slot is not drawn as one');
    assert.ok(slot.children[1].hidden, 'a slot with nothing in it is showing a count');
  }

  // Tapping one anyway arms nothing: the button is disabled, and the page
  // checks the count again rather than trusting the last frame's drawing.
  click(slots[Consumable.ROCKET], 'the rocket slot');
  assert.equal(hud.armed, null, 'an empty slot armed itself');

  engine.restoreConsumables(Consumable.ROCKET, 2);
  engine.restoreConsumables(Consumable.ROCKET_CLUSTER, 1);
  pump(1);
  const rocket = slots[Consumable.ROCKET];
  assert.ok(!rocket.disabled && !rocket.classList.contains('empty'), 'a slot with two in it is dim');
  assert.equal(rocket.children[1].hidden, false, 'a slot holding something shows no count');
  assert.equal(rocket.children[1].textContent, '2', 'the count on the slot is wrong');
  assert.ok(slots[Consumable.RAINBOW].disabled, 'a kind the run has none of came up available');

  // Arming, and the two ways back out of it.
  click(rocket, 'the rocket slot');
  assert.equal(hud.armed, Consumable.ROCKET, 'tapping a full slot did not arm it');
  pump(1);
  assert.ok(rocket.classList.contains('armed'), 'the armed slot is not drawn as armed');
  click(rocket, 'the rocket slot');
  assert.equal(hud.armed, null, 'tapping the armed item again did not put it away');

  click(rocket, 'the rocket slot');
  gesture('pointerdown', { target: elements.get('feed') });
  assert.equal(hud.armed, null, 'tapping away from the board left it armed');

  // And the board does not count as away from it: that is where it goes.
  click(rocket, 'the rocket slot');
  gesture('pointerdown', { target: elements.get('board') });
  assert.equal(hud.armed, Consumable.ROCKET, 'aiming at the board put the item away');

  const SELECTED = 4;
  const anySelected = () =>
    engine.snapshot().cells.some((byte, at) => at % 4 === 3 && (byte & SELECTED) !== 0);

  // An aim the engine refuses still takes the tap, and this is the case the
  // whole guard is for. A spend that lands leaves the board busy, so nothing
  // could fall through it anyway; a refusal leaves the board exactly as it
  // was, and without the guard the tap goes on to select the gem under it.
  // The player meant to point at something and is left mid-swap.
  //
  // Refused here by emptying the run's pocket behind the armed item, which is
  // the one refusal that can be arranged on any board. Still armed from the
  // check above, which is why nothing arms it again here.
  engine.restoreConsumables(Consumable.ROCKET, 0);
  const refused = plainCell('a refused rocket');
  dispatch('board', 'pointerdown', refused);
  dispatch('board', 'pointerup', refused);
  // A frame, because selecting a gem marks the board and the board is only
  // written out for reading on the tick after. Without this the check below
  // reads the frame before the tap and passes whatever happened.
  pump(1);
  assert.equal(hud.armed, Consumable.ROCKET, 'a refused aim put the item away');
  assert.ok(!anySelected(), 'a refused aim fell through and selected the gem under it');
  engine.restoreConsumables(Consumable.ROCKET, 2);

  // Spending it. The tap is the aim and nothing else: no gem is selected and
  // no move is spent, because a bonus is not a turn.
  const movesBefore = engine.movesLeft;
  const heldBefore = engine.consumables(Consumable.ROCKET);
  dispatch('board', 'pointerdown', plainCell('a rocket'));
  assert.equal(hud.armed, null, 'spending it left it armed');
  assert.equal(engine.consumables(Consumable.ROCKET), heldBefore - 1, 'the run was not charged');
  assert.equal(engine.movesLeft, movesBefore, 'spending a bonus cost a move');
  assert.equal(engine.phase, 3, `the tap did not put a rocket in the air: phase ${engine.phase}`);
  assert.ok(!anySelected(), 'aiming at a cell selected the gem in it as well');

  // In the air, and turning as it goes. This is the only rocket that flies on
  // this run, so it is the only chance to trace the easing: a rocket comes
  // round to its heading rather than snapping to it, and what that comes to
  // is a cap on how far it may turn in one frame.
  const MOST_PER_FRAME = 0.0095 * 16;
  const angles = [];
  let painted = 0;
  for (let i = 0; i < 200 && engine.phase === 3; i += 1) {
    const before = fxCalls.drawImage;
    pump(1);
    if (renderer.flights.length > 0) {
      angles.push(renderer.flights[0].angle);
      painted += fxCalls.drawImage > before ? 1 : 0;
    }
  }
  // Drawn, and on the effects layer: the board's own canvas stops at the
  // board, and this one sets off from under it, out of the bar it was tapped
  // in. Worked out but never painted is a rocket nobody sees.
  assert.equal(painted, angles.length, `the rocket went unpainted on ${angles.length - painted} of its frames`);
  flightFrames = angles.length;
  assert.ok(angles.length > 5, `the rocket was only drawn in flight for ${angles.length} frames`);
  assert.ok(
    Math.abs(angles.at(-1) - angles[0]) > 0.2,
    'the rocket flew the whole way without ever turning',
  );
  for (let i = 1; i < angles.length; i += 1) {
    const step = Math.abs(angles[i] - angles[i - 1]);
    assert.ok(
      step <= MOST_PER_FRAME + 1e-6,
      `the rocket swung ${step.toFixed(3)} radians in one frame, which is a flip`,
    );
  }

  // And the cap on top of the easing, which that flight never needed: the
  // easing takes a fifth of whatever is left over each frame, and a fifth of
  // a small angle is small. The cap is for a rocket pointed somewhere else
  // entirely, where a fifth of the gap is still a flip. Straight down, from a
  // rocket pointing straight up, is the worst case there is: half a turn.
  renderer.frameMs = 16;
  const spun = renderer.turnToward(0, 100, { x: 0, y: 0, angle: 0, wants: 0 }, true);
  assert.ok(Math.abs(spun.angle) > 0, 'a rocket pointed the wrong way never came round at all');
  assert.ok(
    Math.abs(spun.angle) <= MOST_PER_FRAME + 1e-6,
    `it turned ${spun.angle.toFixed(3)} radians in one frame, which is a flip`,
  );

  // A rocket's drawn position moves for reasons that are not flight: it falls
  // when the gems under it clear, and slides when it is swapped. Only travel
  // is a heading. Read the other way round, a rocket minted mid-cascade turned
  // to face the way it was dropping and hung there, nose down, until it fired.
  //
  // Both halves, because a rule that never turns anything would pass the first
  // of these on its own.
  const drop = (flying) => {
    let at;
    for (let i = 0; i < 40; i += 1) {
      at = renderer.turnToward(100, 100 + i * 3, at, flying);
    }
    return at.angle;
  };
  const degrees = (radians) => ((radians * 180) / Math.PI).toFixed(1);
  assert.ok(
    Math.abs(drop(false)) < 1e-9,
    `a rocket that only fell came to rest ${degrees(drop(false))} degrees off upright`,
  );
  assert.ok(
    Math.abs(Math.abs(drop(true)) - Math.PI) < 0.05,
    `a rocket flying downward pointed ${degrees(drop(true))} degrees rather than straight down`,
  );

  // It struck something: the board has work to do that the tap started.
  pump(120);
  assert.equal(renderer.flights.length, 0, 'a rocket is still being drawn after it landed');

  // And the run's stock went into the save, which is the only place a solo
  // run can keep it.
  const saved = JSON.parse(store.get(SAVE_KEY));
  assert.equal(
    saved.consumables?.[Consumable.ROCKET],
    engine.consumables(Consumable.ROCKET),
    'what the run has left to spend was not saved',
  );

  // A rainbow raises its whole first clear inside the call that spends it,
  // and the engine only ever reports what the last call raised. Leaving that
  // for the next tick to notice drops every pop, every piece of debris and
  // every mote of it, so the page has to collect them then and there.
  freshBoard('the rainbow');
  engine.restoreConsumables(Consumable.RAINBOW, 1);
  pump(1);
  renderer.pendingBursts.length = 0;
  renderer.particles.length = 0;
  click(slots[Consumable.RAINBOW], 'the rainbow slot');
  dispatch('board', 'pointerdown', plainCell('a rainbow'));
  assert.equal(engine.consumables(Consumable.RAINBOW), 0, 'the rainbow was not spent');
  assert.ok(
    renderer.pendingBursts.length + renderer.particles.length > 0,
    'the clear a spent rainbow raised was never collected, so none of it was drawn',
  );

  // The cluster aims itself, so tapping it is the whole gesture.
  freshBoard('the cluster');
  click(slots[Consumable.ROCKET_CLUSTER], 'the cluster slot');
  assert.equal(hud.armed, null, 'the cluster asked for a cell');
  assert.equal(engine.consumables(Consumable.ROCKET_CLUSTER), 0, 'the cluster was not spent');
  assert.equal(engine.phase, 3, 'tapping the cluster put nothing in the air');
  pump(1);
  assert.ok(renderer.flights.length >= 3, `a cluster is three rockets at least: ${renderer.flights.length}`);
  pump(200);
}

// ---- voices are released by the audio clock ----
//
// A voice used to be freed by `setTimeout`. A phone under load delays those
// and a backgrounded tab throttles them hard, and once the releases fall
// behind the arrivals the count sticks at the cap: every later sound of that
// name is dropped, so the game goes quiet or lets a few through in pieces.
//
// Note what pins the "no timer" half of this: the stub window below has no
// `setTimeout` at all, so anything reaching for one throws rather than quietly
// working here and failing on a phone.
{
  const { Audio } = await import(path.resolve('web/js/audio.js'));

  let disconnects = 0;
  const param = () => ({
    value: 0,
    setValueAtTime() {},
    linearRampToValueAtTime() {},
    exponentialRampToValueAtTime() {},
  });
  const audioNode = () => ({
    frequency: param(), Q: param(), gain: param(), detune: param(), pan: param(),
    threshold: param(), knee: param(), ratio: param(), attack: param(), release: param(),
    connect() {},
    disconnect() { disconnects += 1; },
    start() {},
    stop() {},
  });
  const ctx = {
    currentTime: 0,
    sampleRate: 48000,
    state: 'running',
    createDynamicsCompressor: audioNode,
    createGain: audioNode,
    createOscillator: audioNode,
    createBufferSource: audioNode,
    createBiquadFilter: audioNode,
    createStereoPanner: audioNode,
    createBuffer: (channels, frames) => ({
      duration: frames / 48000,
      getChannelData: () => new Float32Array(frames),
    }),
    destination: audioNode(),
  };

  const audio = new Audio();
  audio.attach(ctx);

  // A level's worth of clears with the clock running, which is the case that
  // went silent: nothing here is over the cap at any one instant.
  let played = 0;
  let dropped = 0;
  for (let round = 0; round < 40; round += 1) {
    ctx.currentTime += 0.25;
    for (let i = 0; i < 3; i += 1) {
      if (audio.play('pop', { delay: 0 })) {
        played += 1;
      } else {
        dropped += 1;
      }
    }
  }
  assert.equal(dropped, 0, `${dropped} of ${played + dropped} pops were dropped over a whole level`);
  assert.ok(disconnects > 0, 'nothing was ever unplugged from the master bus');

  // And the cap still caps. Twenty at one instant is a rainbow taking a whole
  // color, and past ten of them the extra copies are inaudible under the rest.
  const burst = new Audio();
  burst.attach(ctx);
  let loud = 0;
  for (let i = 0; i < 20; i += 1) {
    loud += burst.play('pop', { delay: 0 }) ? 1 : 0;
  }
  assert.ok(loud > 0 && loud < 20, `twenty at once let ${loud} through, which is not a cap`);

  // Once they have rung out the voices come back, without anything having to
  // fire on time for them to.
  ctx.currentTime += 5;
  assert.ok(burst.play('pop', { delay: 0 }), 'the voices never came back after the sound ended');
}

// ---- the level picker, while it is open ------------------------------------
//
// An item arriving while the panel is up is written into it, not rebuilt
// around it. Under a multiworld the run changes while nobody is playing it,
// and this is the panel most likely to be read at exactly that moment.
//
// The rows are kept by identity, because that is the whole of the ask:
// replacing them would scroll the list back, drop whatever has focus, and
// interrupt a drag in progress.
//
// Last in this file on purpose. The items it receives are real, and a moves
// upgrade changes what the flourish has left to spend, which earlier checks
// count.
// A closed panel is not kept up to date. `hideOverlay` puts away what is over
// the board rather than unpicking what is inside it, so the tracker's own
// class stays true long after the menu is gone: test it alone and the frame
// loop walks sixteen rows forever over a panel nobody can see.
{
  const { engine } = window.twiddlygems;
  const real = engine.levelGems.bind(engine);
  let asked = 0;
  engine.levelGems = (at) => {
    asked += 1;
    return real(at);
  };

  dispatch('levels-button', 'click', {});
  pump(2);
  assert.ok(asked > 0, 'the open picker was never looked at');
  click(overlayButton('Close'), 'the level picker has no way out');
  asked = 0;
  pump(5);
  assert.equal(asked, 0, 'a closed picker is still being walked every frame');
  engine.levelGems = real;
}

{
  const { engine } = window.twiddlygems;
  dispatch('levels-button', 'click', {});
  const rows = elements.get('level-list');
  const slots = elements.get('tracker-items');
  const lit = () => slots.children.filter((slot) => slot.classList.contains('found')).length;

  // A level still waiting on its upgrade, found rather than assumed: this run
  // was dealt from a seed of its own, and which items the opening level paid
  // out is not something to write down here.
  const at = [...Array(engine.levelCount).keys()].find((i) => {
    const mark = engine.levelMoves(i);
    return mark.total > 0 && mark.found === 0;
  });
  assert.ok(at !== undefined, 'this run has found every moves upgrade already');
  const total = engine.levelMoves(at).total;

  const row = rows.children[at];
  const slot = slots.children[0];
  const label = row.getAttribute('aria-label');
  const before = lit();

  // That upgrade and an unlock, the way a room hands them over: one changes a
  // row, the other changes the tracker above it. By the special's own code,
  // which is what the item id is; 0 is "no special" and 6 is an Archipelago
  // gem, and neither is a thing to hold.
  assert.ok(engine.receive(1_000 + at), 'the engine refused the moves upgrade');
  const unlock = [1, 2, 3, 4, 5].find((code) => !engine.unlockedSpecials.has(code));
  assert.ok(unlock !== undefined, 'this run holds every unlock, so nothing can arrive');
  assert.ok(engine.receive(unlock), 'the engine refused the unlock');
  pump(1);

  assert.equal(rows.children[at], row, 'the picker rebuilt its rows instead of writing to them');
  assert.equal(slots.children[0], slot, 'the tracker rebuilt its slots');
  assert.notEqual(row.getAttribute('aria-label'), label, 'the row never noticed the item');
  assert.match(
    row.getAttribute('aria-label'),
    new RegExp(`1 of ${total} moves upgrades`),
    'the row does not say the upgrade arrived',
  );
  assert.equal(lit(), before + 1, 'the tracker did not light the unlock that arrived');
  assert.match(
    elements.get('overlay-body').textContent,
    /\d+ of \d+ unlocked/,
    'the panel stopped saying how much of the ladder is open',
  );
  click(overlayButton('Close'), 'the level picker has no way out');
}

// ---- the connection overlay -----------------------------------------------
//
// What a player sees when the room stops listening. The words themselves are
// put together in `main.js` against a live client, which is `ap_smoke`'s half;
// this is the half that has to hold on the page: a wash that says one thing in
// a live region and counts down outside it, and goes away on its own when the
// room comes back.
{
  const { hud } = window.twiddlygems;
  const link = elements.get('link');
  const word = elements.get('link-word');
  const aside = elements.get('link-aside');

  // The countdown is the reason for the split. It changes every second, and a
  // screen reader asked to read a live region every second is a game that
  // cannot be played with one.
  assert.match(
    markup,
    /id="link-word"[^>]*aria-live="polite"/,
    'the connection state is not in a live region, so it is announced to nobody',
  );
  assert.match(
    markup,
    /id="link-aside"[^>]*aria-hidden="true"/,
    'the countdown is inside the live region, so it would be read out every second',
  );
  assert.doesNotMatch(markup, /id="link"[^>]*aria-live/, 'the whole overlay is a live region');

  hud.showLink('Disconnected.', { kind: 'bad', aside: ' (reconnecting in 2s)' });
  assert.ok(!link.classList.contains('hidden'), 'the overlay stayed hidden while disconnected');
  assert.ok(link.classList.contains('bad'), 'a lost connection was not marked as bad');
  assert.equal(word.textContent, 'Disconnected.');
  assert.equal(aside.textContent, ' (reconnecting in 2s)');

  // Being told is no reason to be stopped from playing, which is the whole
  // point of it being a wash rather than a dialog. The stylesheet is what
  // makes that true, so that is what is read.
  const style = await readFile(new URL('../web/css/style.css', import.meta.url), 'utf8');
  const rule = /#link \{[^}]*\}/.exec(style);
  assert.ok(rule, 'the overlay has no rule of its own in the stylesheet');
  assert.match(rule[0], /pointer-events:\s*none/, 'the overlay would swallow taps meant for the board');
  assert.match(rule[0], /position:\s*absolute/, 'the overlay is not laid over the board');

  // Coming back says so and then gets out of the way. The only message that
  // goes away on its own: every other one is a state the player is still in.
  const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  hud.showLink('Connected.', { kind: 'good', fade: true });
  assert.ok(link.classList.contains('good'), 'coming back was not marked as good');
  assert.ok(!link.classList.contains('hidden'), 'the good news was hidden before it was read');
  await wait(1_900);
  assert.ok(link.classList.contains('hidden'), '"Connected." never went away');

  // And a message arriving part way through that fade is not swallowed by it.
  // A room can drop again while the good news about the last one is still on
  // its way out, and the timer that was going to hide that must not hide this.
  hud.showLink('Connected.', { kind: 'good', fade: true });
  await wait(1_300);
  assert.ok(link.classList.contains('fading'), 'the fade never started, so this proves nothing');
  hud.showLink('Disconnected.', { kind: 'bad' });
  assert.ok(!link.classList.contains('hidden'), 'the next message inherited the fade before it');
  assert.ok(!link.classList.contains('fading'), 'the next message started part way faded out');
  assert.equal(aside.textContent, '', 'a countdown was left over from the message before');
  await wait(600);
  assert.ok(
    !link.classList.contains('hidden'),
    'a fade left over from the message before took this one away with it',
  );

  hud.showLink('');
  assert.ok(link.classList.contains('hidden'), 'leaving the room left the overlay up');

  // A held reconnection resumes because something asked to be told the tab was
  // looked at. Nothing else here would notice if that were never registered,
  // and the symptom would be a tab that stays disconnected until a reload.
  assert.ok(
    (documentListeners.get('visibilitychange') ?? []).length > 0,
    'nothing on the page listens for the tab being looked at',
  );
  // And switching away and back is not something a run notices.
  look('hidden');
  look('visible');
  assert.ok(link.classList.contains('hidden'), 'coming back to the tab put up a connection message');
}

console.log(
  `page ok: ${framesRun} frames, ${calls.drawImage} blits, ${calls.fill} fills, ` +
    `${calls.stroke} strokes, ${objectives.children.length} objective chips, ` +
    `${list.children.length} levels listed, ` +
    `a spent rocket turned over ${flightFrames} frames, ` +
    `a swipe scored and spent a move (${scoreBefore} -> played -> reset)`,
);
