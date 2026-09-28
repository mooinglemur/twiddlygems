#!/usr/bin/env node
// Drives the Archipelago client through a whole conversation, without a server.
//
// The client's job is a state machine over a socket and a set of calls into the
// engine, so that is what this exercises: a stub socket scripted with the
// packets a real server sends, a real wasm session on the other side, and
// assertions about what came back out on the wire and what the run ended up
// holding.
//
// The wire itself is not what this is about. What it checks is the part that
// would be wrong in a way nobody notices: that a reconnection does not double
// the items, that spent things stay spent, that the same check is never sent
// twice, and that a seed built by a different version is refused rather than
// played.
//
//   make smoke

import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';

import { loadEngine, Consumable } from '../web/js/engine.js';
import { ArchipelagoClient, MemoryCache, State, addressesFor, GAME } from '../web/js/archipelago.js';

const path = process.argv[2] ?? 'web/twiddlygems.wasm';

// The engine module is loaded through `fetch`, which node has, but not from a
// file path. A tiny shim keeps the front end's own loader in play rather than
// instantiating the module a second way here, which would test a path the
// browser never takes.
const bytes = await readFile(path);
globalThis.fetch = async () => new Response(bytes, { headers: { 'content-type': 'application/wasm' } });
// The client reaches for these to remember a uuid between visits. Failing to
// is a supported outcome, but the warning it prints is noise in a test.
globalThis.window = { localStorage: { getItem: () => null, setItem: () => {} } };

/** A socket that goes nowhere, with everything the client listens for. */
class StubSocket {
  constructor(url) {
    this.url = url;
    this.readyState = 1;
    this.sent = [];
    this.listeners = new Map();
    // Opened on the next turn of the loop, the way a real one would be: the
    // client attaches its handlers after the call returns.
    queueMicrotask(() => this.fire('open', {}));
  }

  addEventListener(kind, handler) {
    this.listeners.set(kind, [...(this.listeners.get(kind) ?? []), handler]);
  }

  removeEventListener(kind, handler) {
    this.listeners.set(kind, (this.listeners.get(kind) ?? []).filter((it) => it !== handler));
  }

  fire(kind, event) {
    for (const handler of [...(this.listeners.get(kind) ?? [])]) {
      handler(event);
    }
  }

  send(text) {
    this.sent.push(...JSON.parse(text));
  }

  close() {
    this.readyState = 3;
    this.fire('close', {});
  }

  /** What the server says back. */
  deliver(...packets) {
    this.fire('message', { data: JSON.stringify(packets) });
  }

  /** Everything of one kind the client has sent, oldest first. */
  ofKind(cmd) {
    return this.sent.filter((packet) => packet.cmd === cmd);
  }

  lastOfKind(cmd) {
    return this.ofKind(cmd).at(-1);
  }
}

/** Lets the client's own promises settle before asserting on what it did. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

const AP = 7_477_000;
const ITEM = {
  ROCKET: AP + 3_000,
  RAINBOW_UNLOCK: AP + 4,
  LEVEL_UNLOCK: AP + 4_000,
  MOVES_1: AP + 1_000,
};

/** A room to connect to, as the server introduces it. */
function roomInfo(checksums = {}) {
  return {
    cmd: 'RoomInfo',
    seed_name: 'test-seed',
    datapackage_checksums: checksums,
    games: Object.keys(checksums),
    password: false,
  };
}

function connected(engine, over = {}) {
  return {
    cmd: 'Connected',
    team: 0,
    slot: 1,
    players: [{ team: 0, slot: 1, name: 'twiddly', alias: 'twiddly' }],
    slot_info: { 1: { name: 'twiddly', game: GAME, type: 1 } },
    checked_locations: [],
    slot_data: {
      levels: [],
      options: {},
      ap_gems_per_level: engine.gemsPerLevel,
    },
    ...over,
  };
}

/** Opens a session and walks it as far as being in a room. */
async function joinARoom({ cache = new MemoryCache(), feed = [], over = {} } = {}) {
  const engine = await loadEngine('unused', 20260920);
  let socket = null;
  const client = new ArchipelagoClient(engine, {
    cache,
    open: (url) => {
      socket = new StubSocket(url);
      return socket;
    },
    onFeed: (said) => feed.push(said),
  });
  await client.connect({ host: 'localhost', port: 38281, slot: 'twiddly' });
  socket.deliver(roomInfo());
  await settle();
  socket.deliver(connected(engine, over));
  await settle();
  return { engine, client, socket };
}

// ---- the address a player types ------------------------------------------

assert.deepEqual(addressesFor('localhost', 38281), [
  'wss://localhost:38281',
  'ws://localhost:38281',
]);
assert.deepEqual(
  addressesFor('ws://localhost', 38281),
  ['ws://localhost:38281'],
  'a typed scheme is taken at its word rather than tried both ways',
);
// The shape a room actually gets shared in, pasted whole into the first box
// while the port box still holds its default.
assert.deepEqual(
  addressesFor('archipelago.gg:45678', '38281'),
  ['wss://archipelago.gg:45678', 'ws://archipelago.gg:45678'],
  'a port typed into the address lost to the one in the other box',
);
assert.deepEqual(addressesFor('archipelago.gg/', ''), [
  'wss://archipelago.gg',
  'ws://archipelago.gg',
]);

// ---- the handshake --------------------------------------------------------

{
  const { client, socket, engine } = await joinARoom();
  assert.equal(client.state, State.PLAYING, 'the client never got into the room');

  const hello = socket.lastOfKind('Connect');
  assert.equal(hello.game, GAME);
  assert.equal(hello.name, 'twiddly');
  assert.equal(hello.items_handling, 0b111, 'we must be sent our own items too');
  assert.equal(hello.slot_data, true, 'without slot data the run cannot be set up');

  // The placement is the server's now, so clearing a level must pay nothing.
  assert.equal(engine.wasm.tg_goal_met(engine.handle), 0);
  assert.ok(socket.ofKind('Get').length === 1, 'the spent counts were never asked for');
  assert.ok(socket.ofKind('SetNotify').length === 1, 'another window spending one would go unnoticed');
}

// ---- the datapackage ------------------------------------------------------

{
  // Nothing cached: the client has to ask, and has to wait for the answer
  // before it authenticates, or it would have no names to show.
  const cache = new MemoryCache();
  const engine = await loadEngine('unused', 7);
  let socket = null;
  const client = new ArchipelagoClient(engine, { cache, open: (url) => (socket = new StubSocket(url)) });
  await client.connect({ host: 'localhost', port: 38281, slot: 'twiddly' });
  socket.deliver(roomInfo({ [GAME]: 'abc', Other: 'def' }));
  await settle();

  const asked = socket.lastOfKind('GetDataPackage');
  assert.deepEqual(asked.games.sort(), [GAME, 'Other'].sort(), 'it asked for the wrong games');
  assert.equal(socket.ofKind('Connect').length, 0, 'it authenticated before it had the names');

  socket.deliver({
    cmd: 'DataPackage',
    data: {
      games: {
        [GAME]: { checksum: 'abc', item_name_to_id: { Rocket: 10 }, location_name_to_id: {} },
        Other: { checksum: 'def', item_name_to_id: {}, location_name_to_id: { Chest: 20 } },
      },
    },
  });
  await settle();
  assert.equal(socket.ofKind('Connect').length, 1, 'it never authenticated');
  assert.equal(await cache.read(GAME).then((it) => it.checksum), 'abc', 'nothing was cached');

  // And now that it is cached, a fresh client with the same checksums asks for
  // nothing at all and goes straight to saying hello.
  const second = await loadEngine('unused', 7);
  let reused = null;
  const again = new ArchipelagoClient(second, { cache, open: (url) => (reused = new StubSocket(url)) });
  await again.connect({ host: 'localhost', port: 38281, slot: 'twiddly' });
  reused.deliver(roomInfo({ [GAME]: 'abc', Other: 'def' }));
  await settle();
  assert.equal(reused.ofKind('GetDataPackage').length, 0, 'it re-fetched a datapackage it had');
  assert.equal(reused.ofKind('Connect').length, 1);

  // A checksum that has moved is the one case that must be fetched again.
  const third = await loadEngine('unused', 7);
  let moved = null;
  const changed = new ArchipelagoClient(third, { cache, open: (url) => (moved = new StubSocket(url)) });
  await changed.connect({ host: 'localhost', port: 38281, slot: 'twiddly' });
  moved.deliver(roomInfo({ [GAME]: 'abc', Other: 'CHANGED' }));
  await settle();
  assert.equal(moved.ofKind('GetDataPackage').length, 1, 'a game whose checksum moved was not refetched');
  assert.deepEqual(moved.lastOfKind('GetDataPackage').games, ['Other'], 'the game that had not changed was refetched too');
}

// ---- items, and the reconnection that must not double them ----------------

{
  const { engine, client, socket } = await joinARoom();
  const sent = (id, index) => ({ cmd: 'ReceivedItems', index, items: [{ item: id, location: 1, player: 1, flags: 0 }] });

  // The server's opening list: two level unlocks and three rockets.
  socket.deliver({
    cmd: 'ReceivedItems',
    index: 0,
    items: [
      { item: ITEM.LEVEL_UNLOCK, location: 1, player: 1, flags: 1 },
      { item: ITEM.LEVEL_UNLOCK, location: 2, player: 1, flags: 1 },
      { item: ITEM.ROCKET, location: 3, player: 1, flags: 2 },
      { item: ITEM.ROCKET, location: 4, player: 1, flags: 2 },
      { item: ITEM.ROCKET, location: 5, player: 1, flags: 2 },
    ],
  });
  socket.deliver({ cmd: 'Retrieved', keys: { [`${GAME}_spent_${Consumable.ROCKET}_0_1`]: 0 } });
  await settle();
  assert.equal(engine.unlocked, 3, 'the ladder did not open by item');
  assert.equal(engine.consumables(Consumable.ROCKET), 3);

  // One more arrives while playing, which needs no arithmetic at all.
  socket.deliver(sent(ITEM.ROCKET, 5));
  await settle();
  assert.equal(engine.consumables(Consumable.ROCKET), 4, 'an item that arrived mid-run went missing');

  // The player fires two. The server is told, rather than this being written
  // down here, so another browser would see it.
  client.spend(Consumable.ROCKET);
  client.spend(Consumable.ROCKET);
  const spend = socket.lastOfKind('Set');
  assert.equal(spend.key, `${GAME}_spent_${Consumable.ROCKET}_0_1`);
  assert.deepEqual(spend.operations, [{ operation: 'add', value: 1 }], 'a total would race another window');
  assert.equal(spend.default, 0);

  // Now the reconnection, which is the whole point. The server resends its
  // whole list from the beginning, and it still thinks it sent four rockets,
  // because it has no idea any were fired.
  socket.deliver({
    cmd: 'ReceivedItems',
    index: 0,
    items: [
      { item: ITEM.LEVEL_UNLOCK, location: 1, player: 1, flags: 1 },
      { item: ITEM.LEVEL_UNLOCK, location: 2, player: 1, flags: 1 },
      { item: ITEM.ROCKET, location: 3, player: 1, flags: 2 },
      { item: ITEM.ROCKET, location: 4, player: 1, flags: 2 },
      { item: ITEM.ROCKET, location: 5, player: 1, flags: 2 },
      { item: ITEM.ROCKET, location: 6, player: 1, flags: 2 },
    ],
  });
  socket.deliver({ cmd: 'Retrieved', keys: { [`${GAME}_spent_${Consumable.ROCKET}_0_1`]: 2 } });
  await settle();
  assert.equal(engine.unlocked, 3, 'the resent list opened the ladder twice over');
  assert.equal(
    engine.consumables(Consumable.ROCKET),
    2,
    'a reconnection handed back rockets the player had already fired',
  );
}

// ---- the feed comes from the server, so a reconnection is quiet -----------

{
  const feed = [];
  const { socket } = await joinARoom({ feed });
  socket.deliver({
    cmd: 'ReceivedItems',
    index: 0,
    items: [{ item: ITEM.RAINBOW_UNLOCK, location: 1, player: 1, flags: 1 }],
  });
  await settle();
  assert.equal(feed.length, 0, 'the items themselves wrote to the feed, so a resync would replay it');

  socket.deliver({
    cmd: 'PrintJSON',
    type: 'ItemSend',
    receiving: 1,
    item: { item: ITEM.RAINBOW_UNLOCK, location: 1, player: 1, flags: 1 },
    data: [
      { type: 'player_id', text: '1' },
      { text: ' found their ' },
      { type: 'item_id', text: String(ITEM.RAINBOW_UNLOCK), player: 1, flags: 1 },
    ],
  });
  await settle();
  assert.equal(feed.length, 1, 'the server said something and nothing reached the feed');
  assert.equal(feed[0].type, 'ItemSend');
  assert.equal(feed[0].mine, true, 'an item for us was not marked as ours');
  assert.equal(feed[0].parts[0].text, 'twiddly', 'a player id was not resolved to a name');
  assert.equal(feed[0].parts[0].you, true);
  assert.equal(feed[0].parts[2].flags, 1, 'what the item is worth was dropped');
}

// ---- checks go out once, not every frame ----------------------------------

{
  const { engine, client, socket } = await joinARoom();
  // Nothing checked yet, so polling a hundred frames must say nothing at all.
  for (let i = 0; i < 100; i += 1) {
    client.poll();
  }
  assert.equal(socket.ofKind('LocationChecks').length, 0, 'an idle frame sent a packet');

  // Play until something gets checked, then poll hard. A location checks on
  // every frame a board sits won, so this is exactly the case that goes wrong.
  const hint = engine.hint();
  engine.swap(...hint);
  for (let i = 0; i < 600; i += 1) {
    engine.update(16);
    client.poll();
  }
  const packets = socket.ofKind('LocationChecks');
  assert.ok(engine.checkedCount > 0, 'nothing got checked, so this proves nothing');
  const ids = packets.flatMap((packet) => packet.locations);
  assert.equal(new Set(ids).size, ids.length, 'the same location was sent more than once');
  assert.equal(ids.length, engine.checkedCount, 'what was sent is not what was checked');
  assert.ok(ids.every((id) => id >= AP), 'a location went out without the multiworld offset');
}

// ---- a seed from another version is refused, loudly -----------------------

{
  const engine = await loadEngine('unused', 7);
  let socket = null;
  const states = [];
  const client = new ArchipelagoClient(engine, {
    cache: new MemoryCache(),
    open: (url) => (socket = new StubSocket(url)),
    onState: (state, detail) => states.push([state, detail]),
  });
  await client.connect({ host: 'localhost', port: 38281, slot: 'twiddly' });
  socket.deliver(roomInfo());
  await settle();
  socket.deliver(connected(engine, { slot_data: { levels: [], options: {}, ap_gems_per_level: 99 } }));
  await settle();
  assert.equal(client.state, State.REFUSED, 'a seed the game cannot play was played anyway');
  assert.match(states.at(-1)[1], /different versions/, 'the refusal did not say why');
}

// ---- a refusal is not retried ---------------------------------------------

{
  const engine = await loadEngine('unused', 7);
  let socket = null;
  const client = new ArchipelagoClient(engine, {
    cache: new MemoryCache(),
    open: (url) => (socket = new StubSocket(url)),
  });
  await client.connect({ host: 'localhost', port: 38281, slot: 'twiddly' });
  socket.deliver(roomInfo());
  await settle();
  socket.deliver({ cmd: 'ConnectionRefused', errors: ['InvalidPassword'] });
  await settle();
  assert.equal(client.state, State.REFUSED);
  assert.match(client.error, /InvalidPassword/);
  assert.equal(client.retryAt, null, 'a wrong password would be retried forever');
}

console.log('ap ok: handshake, datapackage cache, resync, spent items, checks sent once, version refused');
