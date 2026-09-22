#!/usr/bin/env node
// Plays the game in a real headless browser and writes screenshots.
//
// This is the only way to see the thing from a terminal, and it is a real end
// to end check: a served page, a real wasm module, real input events, and the
// frame loop running in real time. It reports anything the page threw.
//
// Note for anyone reaching for `--screenshot` instead: Chrome's
// `--virtual-time-budget` freezes the compositor, so requestAnimationFrame
// fires two or three times and the game never advances. Screenshots taken that
// way show a first frame and nothing else. Hence the DevTools Protocol.
//
//   make shots

import { writeFile, mkdir } from 'node:fs/promises';
import { spawn } from 'node:child_process';

const OUT = process.argv[2] ?? 'shots';
const BROWSER = process.env.BROWSER ?? 'google-chrome-stable';
const PORT = Number(process.env.SHOT_PORT ?? 8099);
const DEBUG_PORT = Number(process.env.SHOT_DEBUG_PORT ?? 9333);
// Which level to photograph. Level 1 has specials switched off, so point this
// at a later one to see line, cross and rocket gems on a real board.
const LEVEL = Number(process.env.SHOT_LEVEL ?? 0);

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
await mkdir(OUT, { recursive: true });

const server = spawn('python3', ['-m', 'http.server', String(PORT), '--bind', '127.0.0.1', '--directory', 'web'], {
  stdio: 'ignore',
});
const browser = spawn(
  BROWSER,
  [
    // Muted because this plays the game for real, sound and all, and headless
    // or not it comes out of whatever speakers the machine is using.
    '--headless', '--no-sandbox', '--disable-gpu', '--hide-scrollbars', '--mute-audio',
    `--remote-debugging-port=${DEBUG_PORT}`,
    '--user-data-dir=/tmp/twiddlygems-shots',
    'about:blank',
  ],
  { stdio: 'ignore' },
);
browser.on('error', (error) => {
  console.error(`could not start ${BROWSER}: ${error.message}`);
  console.error('set BROWSER to a chrome or chromium binary on your machine.');
  process.exit(1);
});

const stop = () => {
  browser.kill();
  server.kill();
};
process.on('exit', stop);

async function waitFor(url, attempts = 100) {
  for (let i = 0; i < attempts; i += 1) {
    try {
      if ((await fetch(url)).ok) return true;
    } catch {}
    await sleep(100);
  }
  throw new Error(`${url} never came up`);
}

await waitFor(`http://127.0.0.1:${PORT}/index.html`);
await waitFor(`http://127.0.0.1:${DEBUG_PORT}/json/version`);

const target = await (
  await fetch(`http://127.0.0.1:${DEBUG_PORT}/json/new?about:blank`, { method: 'PUT' })
).json();
const socket = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.onopen = resolve;
  socket.onerror = reject;
});

let nextId = 1;
const pending = new Map();
const thrown = [];
socket.addEventListener('message', (message) => {
  const data = JSON.parse(message.data);
  if (data.id && pending.has(data.id)) {
    const { resolve, reject } = pending.get(data.id);
    pending.delete(data.id);
    if (data.error) {
      reject(new Error(JSON.stringify(data.error)));
    } else {
      resolve(data.result);
    }
  } else if (data.method === 'Runtime.exceptionThrown') {
    thrown.push(data.params.exceptionDetails);
  } else if (data.method === 'Runtime.consoleAPICalled' && data.params.type === 'error') {
    thrown.push(data.params.args.map((a) => a.value ?? a.description).join(' '));
  }
});

const send = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, { resolve, reject });
    socket.send(JSON.stringify({ id, method, params }));
  });

const evaluate = async (expression) => {
  const { result, exceptionDetails } = await send('Runtime.evaluate', {
    expression,
    returnByValue: true,
    // So an expression can wait on frames going by and hand back what it
    // found, rather than returning a promise nobody unwraps.
    awaitPromise: true,
  });
  // Without this a script that throws comes back as `undefined` and the run
  // carries on: the page is never driven, every shot is of whatever was on
  // screen before, and nothing anywhere says so.
  if (exceptionDetails) {
    const thrown = exceptionDetails.exception?.description ?? exceptionDetails.text;
    throw new Error(`evaluating ${expression.trim().slice(0, 80)}: ${thrown}`);
  }
  return result.value;
};

async function shoot(name) {
  const { data } = await send('Page.captureScreenshot', { format: 'png' });
  await writeFile(`${OUT}/${name}.png`, Buffer.from(data, 'base64'));
}

/** A press, an optional drag, and a release: a tap or a swipe. */
async function gesture(x, y, dx = 0, dy = 0) {
  const shared = { button: 'left', clickCount: 1, buttons: 1 };
  await send('Input.dispatchMouseEvent', { type: 'mousePressed', x, y, ...shared });
  if (dx || dy) {
    await sleep(20);
    await send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: x + dx, y: y + dy, ...shared });
  }
  await sleep(20);
  await send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: x + dx, y: y + dy, ...shared });
}

const readout = () =>
  evaluate(`({
    score: document.getElementById('score').textContent,
    moves: document.getElementById('moves').textContent,
    objective: document.querySelector('.objective-count')?.textContent ?? null,
    level: document.getElementById('level-name').textContent,
  })`);

await send('Page.enable');
await send('Runtime.enable');
// The profile lives across runs, and only the wasm module is fetched
// no-cache, so without this the page comes back with yesterday's JS and the
// screenshot quietly shows the last version of the front end.
await send('Network.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });

for (const [name, metrics] of [
  ['phone', { width: 390, height: 844, deviceScaleFactor: 3, mobile: true }],
  ['desktop', { width: 1280, height: 860, deviceScaleFactor: 1, mobile: false }],
]) {
  await send('Emulation.setDeviceMetricsOverride', metrics);
  // A fresh seed each run would make shots incomparable, so pin one.
  await send('Page.navigate', { url: `http://127.0.0.1:${PORT}/index.html` });
  await sleep(600);
  await evaluate(
    // Unlocked past the end of the ladder, so any level can be photographed
    // without playing up to it, and holding the five unlocks, which are the
    // items on the first five level clears (locations 0 to 4). Without those
    // the board makes no specials at all and the shots are of a much plainer
    // game than anyone past the opening level plays.
    //
    // Plus a silver and a gold on a couple of levels (2000 and 3000 up), so
    // the level picker has one of each to show rather than a grid of green.
    `localStorage.setItem('twiddlygems.save.v1', JSON.stringify({ seed: 20260920, unlocked: 99, level: ${LEVEL}, checked: [0, 1, 2, 3, 4, 2001, 3001, 2002] }))`,
  );
  // `?debug` puts the engine, renderer and HUD on `window.twiddlygems`, which
  // is how the shots below reach past the board to things an ordinary run only
  // gets to by playing for a while.
  await send('Page.navigate', { url: `http://127.0.0.1:${PORT}/index.html?debug` });
  await sleep(2200);

  // The game opens on its menu now, so that gets photographed and then
  // dismissed: nothing below can reach the board until a mode is chosen.
  await shoot(`${name}-00-title`);
  await evaluate(`document.getElementById('solo-button').click()`);
  await sleep(400);

  const box = await evaluate(`
    (() => {
      const r = document.getElementById('board').getBoundingClientRect();
      return { left: r.left, top: r.top, width: r.width };
    })()
  `);
  const cell = box.width / 8.4;
  const pad = cell * 0.2;
  const at = (r, c) => ({
    x: box.left + pad + (c + 0.5) * cell,
    y: box.top + pad + (r + 0.5) * cell,
  });

  await shoot(`${name}-01-fresh`);

  const selected = at(4, 3);
  await gesture(selected.x, selected.y);
  await sleep(300);
  await shoot(`${name}-02-selected`);
  await gesture(selected.x, selected.y);
  await sleep(200);

  // Swipe around until the board has some score on it.
  for (let i = 0; i < 40; i += 1) {
    const from = at(i % 8, (i * 3) % 7);
    await gesture(from.x, from.y, cell, 0);
    await sleep(420);
  }
  await shoot(`${name}-03-played`);
  const state = await readout();

  // The item feed holds its height empty, so the shot above is the case that
  // matters most. This one is what it looks like with something in it, which
  // an ordinary run only reaches by clearing a level.
  await evaluate(`
    (() => {
      const { hud } = window.twiddlygems;
      hud.logItem({ said: 'Found ', what: 'Vertical Line Clear', where: 'Level 1 Clear' });
      hud.logItem({ said: 'Found ', what: 'Horizontal Line Clear', where: '5 Chain' });
      hud.logItem({ said: 'Received ', what: 'Level 4 Progressive Moves', where: null });
    })()
  `);
  await sleep(200);
  await shoot(`${name}-04-feed`);

  // The end of a level: the goal met, the moves left over being spent one at
  // a time, and each gem turning into a special throwing motes. Played out
  // through the engine's own hints rather than by gesture, because forty more
  // swipes is a minute of screenshot run time.
  const cashingIn = await evaluate(`
    (async () => {
      const { engine, Phase } = { ...window.twiddlygems, Phase: { CASHING_IN: 7 } };
      for (let i = 0; i < 400; i += 1) {
        if (engine.phase === Phase.CASHING_IN) {
          return true;
        }
        if (engine.acceptsInput) {
          const move = engine.hint();
          if (move) { engine.swap(...move); }
        }
        await new Promise((done) => requestAnimationFrame(done));
      }
      return engine.phase === Phase.CASHING_IN;
    })()
  `);
  if (!cashingIn) {
    console.error('never reached the end-of-level run down, so there is no shot of it');
    stop();
    process.exit(1);
  }
  await sleep(600);
  await shoot(`${name}-05-cashing-in`);

  await evaluate(`document.getElementById('levels-button').click()`);
  await sleep(400);
  await shoot(`${name}-06-levels`);

  // `hidden` is a utility class, and every panel it goes on is an id selector
  // that sets its own `display`, which outweighs a bare class. When that goes
  // wrong the JS looks right and nothing throws: the panel simply stays on
  // screen, which is how a stale level picker sat under the end-of-level
  // buttons unnoticed. Only a real browser computes this, so it is checked
  // here rather than in the stubbed page smoke.
  const stuck = await evaluate(`
    (() => {
      const bad = [];
      for (const element of document.querySelectorAll('[id]')) {
        const had = element.classList.contains('hidden');
        element.classList.add('hidden');
        if (getComputedStyle(element).display !== 'none') {
          bad.push(element.id);
        }
        if (!had) {
          element.classList.remove('hidden');
        }
      }
      return bad;
    })()
  `);
  if (stuck.length) {
    console.error(`the "hidden" class does not hide: ${stuck.join(', ')}`);
    stop();
    process.exit(1);
  }

  await evaluate(`document.querySelector('#overlay-buttons button').click()`);
  await sleep(200);

  console.log(`${name}: ${JSON.stringify(state)}`);
}

if (thrown.length) {
  console.error(`the page reported ${thrown.length} error(s):`);
  console.error(JSON.stringify(thrown, null, 2).slice(0, 3000));
  stop();
  process.exit(1);
}

console.log(`shots ok: no page errors, written to ${OUT}/`);
socket.close();
stop();
