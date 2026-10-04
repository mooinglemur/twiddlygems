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
import { request as httpRequest } from 'node:http';
import assert from 'node:assert/strict';

// See the same guard in `shoot.mjs`: this drives the browser over a WebSocket,
// which Node only grew in 22.4.
if (typeof WebSocket === 'undefined') {
  console.error(`this needs Node 22.4 or newer for its WebSocket; this is ${process.version}.`);
  process.exit(1);
}

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
    return {
      rows: game.engine.rows,
      cols: game.engine.cols,
      levels: game.engine.levelCount,
      version: game.engine.version,
      footer: document.getElementById('build-version').textContent,
    };
  })()
`);
assert.ok(booted, `the game never started. Page errors:\n${thrown.join('\n') || '(none)'}`);
assert.ok(booted.rows > 0 && booted.cols > 0, 'the board has no size');
assert.ok(booted.levels > 0, 'no levels loaded');

// Which build this is, off the page the binary served rather than off a file
// on disk. The footer is written from the module, so this says the module in
// the image is the one the page describes.
//
// The shape only. A tree with no checkout to ask builds a legitimate binary
// that says `unknown`, and failing here would make that build unbuildable;
// the image is where that is refused, because there the commit is handed in.
assert.match(booted.version, /^\d+\.\d+\.\d+\+\S+$/, `the build names itself "${booted.version}"`);
assert.equal(booted.footer, booted.version, 'the footer and the module disagree about the build');
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
  served.some((response) => response.url.endsWith('/favicon.ico') && response.status === 200),
  'the browser never got an icon',
);

// ---- compression ----
//
// There is no compressing proxy in front of this, so the server does it, with
// an encoder written in this repository. That makes this the test that matters
// most about it: the gzip stream has to be one that zlib and a real browser
// both accept, and nothing inside the encoder can tell us whether it is.
{
  const url = wasm.url;
  // Node's fetch offers gzip and inflates what comes back, so a body that
  // matches the identity copy means the stream decoded correctly through
  // somebody else's zlib rather than through anything of ours.
  const squeezed = Buffer.from(await (await fetch(url)).arrayBuffer());
  const plain = Buffer.from(
    await (await fetch(url, { headers: { 'accept-encoding': 'identity' } })).arrayBuffer(),
  );
  assert.ok(plain.length > 0, 'the module came back empty');
  assert.ok(squeezed.equals(plain), 'what came back compressed is not what came back plain');

  // And that it really was compressed on the wire, which the check above
  // cannot see: fetch hides the encoding by handling it.
  const raw = await new Promise((resolve, reject) => {
    const request = httpRequest(url, { headers: { 'accept-encoding': 'gzip' } }, (response) => {
      const chunks = [];
      response.on('data', (chunk) => chunks.push(chunk));
      response.on('end', () => resolve({ headers: response.headers, body: Buffer.concat(chunks) }));
    });
    request.on('error', reject);
    request.end();
  });
  assert.equal(raw.headers['content-encoding'], 'gzip', 'the module was sent uncompressed');
  assert.equal(raw.headers.vary, 'Accept-Encoding', 'a cache would serve the wrong form to someone');
  assert.equal(Number(raw.headers['content-length']), raw.body.length, 'the length was wrong');
  assert.ok(
    raw.body.length < plain.length * 0.8,
    `the module only went from ${plain.length} to ${raw.body.length} bytes`,
  );
  assert.deepEqual([...raw.body.subarray(0, 3)], [0x1f, 0x8b, 8], 'that is not a gzip stream');

  // A client that says it cannot take gzip must not be sent it. This is the
  // one that breaks somebody rather than merely wasting bandwidth.
  const refused = await new Promise((resolve, reject) => {
    const request = httpRequest(url, { headers: { 'accept-encoding': 'gzip;q=0' } }, (response) => {
      response.resume();
      resolve(response.headers);
    });
    request.on('error', reject);
    request.end();
  });
  assert.equal(refused['content-encoding'], undefined, 'gzip;q=0 was read as a yes');
}

// The page itself must never be cached, or a player keeps last week's
// fingerprints and never sees a new build at all.
const page = await fetch(`http://127.0.0.1:${PORT}/`);
assert.match(page.headers.get('cache-control') ?? '', /no-cache/, 'the page is cacheable');

// The policy the page carries. Checked here as well as in the engine's own
// tests because this is the copy that went over a socket, and because the
// browser above had to load the whole game under it: a policy that blocked the
// module would have failed the boot check rather than this one.
const policy = page.headers.get('content-security-policy') ?? '';
assert.match(policy, /wasm-unsafe-eval/, 'the module could not be instantiated under this policy');
assert.match(policy, /connect-src [^;]*wss:/, 'a multiworld would be blocked by this policy');

// What the orchestrator asks. The two must differ: liveness follows the first
// and readiness the second, and a shutdown moves only one of them.
const healthz = await fetch(`http://127.0.0.1:${PORT}/healthz`);
const readyz = await fetch(`http://127.0.0.1:${PORT}/readyz`);
assert.equal(healthz.status, 200, 'the process does not say it is alive');
assert.equal(readyz.status, 200, 'the process does not say it is ready');

// The monitoring contract. A probe on the cluster asserts exactly this and
// pages somebody when it stops matching, so it is checked here rather than
// discovered at three in the morning.
const html = await page.text();
assert.match(
  html,
  /src="\/a\/[0-9a-f]{16}\/js\/main\.js"/,
  'the page no longer matches the string the uptime probe looks for',
);
const asset = await fetch(wasm.url);
assert.match(
  asset.headers.get('cache-control') ?? '',
  /immutable/,
  'a fingerprinted file is not cached forever, which is the point of the fingerprint',
);
assert.equal(asset.headers.get('content-type'), 'application/wasm');

console.log(
  `site ok: served by the shipping binary, ${served.length} requests, all fingerprinted, ` +
    `gzip round-trips, ${booted.levels} levels on a ${booted.rows}x${booted.cols} board`,
);
// All three of these hold the event loop open, so without closing them the run
// finishes its work and then sits there forever.
socket.close();
stop();
