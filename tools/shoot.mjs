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
  const { result } = await send('Runtime.evaluate', { expression, returnByValue: true });
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
    // without playing up to it.
    `localStorage.setItem('twiddlygems.save.v1', JSON.stringify({ seed: 20260920, unlocked: 99, level: ${LEVEL} }))`,
  );
  await send('Page.navigate', { url: `http://127.0.0.1:${PORT}/index.html` });
  await sleep(2200);

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

  await evaluate(`document.getElementById('levels-button').click()`);
  await sleep(400);
  await shoot(`${name}-04-levels`);
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
