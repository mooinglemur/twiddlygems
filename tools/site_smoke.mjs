#!/usr/bin/env node
// Loads the game out of the server that ships it, in a real browser.
//
// `make smoke` drives the front end against a stubbed DOM and `make shots`
// drives it against a development server that hands out the files under their
// plain names. Neither one exercises the thing that is only true of the built
// image: that everything but the page is served under a prefix carrying a
// fingerprint of the build, and that every relative path inside the site still
// finds what it is looking for once it is under there.
//
// That is a fiddly property with a quiet failure. A module importing `./engine.js`
// resolves it against its own URL and so lands under the prefix on its own, but
// the engine reaching for the wasm through `fetch` resolves against the
// *document*, which is not under the prefix. Get that wrong and the page loads,
// the modules load, and the game stops at a blank board.
//
//   make site-smoke

import { spawn } from 'node:child_process';
import assert from 'node:assert/strict';

const SERVER = process.env.SITE_BIN ?? 'target/release/twiddlygems-serve';
const PORT = Number(process.env.SITE_PORT ?? 8097);
const DEBUG_PORT = Number(process.env.SITE_DEBUG_PORT ?? 9335);
// The same default as `tools/shoot.mjs`, so one `BROWSER` setting covers both.
const BROWSER = process.env.BROWSER ?? 'google-chrome-stable';

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const server = spawn(SERVER, [], {
  stdio: ['ignore', 'pipe', 'pipe'],
  env: { ...process.env, PORT: String(PORT), BIND: '127.0.0.1' },
});
let serverSaid = '';
server.stdout.on('data', (chunk) => (serverSaid += chunk));
server.stderr.on('data', (chunk) => (serverSaid += chunk));
server.on('error', (error) => {
  console.error(`could not start ${SERVER}: ${error.message}`);
  console.error("build it with 'make site'.");
  process.exit(1);
});

const browser = spawn(
  BROWSER,
  [
    '--headless', '--no-sandbox', '--disable-gpu', '--hide-scrollbars', '--mute-audio',
    `--remote-debugging-port=${DEBUG_PORT}`,
    '--user-data-dir=/tmp/twiddlygems-site-smoke',
    'about:blank',
  ],
  { stdio: 'ignore' },
);
browser.on('error', (error) => {
  console.error(`could not start ${BROWSER}: ${error.message}`);
  console.error('set BROWSER to a chrome or chromium binary on your machine.');
  process.exit(1);
});

// Both are children of this process and both are killed through their own
// handles. Nothing here is ever looked up by name.
const stop = () => {
  browser.kill();
  server.kill();
};
process.on('exit', stop);

async function waitFor(url, attempts = 100) {
  for (let i = 0; i < attempts; i += 1) {
    try {
      if ((await fetch(url)).ok) return true;
    } catch {
      // Not up yet, which is the ordinary case for the first few tries.
    }
    await sleep(100);
  }
  throw new Error(`${url} never came up:\n${serverSaid}`);
}

await waitFor(`http://127.0.0.1:${PORT}/healthz`);
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
/// Every response the page took, so what it actually asked for can be checked
/// rather than inferred from the page still working.
const served = [];
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
    thrown.push(data.params.exceptionDetails.exception?.description ?? data.params.exceptionDetails.text);
  } else if (data.method === 'Runtime.consoleAPICalled' && data.params.type === 'error') {
    thrown.push(data.params.args.map((a) => a.value ?? a.description).join(' '));
  } else if (data.method === 'Network.responseReceived') {
    served.push({ url: data.params.response.url, status: data.params.response.status });
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
    awaitPromise: true,
  });
  if (exceptionDetails) {
    throw new Error(
      `evaluating ${expression.trim().slice(0, 60)}: ` +
        `${exceptionDetails.exception?.description ?? exceptionDetails.text}`,
    );
  }
  return result.value;
};

await send('Page.enable');
await send('Runtime.enable');
await send('Network.enable');
// Nothing may come out of a cache: what this is checking is what the server
// hands over, and a second run reusing the first one's files would check
// nothing at all.
await send('Network.setCacheDisabled', { cacheDisabled: true });

// `?debug` puts the engine on `window.twiddlygems`, which is how this reaches
// past the title screen to ask whether the module really loaded.
await send('Page.navigate', { url: `http://127.0.0.1:${PORT}/?debug` });
await sleep(2500);

// The game is up, which means the page, every module and the wasm all arrived.
const booted = await evaluate(`
  (() => {
    const game = window.twiddlygems;
    if (!game) return null;
    return { rows: game.engine.rows, cols: game.engine.cols, levels: game.engine.levelCount };
  })()
`);
assert.ok(booted, `the game never started. Page errors:\n${thrown.join('\n') || '(none)'}`);
assert.ok(booted.rows > 0 && booted.cols > 0, 'the board has no size');
assert.ok(booted.levels > 0, 'no levels loaded');
assert.deepEqual(thrown, [], 'the page reported errors');

// Nothing 404ed, which is the failure this whole arrangement invites: a path
// that resolved against the document instead of its module lands outside the
// prefix and is not there.
const missing = served.filter((response) => response.status >= 400);
assert.deepEqual(missing, [], 'something the page asked for was not served');

const fingerprinted = /\/a\/[0-9a-f]{16}\//;
const wasm = served.find((response) => response.url.endsWith('.wasm'));
assert.ok(wasm, 'the module was never fetched, so the board is drawn from nothing');
assert.match(
  wasm.url,
  fingerprinted,
  `the module came from outside the fingerprinted prefix: ${wasm.url}`,
);

// And so did every module and the stylesheet, which is what makes them safe to
// cache forever.
for (const response of served) {
  const path = new URL(response.url).pathname;
  // The page is the one thing that is deliberately not, and the icon is the
  // one thing that is deliberately not there: the browser asks for it whatever
  // the page says, and the server answers 204 so it asks once.
  if (path === '/' || path === '/favicon.ico') {
    continue;
  }
  assert.match(response.url, fingerprinted, `${path} is served without a fingerprint`);
}
assert.ok(
  served.some((response) => response.url.endsWith('/favicon.ico') && response.status === 204),
  'the icon nobody has was not answered, so every visit asks for it again',
);

// The page itself must never be cached, or a player keeps last week's
// fingerprints and never sees a new build at all.
const page = await fetch(`http://127.0.0.1:${PORT}/`);
assert.match(page.headers.get('cache-control') ?? '', /no-cache/, 'the page is cacheable');
const asset = await fetch(wasm.url);
assert.match(
  asset.headers.get('cache-control') ?? '',
  /immutable/,
  'a fingerprinted file is not cached forever, which is the point of the fingerprint',
);
assert.equal(asset.headers.get('content-type'), 'application/wasm');

console.log(
  `site ok: served by the shipping binary, ${served.length} requests, ` +
    `all fingerprinted, ${booted.levels} levels on a ${booted.rows}x${booted.cols} board`,
);
// All three of these hold the event loop open, so without closing them the run
// finishes its work and then sits there forever.
socket.close();
stop();
