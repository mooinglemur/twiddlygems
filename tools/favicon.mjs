#!/usr/bin/env node
// Draws the browser-tab icon out of the game's own gem painter.
//
// The ruby in the tab is the ruby on the board: same palette entry, same
// shape, same highlight, because this runs `paintGemIcon` in a real browser
// rather than redrawing something that looks like it. If the palette moves,
// re-running this moves the icon with it.
//
// The result is committed, not built. It changes approximately never, and
// making the image depend on a headless browser would mean the container build
// needed one.
//
//   make favicon
//
// An .ico is a small container: a header, one directory entry per size, and
// then the images. Each image here is a PNG, which every browser since IE11
// reads, and which means the canvas can be asked for the bytes directly rather
// than a BMP being assembled by hand with its upside-down rows and its
// separate mask.

import { writeFile } from 'node:fs/promises';
import { spawn } from 'node:child_process';

const OUT = process.argv[2] ?? 'web/favicon.ico';
const PORT = Number(process.env.ICON_PORT ?? 8095);
const DEBUG_PORT = Number(process.env.ICON_DEBUG_PORT ?? 9337);
const BROWSER = process.env.BROWSER ?? 'google-chrome-stable';
/// Ruby, which is `PALETTE[0]`.
const COLOR = Number(process.env.ICON_COLOR ?? 0);
/// What goes in the file. 16 and 32 are what browsers actually ask for; 48 is
/// for the places that want something bigger and would otherwise scale 32 up.
const SIZES = [16, 32, 48];

if (typeof WebSocket === 'undefined') {
  console.error(`this needs Node 22.4 or newer for its WebSocket; this is ${process.version}.`);
  process.exit(1);
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// A page is needed at all only because the painter draws onto a canvas, and a
// canvas needs a document. Anything serving the module will do.
const server = spawn(
  'python3',
  ['-m', 'http.server', String(PORT), '--bind', '127.0.0.1', '--directory', 'web'],
  { stdio: 'ignore' },
);
const browser = spawn(
  BROWSER,
  [
    '--headless', '--no-sandbox', '--disable-gpu', '--hide-scrollbars', '--mute-audio',
    `--remote-debugging-port=${DEBUG_PORT}`,
    '--user-data-dir=/tmp/twiddlygems-favicon',
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
    } catch {
      // Not up yet.
    }
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
socket.addEventListener('message', (message) => {
  const data = JSON.parse(message.data);
  if (data.id && pending.has(data.id)) {
    const { resolve, reject } = pending.get(data.id);
    pending.delete(data.id);
    if (data.error) reject(new Error(JSON.stringify(data.error)));
    else resolve(data.result);
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
      `${exceptionDetails.exception?.description ?? exceptionDetails.text}`,
    );
  }
  return result.value;
};

await send('Page.enable');
await send('Runtime.enable');
await send('Page.navigate', { url: `http://127.0.0.1:${PORT}/index.html` });
await sleep(800);

const drawn = await evaluate(`
  (async () => {
    const { paintGemIcon } = await import('http://127.0.0.1:${PORT}/js/render.js');
    const out = {};
    for (const size of ${JSON.stringify(SIZES)}) {
      const canvas = document.createElement('canvas');
      paintGemIcon(canvas, size, ${COLOR});
      out[size] = canvas.toDataURL('image/png').split(',')[1];
    }
    return out;
  })()
`);

const images = SIZES.map((size) => ({ size, bytes: Buffer.from(drawn[size], 'base64') }));
for (const image of images) {
  // A PNG, not whatever the canvas felt like. The container says nothing about
  // what is inside each entry, so a reader finding something else would simply
  // show nothing.
  if (image.bytes.subarray(0, 8).toString('hex') !== '89504e470d0a1a0a') {
    throw new Error(`the ${image.size}px image is not a PNG`);
  }
}

// ICONDIR: reserved, type 1 (icon), count. Then six-byte-plus entries, then
// the images themselves.
const HEADER = 6;
const ENTRY = 16;
const header = Buffer.alloc(HEADER);
header.writeUInt16LE(0, 0);
header.writeUInt16LE(1, 2);
header.writeUInt16LE(images.length, 4);

let offset = HEADER + ENTRY * images.length;
const entries = images.map((image) => {
  const entry = Buffer.alloc(ENTRY);
  // 0 means 256 in this field, which is why it is a byte for sizes up to 256.
  entry.writeUInt8(image.size === 256 ? 0 : image.size, 0);
  entry.writeUInt8(image.size === 256 ? 0 : image.size, 1);
  entry.writeUInt8(0, 2); // colors in the palette: 0 for truecolor
  entry.writeUInt8(0, 3); // reserved
  entry.writeUInt16LE(1, 4); // color planes
  entry.writeUInt16LE(32, 6); // bits per pixel
  entry.writeUInt32LE(image.bytes.length, 8);
  entry.writeUInt32LE(offset, 12);
  offset += image.bytes.length;
  return entry;
});

const ico = Buffer.concat([header, ...entries, ...images.map((image) => image.bytes)]);
await writeFile(OUT, ico);
console.log(
  `favicon ok: ${OUT}, ${images.map((image) => `${image.size}px`).join(' ')}, ${ico.length} bytes`,
);

socket.close();
stop();
