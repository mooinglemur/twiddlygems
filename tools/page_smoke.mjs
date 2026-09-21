#!/usr/bin/env node
// Boots the real front end against the real wasm module, with the browser
// stubbed out just far enough to run it.
//
// This is not a substitute for opening the page: nothing here knows what the
// board looks like. What it does catch is the whole class of faults that turn
// the page blank — a bad import, a misspelled DOM id, a canvas call that does
// not exist, an exception in the frame loop — none of which the engine's own
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
    'fillRect', 'strokeRect', 'translate', 'scale', 'rotate', 'drawImage',
  ];
  for (const name of methods) {
    ctx[name] = () => {
      calls[name] = (calls[name] ?? 0) + 1;
    };
  }
  ctx.createLinearGradient = () => ({ addColorStop() {} });
  ctx.createRadialGradient = () => ({ addColorStop() {} });
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
    append(...nodes) { this.children.push(...nodes); },
    replaceChildren(...nodes) { this.children = nodes; },
    addEventListener(type, handler) {
      const key = `${id}:${type}`;
      listeners.set(key, [...(listeners.get(key) ?? []), handler]);
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
  'board', 'stage', 'level-number', 'level-name', 'score', 'moves', 'objectives',
  'overlay', 'overlay-title', 'overlay-body', 'overlay-buttons', 'level-grid',
  'levels-button', 'hint-button', 'retry-button', 'sound-button',
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
globalThis.window = {
  devicePixelRatio: 2,
  location: { search: '', href: 'http://localhost/index.html' },
  localStorage: {
    getItem: (key) => store.get(key) ?? null,
    setItem: (key, value) => store.set(key, value),
  },
  addEventListener() {},
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

await import(path.resolve('web/js/main.js'));

// boot() is async and pulls in a module graph, so give it real time rather
// than a fixed number of microtask turns — that raced as soon as another
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
assert.equal(elements.get('level-number').textContent, 'Level 1');
assert.ok(elements.get('level-name').textContent.length > 0, 'the level has no name on screen');
assert.equal(elements.get('moves').textContent, '20');
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
const movesAfterPlay = Number(elements.get('moves').textContent);
assert.ok(movesAfterPlay < 20, 'a scoring swap did not cost a move');

// The hint button has to produce a highlight without throwing.
dispatch('hint-button', 'click', {});
pump(2);

// Restart, which reloads the level and rebuilds the HUD.
dispatch('retry-button', 'click', {});
pump(5);
assert.equal(elements.get('score').textContent, '0', 'restarting did not reset the score on screen');
assert.equal(elements.get('moves').textContent, '20', 'restarting did not restore the moves');

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

// The level picker builds one chip per level, with the locked ones disabled.
dispatch('levels-button', 'click', {});
const grid = elements.get('level-grid');
assert.ok(grid.children.length >= 10, 'the level picker is missing levels');
assert.equal(grid.children[0].disabled, false, 'level one is locked');
assert.equal(grid.children[9].disabled, true, 'a level nobody has reached is unlocked');

console.log(
  `page ok: ${framesRun} frames, ${calls.drawImage} blits, ${calls.fill} fills, ` +
    `${calls.stroke} strokes, ${objectives.children.length} objective chips, ` +
    `${grid.children.length} levels listed, ` +
    `a swipe scored and spent a move (${scoreBefore} -> played -> reset)`,
);
