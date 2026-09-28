#!/usr/bin/env node
// Plays a real multiworld against a real Archipelago server.
//
// Everything else about the client is checked against a socket this repository
// wrote, scripted from a reading of the protocol document. That is worth
// having and it has one hole in it: if the reading is wrong, the stub is wrong
// the same way and the tests agree with both. This is the one that cannot be,
// because the other end is Archipelago's own `MultiServer`, loading a seed our
// own apworld generated, over a real socket.
//
// What it proves that nothing else does: the handshake is accepted, the slot
// data arrives in the shape the client expects, a location check reaches the
// server and comes back as the item that was sitting in it, and a reconnection
// is told what the run had already done.
//
//   make ap-live
//
// The server is a child of this process and is killed through that handle. It
// is never looked up by name.

import { spawn } from 'node:child_process';
import { connect } from 'node:net';
import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import path from 'node:path';

import { loadEngine } from '../web/js/engine.js';
import { ArchipelagoClient, MemoryCache, State } from '../web/js/archipelago.js';

const multidata = process.argv[2];
const WASM = process.argv[3] ?? 'web/twiddlygems.wasm';
const PORT = Number(process.env.AP_PORT ?? 38281);
const SLOT = process.env.AP_SLOT ?? 'Tester';
const AP = 'vendor/Archipelago';
const PYTHON = '.venv/bin/python';

assert.ok(multidata, 'usage: ap_live.mjs <multidata.archipelago> [module.wasm]');

const bytes = await readFile(WASM);
globalThis.fetch = async () => new Response(bytes, { headers: { 'content-type': 'application/wasm' } });

/** Waits for something to become true, or says what it was waiting for. */
async function waitFor(what, predicate, ms = 10_000) {
  const until = Date.now() + ms;
  while (Date.now() < until) {
    if (predicate()) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`timed out waiting for ${what}`);
}

/** Whether anything is listening yet. */
function listening(port) {
  return new Promise((resolve) => {
    const socket = connect({ host: '127.0.0.1', port }, () => {
      socket.destroy();
      resolve(true);
    });
    socket.on('error', () => resolve(false));
  });
}

// ---- the server ----

const server = spawn(
  path.resolve(PYTHON),
  [
    'MultiServer.py',
    '--host',
    '127.0.0.1',
    '--port',
    String(PORT),
    // Nothing here should outlive the run: a savefile would hand the next run
    // a room that had already been played.
    '--disable_save',
    '--loglevel',
    'warning',
    path.resolve(multidata),
  ],
  {
    cwd: AP,
    // Its console reads stdin. Given none it stops asking, which is what a
    // server run by a supervisor gets anyway.
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, SKIP_REQUIREMENTS_UPDATE: '1' },
  },
);

let serverSaid = '';
server.stdout.on('data', (chunk) => (serverSaid += chunk));
server.stderr.on('data', (chunk) => (serverSaid += chunk));

/// Filled in once there is a client; read by the failure report below, which
/// has to work whether or not we ever got that far.
let lastStates = () => [];

/// Every client opened, so the end of the run can close them all.
///
/// An open socket keeps node's event loop alive, so a run that fails an
/// assertion part way through would otherwise sit there until something else
/// killed it rather than reporting and stopping. Which is exactly what it did.
const clients = [];

let stopped = false;
const stop = () => {
  if (!stopped) {
    stopped = true;
    // The child handle, never a name. This process started it and this is the
    // only thing it kills.
    server.kill('SIGTERM');
  }
};
process.on('exit', stop);

server.on('exit', (code) => {
  if (!stopped) {
    console.error(serverSaid);
    throw new Error(`the server stopped on its own with code ${code}`);
  }
});

try {
  let up = false;
  const until = Date.now() + 30_000;
  while (!up && Date.now() < until) {
    up = await listening(PORT);
    if (!up) {
      await new Promise((resolve) => setTimeout(resolve, 200));
    }
  }
  if (!up) {
    console.error(serverSaid);
    throw new Error(`the server never listened on ${PORT}`);
  }

  // ---- the client ----

  const engine = await loadEngine('unused', 20260920);
  const feed = [];
  const states = [];
  const client = new ArchipelagoClient(engine, {
    cache: new MemoryCache(),
    onFeed: (said) => feed.push(said),
    onState: (state, detail) => states.push([state, detail]),
  });
  lastStates = () => states;
  clients.push(client);

  await client.connect({ host: '127.0.0.1', port: PORT, slot: SLOT });
  await waitFor('the server to let us in', () => client.state === State.PLAYING);

  // The datapackage came off the wire and was turned over into names. Ours is
  // in it because we are in the room, and the names are the ones the engine
  // wrote out.
  assert.ok(client.names.size > 0, 'no datapackage was read');
  assert.equal(typeof client.seed, 'string', 'the room never said which seed it is');

  // A real seed, generated by the real world, carries the generation it was
  // built to, and this build plays it. If it did not, the connection above
  // would have been refused rather than reaching here, so this is really
  // asserting that the number survived the whole trip.
  assert.ok(
    engine.playsGenerator(engine.generator),
    'this build does not play the generation it writes',
  );

  // The settings this seed was generated with reached the engine. These are
  // the ones `tools/twiddlygems.yaml` asks for, which are deliberately not the
  // defaults, so a client that ignored the slot data would fail here.
  const valueOf = (key) => {
    const option = engine.options.find((it) => it.key === key);
    return engine.optionValue(option.index);
  };
  assert.equal(valueOf('progressive_levels'), 1, 'the ladder setting did not cross the wire');
  assert.equal(valueOf('inventory_items'), 15, 'the bonus item count did not cross the wire');
  assert.equal(valueOf('rocket'), 200, 'a weight did not cross the wire');
  assert.equal(valueOf('rocket_cluster'), 0, 'a weight of nothing did not cross the wire');

  // The multiworld owns the placement now, so nothing here pays itself.
  assert.equal(engine.unlocked, 1, 'the ladder is open before anything opened it');

  // ---- play until something gets checked ----

  // Not until a level is won: the opening level is a three move puzzle and
  // what this is about is the wire, not whether a bot can solve it. Lining up
  // three gems is a location of its own and happens almost at once.
  let moves = 0;
  while (engine.checkedCount === 0 && moves < 200) {
    const move = engine.hint();
    if (move) {
      engine.swap(...move);
      moves += 1;
    }
    for (let i = 0; i < 200 && engine.phase !== 0; i += 1) {
      engine.update(16);
    }
    engine.update(16);
    client.poll();
    if (engine.movesLeft === 0) {
      engine.retry();
    }
  }
  assert.ok(engine.checkedCount > 0, `nothing got checked in ${moves} moves`);
  client.poll();
  const checked = engine.checked.map((id) => id + client.base);

  // A one player room holds nothing but our own items, so a location we check
  // is an item we are sent. Which is the whole loop: our check went out, the
  // server looked it up, and it came back as an item and as a message about
  // it.
  await waitFor(
    'the server to send back what was in the location',
    () => feed.some((said) => said.type === 'ItemSend' && said.mine),
  );
  const told = feed.find((said) => said.type === 'ItemSend' && said.mine);
  assert.ok(told.text.length > 0, 'the message about it was empty');
  assert.ok(
    told.parts.some((part) => part.kind === 'item' && part.text && !/^Item \d+$/.test(part.text)),
    `an item id was never resolved to a name: ${told.text}`,
  );
  assert.ok(
    told.parts.some((part) => part.kind === 'location' && !/^Location \d+$/.test(part.text)),
    `a location id was never resolved to a name: ${told.text}`,
  );

  // ---- and the reconnection ----

  // The part most likely to be wrong, against the only thing that can say so.
  // A fresh engine, so what it ends up holding can only have come from the
  // server.
  client.disconnect();
  const second = await loadEngine('unused', 20260920);
  const back = new ArchipelagoClient(second, { cache: new MemoryCache() });
  clients.push(back);
  await back.connect({ host: '127.0.0.1', port: PORT, slot: SLOT });
  await waitFor('the second connection', () => back.state === State.PLAYING);
  await waitFor('the run to be handed back what it had done', () => second.checkedCount > 0);

  const remembered = second.checked.map((id) => id + back.base);
  for (const id of checked) {
    assert.ok(
      remembered.includes(id),
      `the room forgot location ${id}, which this run had checked`,
    );
  }
  // And it was handed back what it held, without ever having found it itself.
  await waitFor(
    'the items to come back',
    () => second.unlocked > 1 || second.unlockedSpecials.size > 0 || second.movesTotal > 0,
  );

  back.disconnect();
  console.log(
    `ap live ok: real server, ${client.names.size} games in the datapackage, ` +
      `${checked.length} location(s) checked and remembered across a reconnect`,
  );
} catch (error) {
  // A failure here is about two processes talking, so the two halves of that
  // conversation are what somebody needs to see. Without them this is a
  // timeout with nothing to go on.
  console.error(`\n${error.message}\n`);
  console.error('--- what the client went through ---');
  for (const [state, detail] of lastStates()) {
    console.error(`  ${state}${detail ? `: ${detail}` : ''}`);
  }
  console.error('--- what the server said ---');
  console.error(serverSaid.split('\n').slice(-25).join('\n'));
  process.exitCode = 1;
} finally {
  // Both halves, in this order. A client left connected holds the event loop
  // open and the run never ends; a server left running holds the port and the
  // next run cannot start.
  for (const client of clients) {
    client.disconnect();
  }
  stop();
}
