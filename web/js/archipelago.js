// The multiworld: everything between this game and an Archipelago server.
//
// The split here is the same one the rest of the front end keeps. The engine
// owns the rules and what the run holds; this file owns the wire and translates
// in both directions. It never touches the page: what it has to say comes out
// through the handlers it was given, so the screen can be built on top of it
// without it knowing anything about a screen.
//
// Two things about it are worth knowing before reading it.
//
// **Per-message compression is not here because it is not ours.** The protocol
// asks clients to support it and the server offers it, but in a browser the
// WebSocket implementation negotiates and inflates below the JS API. There is
// no call to make and nothing to switch on: `event.data` is the plain JSON
// either way.
//
// **Almost nothing is remembered between sessions.** What the run holds, where
// it has been and what it was set up as all come from the server every time,
// which is why a player can open the game in another browser and carry on. The
// two exceptions are the datapackage, which is a cache and is checksummed, and
// how many of the things the run can spend it has actually spent, which lives
// in the server's own data store for the same reason everything else does. See
// `reconcileSpending`.

import { Consumable } from './engine.js';

/** What the server knows this game as. Must match the apworld's own name. */
export const GAME = 'Twiddly Gems';

/**
 * The things a run can be given to spend, by the engine's own code for each.
 *
 * By code rather than by name for the same reason the solo save writes them
 * that way: the code is the identity, appending a fifth kind is safe and
 * reordering them is not. It ends up in a key in the room's data store, so it
 * is a promise in the same way a location id in a save is.
 */
const SPENDABLE = Object.values(Consumable);

/**
 * The protocol this client speaks, which is not the same as a version of this
 * game. The server refuses anything older than it supports.
 */
export const PROTOCOL_VERSION = { major: 0, minor: 6, build: 7 };

/**
 * Send us items from other worlds, from our own world, and our starting
 * inventory. All three, because the engine no longer pays out its own
 * locations under a multiworld: if the server did not send our own items back
 * we would check a location holding one of our unlocks and never get it.
 */
export const ITEMS_HANDLING = 0b111;

/** What the server is told about how the run is going. */
export const ClientStatus = { CONNECTED: 5, READY: 10, PLAYING: 20, GOAL: 30 };

/** How much the world cares about an item, as the wire spells it. */
export const ItemFlag = { PROGRESSION: 1, USEFUL: 2, TRAP: 4 };

/**
 * Where this client can be in its conversation with a server.
 *
 * `handshaking` covers everything between the socket opening and the server
 * saying we are in, which is a real state rather than a moment: the datapackage
 * may have to be fetched in the middle of it.
 */
export const State = {
  OFFLINE: 'offline',
  CONNECTING: 'connecting',
  HANDSHAKING: 'handshaking',
  PLAYING: 'playing',
  REFUSED: 'refused',
  LOST: 'lost',
};

/** How long to wait before trying again, and the ceiling it climbs to. */
const FIRST_RETRY_MS = 1_000;
const LONGEST_RETRY_MS = 30_000;

const DB_NAME = 'twiddlygems.ap';
const DB_STORE = 'datapackage';
const UUID_KEY = 'twiddlygems.ap.uuid.v1';

/**
 * One kind of thing the run can spend, as a key in the server's data store.
 *
 * Per team and slot, because a data store is shared by the whole room and two
 * players of this game in one multiworld must not share a count. Named after
 * the game rather than given a bare word, by the same convention every other
 * game follows: the store has no namespaces and a key called `spent` would be
 * a collision waiting for the second game that wanted one.
 */
function spentKey(team, slot, kind) {
  return `${GAME}_spent_${kind}_${team}_${slot}`;
}

/**
 * Remembers each game's half of the datapackage between visits.
 *
 * Worth caching because it is the one big thing a server sends: every item and
 * location name of every game in the room, which for a large multiworld is
 * megabytes, and it does not change. The server hands out a checksum per game
 * in `RoomInfo` before anything is asked for, so what has to be fetched is
 * exactly the games whose checksum we do not already hold.
 *
 * Backed by IndexedDB, because this is too big for localStorage and the whole
 * point is not to fetch it again. Every path falls back to keeping it in memory
 * for this visit only: a browser in private mode, or one that has had its site
 * data blocked, should cost a player a slower connect rather than the game.
 */
export class DataPackageCache {
  constructor() {
    this.memory = new Map();
    this.db = null;
  }

  /** Opens the database, or decides once and for all to do without one. */
  async ready() {
    if (this.db !== null || this.db === false) {
      return this.db;
    }
    if (typeof indexedDB === 'undefined') {
      this.db = false;
      return false;
    }
    try {
      this.db = await new Promise((resolve, reject) => {
        const request = indexedDB.open(DB_NAME, 1);
        request.onupgradeneeded = () => request.result.createObjectStore(DB_STORE);
        request.onsuccess = () => resolve(request.result);
        request.onerror = () => reject(request.error);
        request.onblocked = () => reject(new Error('the datapackage store is blocked'));
      });
    } catch (error) {
      console.warn('no datapackage cache this visit', error);
      this.db = false;
    }
    return this.db;
  }

  async read(game) {
    const db = await this.ready();
    if (!db) {
      return this.memory.get(game) ?? null;
    }
    try {
      return await new Promise((resolve, reject) => {
        const request = db.transaction(DB_STORE, 'readonly').objectStore(DB_STORE).get(game);
        request.onsuccess = () => resolve(request.result ?? null);
        request.onerror = () => reject(request.error);
      });
    } catch (error) {
      console.warn(`could not read the cached datapackage for ${game}`, error);
      return this.memory.get(game) ?? null;
    }
  }

  async write(game, entry) {
    this.memory.set(game, entry);
    const db = await this.ready();
    if (!db) {
      return;
    }
    try {
      await new Promise((resolve, reject) => {
        const store = db.transaction(DB_STORE, 'readwrite').objectStore(DB_STORE);
        const request = store.put(entry, game);
        request.onsuccess = () => resolve();
        request.onerror = () => reject(request.error);
      });
    } catch (error) {
      // A cache that cannot be written is a slower connect next time and
      // nothing worse, so this is a warning rather than a failure.
      console.warn(`could not cache the datapackage for ${game}`, error);
    }
  }
}

/** The same shape with no browser behind it, for tests and for node. */
export class MemoryCache {
  constructor() {
    this.memory = new Map();
  }

  async read(game) {
    return this.memory.get(game) ?? null;
  }

  async write(game, entry) {
    this.memory.set(game, entry);
  }
}

/** Something to identify this browser by, kept so a reconnect looks the same. */
function uuid() {
  try {
    const saved = window.localStorage.getItem(UUID_KEY);
    if (saved) {
      return saved;
    }
    const made = crypto.randomUUID();
    window.localStorage.setItem(UUID_KEY, made);
    return made;
  } catch (error) {
    // Not worth a failed connection: the server only uses this to tell one
    // client from another, and a fresh one every visit still does that.
    return `twiddlygems-${Math.random().toString(36).slice(2)}`;
  }
}

/**
 * Turns `host:port` typed by a player into the URLs worth trying.
 *
 * Secure first, because a room hosted on the official site is always secure
 * and a browser on an https page may not open a plain socket at all. A room
 * someone is running on their own machine is usually the other one, so both get
 * tried before giving up. A player who types a scheme themselves is taken at
 * their word.
 */
export function addressesFor(host, port) {
  const typed = String(host).trim().replace(/\/+$/, '');
  const scheme = /^(wss?):\/\//i.exec(typed);
  const bare = scheme ? typed.slice(scheme[0].length) : typed;
  // A port in the address itself wins over the box beside it. "host:38281" is
  // how a room gets shared and pasted, and taking the other box as well would
  // turn that into a host on some other port, which fails in a way that reads
  // as the room being down.
  const where = /:\d+$/.test(bare) || !port ? bare : `${bare}:${port}`;
  if (scheme) {
    return [`${scheme[1].toLowerCase()}://${where}`];
  }
  return [`wss://${where}`, `ws://${where}`];
}

export class ArchipelagoClient {
  /**
   * @param engine the wasm session this connection plays into
   * @param handlers.open how to make a socket, so this can be driven in a test
   * @param handlers.cache where the datapackage lives between visits
   * @param handlers.onFeed called with a formatted message the server sent
   * @param handlers.onState called when the connection's state changes
   */
  constructor(engine, handlers = {}) {
    this.engine = engine;
    this.open = handlers.open ?? ((url) => new WebSocket(url));
    this.cache = handlers.cache ?? new DataPackageCache();
    this.onFeed = handlers.onFeed ?? (() => {});
    this.onState = handlers.onState ?? (() => {});
    this.base = engine.apIdBase;
    this.state = State.OFFLINE;
    this.socket = null;
    this.retryAt = null;
    this.forget();
  }

  /** Everything about one connection, thrown away when it ends. */
  forget() {
    this.room = null;
    this.slot = null;
    this.team = 0;
    this.players = new Map();
    this.slots = new Map();
    this.names = new Map();
    /// How many of each kind of spendable item the server has sent us, and how
    /// many the run has spent. See `reconcileSpending`.
    this.received = new Map();
    this.spent = new Map();
    this.spentKnown = false;
    /// How far into the run's own list of checks we have told the server. See
    /// `poll`.
    this.sentUpTo = 0;
    this.toldThemWeWon = false;
    this.error = null;
  }

  // ---- the connection ----

  /**
   * Joins a room, trying each address the host resolves to in turn.
   *
   * Resolves once the socket is open and the handshake has been started, not
   * once the server has let us in: being refused is something that happens
   * later and comes out through `onState`, because a wrong password is not a
   * different kind of event from being disconnected an hour in.
   */
  async connect({ host, port, slot, password = '' }) {
    this.disconnect();
    this.forget();
    this.room = { host, port, slot, password };
    this.setState(State.CONNECTING);

    let lastError = null;
    for (const url of addressesFor(host, port)) {
      try {
        this.socket = await this.raise(url);
        this.listen();
        this.setState(State.HANDSHAKING);
        return;
      } catch (error) {
        lastError = error;
      }
    }
    this.error = `could not reach ${host}${port ? `:${port}` : ''}`;
    this.setState(State.OFFLINE, lastError);
    throw new Error(this.error);
  }

  /** One attempt at one address, settled by whichever comes first. */
  raise(url) {
    return new Promise((resolve, reject) => {
      let socket;
      try {
        socket = this.open(url);
      } catch (error) {
        reject(error);
        return;
      }
      const opened = () => {
        socket.removeEventListener('error', failed);
        socket.removeEventListener('close', failed);
        resolve(socket);
      };
      const failed = () => {
        socket.removeEventListener('open', opened);
        // Closing a socket that never opened is harmless and stops a slow
        // secure attempt from still being in flight while the plain one is
        // being tried.
        try {
          socket.close();
        } catch {
          // Already gone, which is the outcome being asked for.
        }
        reject(new Error(`could not open ${url}`));
      };
      socket.addEventListener('open', opened, { once: true });
      socket.addEventListener('error', failed, { once: true });
      socket.addEventListener('close', failed, { once: true });
    });
  }

  listen() {
    this.socket.addEventListener('message', (event) => {
      let packets;
      try {
        packets = JSON.parse(event.data);
      } catch (error) {
        console.warn('the server sent something that is not JSON', error);
        return;
      }
      // One frame carries any number of commands, in order.
      for (const packet of packets) {
        try {
          this.handle(packet);
        } catch (error) {
          // One bad command must not take the connection down with it: the
          // rest of the frame is very likely fine, and the alternative is a
          // player dropped out of their room by a message they never saw.
          console.error(`could not handle ${packet?.cmd}`, error);
        }
      }
    });
    this.socket.addEventListener('close', () => this.dropped());
  }

  /** Says goodbye on purpose, which is the one close that does not retry. */
  disconnect() {
    this.cancelRetry();
    const socket = this.socket;
    this.socket = null;
    if (socket) {
      socket.close();
    }
    if (this.state !== State.OFFLINE) {
      this.setState(State.OFFLINE);
    }
  }

  /**
   * The socket went away without being asked to.
   *
   * Nothing about the run is thrown away here. The engine keeps playing, and
   * what it checks while the connection is down is sent when it comes back:
   * every reconnection begins by sending the whole set, which is what that is
   * for. See `syncChecks`.
   */
  dropped() {
    if (!this.socket) {
      return;
    }
    this.socket = null;
    this.setState(State.LOST);
    this.retryLater();
  }

  retryLater() {
    const wait = Math.min(this.retryIn ?? FIRST_RETRY_MS, LONGEST_RETRY_MS);
    this.retryIn = Math.min(wait * 2, LONGEST_RETRY_MS);
    this.retryAt = setTimeout(() => {
      this.retryAt = null;
      if (this.room) {
        this.connect(this.room).catch(() => this.retryLater());
      }
    }, wait);
  }

  cancelRetry() {
    if (this.retryAt !== null) {
      clearTimeout(this.retryAt);
      this.retryAt = null;
    }
    this.retryIn = FIRST_RETRY_MS;
    this.room = null;
  }

  setState(state, detail = null) {
    this.state = state;
    this.onState(state, detail ?? this.error);
  }

  send(...packets) {
    if (!this.socket || this.socket.readyState !== 1) {
      return false;
    }
    this.socket.send(JSON.stringify(packets));
    return true;
  }

  // ---- the conversation ----

  handle(packet) {
    const handler = this[`on${packet.cmd}`];
    if (typeof handler === 'function') {
      handler.call(this, packet);
    }
  }

  /**
   * The server's hello, which arrives before we have said anything.
   *
   * It carries a checksum per game, so what has to be fetched is settled here
   * rather than by asking for everything and throwing most of it away.
   */
  onRoomInfo(packet) {
    this.seed = packet.seed_name;
    const checksums = packet.datapackage_checksums ?? {};
    const stale = [];
    const load = Object.keys(checksums).map(async (game) => {
      const held = await this.cache.read(game);
      if (held && held.checksum === checksums[game]) {
        this.remember(game, held);
        return;
      }
      stale.push(game);
    });

    Promise.all(load).then(() => {
      if (stale.length > 0) {
        this.awaiting = new Set(stale);
        this.send({ cmd: 'GetDataPackage', games: stale });
        return;
      }
      this.authenticate();
    });
  }

  onDataPackage(packet) {
    const games = packet.data?.games ?? {};
    for (const [game, data] of Object.entries(games)) {
      const entry = {
        checksum: data.checksum,
        items: data.item_name_to_id ?? {},
        locations: data.location_name_to_id ?? {},
      };
      this.remember(game, entry);
      this.cache.write(game, entry);
    }
    this.awaiting = null;
    this.authenticate();
  }

  /**
   * Keeps one game's names the way they get used: by id.
   *
   * The wire sends a name to id mapping, because that is what a world builds.
   * Everything a player reads goes the other way, so it is turned over once
   * here rather than searched every time a message arrives.
   */
  remember(game, entry) {
    this.names.set(game, {
      items: flip(entry.items),
      locations: flip(entry.locations),
    });
  }

  authenticate() {
    this.send({
      cmd: 'Connect',
      game: GAME,
      name: this.room.slot,
      password: this.room.password ?? '',
      uuid: uuid(),
      version: { ...PROTOCOL_VERSION, class: 'Version' },
      items_handling: ITEMS_HANDLING,
      tags: [],
      slot_data: true,
    });
  }

  onConnectionRefused(packet) {
    const said = (packet.errors ?? []).join(', ');
    this.error = said ? `the server refused the connection: ${said}` : 'the server refused the connection';
    // A refusal is not something to retry: the password is wrong, or the slot
    // name is, and trying the same thing every second will not fix either.
    this.cancelRetry();
    this.setState(State.REFUSED);
  }

  /**
   * We are in. This is where the run is put into the shape of this seed.
   *
   * Order matters all the way down and every step says why.
   */
  onConnected(packet) {
    this.slot = packet.slot;
    this.team = packet.team ?? 0;
    for (const player of packet.players ?? []) {
      this.players.set(`${player.team}/${player.slot}`, player.alias || player.name);
    }
    for (const [at, info] of Object.entries(packet.slot_info ?? {})) {
      this.slots.set(Number(at), info);
    }

    // First, because everything after it depends on the run being the right
    // run, and because changing a setting deals the whole run again.
    const settings = packet.slot_data?.options ?? {};
    this.engine.setRemote(true);
    for (const option of this.engine.options) {
      const wanted = settings[option.key];
      if (Number.isInteger(wanted)) {
        this.engine.setOption(option.index, wanted);
      }
    }

    // Then the one number both sides work out for themselves, held up against
    // each other. If these disagree the board spawns gems for locations this
    // seed does not have and the checks behind them go nowhere, which is a
    // failure nobody would notice until a seed would not finish.
    const wanted = packet.slot_data?.ap_gems_per_level;
    if (Number.isInteger(wanted) && wanted !== this.engine.gemsPerLevel) {
      this.error =
        `this seed puts ${wanted} AP gems in a level and the game makes ${this.engine.gemsPerLevel}; ` +
        'the game and the world it was generated with are different versions';
      this.cancelRetry();
      this.disconnect();
      this.setState(State.REFUSED);
      return;
    }

    // What the room already knows we have checked, including anything a
    // co-op partner in this same slot found. Quiet, and under a multiworld it
    // pays out nothing: it is the run's record of where it has been, which is
    // what the level picker reads.
    this.restoreChecks(packet.checked_locations ?? []);

    // How many of the things this run can spend it has already spent. The
    // server sends what it gave us, forever, and has no idea we fired any of
    // them; without this a reload hands the player back everything they spent.
    this.askWhatWasSpent();

    this.setState(State.PLAYING);
    // Last: everything this run has checked, in one go. The one full send per
    // connection, which covers whatever was checked while the socket was down.
    this.syncChecks();
    this.send({ cmd: 'StatusUpdate', status: ClientStatus.PLAYING });
  }

  restoreChecks(ids) {
    for (const id of ids) {
      this.engine.restore(id - this.base);
    }
  }

  /**
   * The items this slot has been given.
   *
   * An index of zero means this is the whole inventory rather than an addition
   * to it, which is what every reconnection looks like. So the run forgets what
   * it was holding and is told again: everything but the unlocks stacks, and
   * adding a resent list to what was already there would double all of it.
   *
   * Nothing is written to the feed from here on purpose. What the player reads
   * is built from the server's own `PrintJSON`, which is only ever sent live,
   * so a reconnection quietly puts fifty items back rather than announcing
   * them all over again.
   */
  onReceivedItems(packet) {
    const whole = packet.index === 0;
    if (whole) {
      this.engine.forgetItems();
      this.received.clear();
    }
    // Counted by watching what the engine holds rather than by reading the
    // ids, because the engine is the one that knows which numbers are
    // spendable things and it already worked it out. On the whole-list path
    // this starts from nothing, so the difference below is the total.
    const before = this.heldNow();
    for (const item of packet.items ?? []) {
      this.engine.receive(item.item - this.base);
    }
    const after = this.heldNow();
    for (const kind of SPENDABLE) {
      const arrived = after.get(kind) - before.get(kind);
      this.received.set(kind, (this.received.get(kind) ?? 0) + arrived);
    }
    this.reconcileSpending();
  }

  /** What the run is carrying right now, by kind. */
  heldNow() {
    return new Map(SPENDABLE.map((kind) => [kind, this.engine.consumables(kind)]));
  }

  askWhatWasSpent() {
    const keys = SPENDABLE.map((kind) => spentKey(this.team, this.slot, kind));
    // Watched as well as asked for, so a second window on this slot spending
    // one is noticed here rather than handing the player two of it.
    this.send({ cmd: 'Get', keys }, { cmd: 'SetNotify', keys });
  }

  onRetrieved(packet) {
    for (const [key, value] of Object.entries(packet.keys ?? {})) {
      const kind = this.kindOfKey(key);
      if (kind !== null) {
        this.spent.set(kind, Number(value) || 0);
      }
    }
    this.spentKnown = true;
    this.reconcileSpending();
  }

  /** A second window on this slot spent one; keep up rather than argue. */
  onSetReply(packet) {
    const kind = this.kindOfKey(packet.key);
    if (kind === null) {
      return;
    }
    this.spent.set(kind, Number(packet.value) || 0);
    this.reconcileSpending();
  }

  kindOfKey(key) {
    for (const kind of SPENDABLE) {
      if (key === spentKey(this.team, this.slot, kind)) {
        return kind;
      }
    }
    return null;
  }

  /**
   * Works out what the run still has to spend, and tells the engine.
   *
   * The server knows what it sent and nothing about what became of it, so what
   * is left is a subtraction rather than a fact either side holds. Both halves
   * arrive on their own schedule, so this runs whenever either one lands and
   * does nothing until both have.
   *
   * It sets rather than adds, which is what `restoreConsumables` is for: this
   * is the one holding a run has that goes down.
   */
  reconcileSpending() {
    if (!this.spentKnown) {
      return;
    }
    for (const kind of SPENDABLE) {
      const sent = this.received.get(kind) ?? 0;
      const gone = this.spent.get(kind) ?? 0;
      this.engine.restoreConsumables(kind, Math.max(0, sent - gone));
    }
  }

  /**
   * The run spent one. Told to the server rather than written down here, so a
   * player picking the game up in another browser gets the run as they left
   * it.
   *
   * An atomic add rather than a new total, because two windows on one slot
   * would otherwise overwrite each other's arithmetic.
   */
  spend(kind) {
    this.spent.set(kind, (this.spent.get(kind) ?? 0) + 1);
    this.send({
      cmd: 'Set',
      key: spentKey(this.team, this.slot, kind),
      default: 0,
      want_reply: false,
      operations: [{ operation: 'add', value: 1 }],
    });
  }

  onRoomUpdate(packet) {
    if (packet.checked_locations) {
      this.restoreChecks(packet.checked_locations);
    }
    for (const player of packet.players ?? []) {
      this.players.set(`${player.team}/${player.slot}`, player.alias || player.name);
    }
  }

  /**
   * Anything the server wants shown, already assembled into parts.
   *
   * Handed on as spans rather than as a string, so the page can color an item
   * by what it is worth and a player by whether it is us. What the server sends
   * is live only: it is not replayed on a reconnection, which is exactly why
   * the feed is built from this rather than from the items themselves.
   */
  onPrintJSON(packet) {
    const parts = (packet.data ?? []).map((part) => this.readPart(part));
    this.onFeed({
      type: packet.type ?? 'Text',
      parts,
      text: parts.map((part) => part.text).join(''),
      // Whether this one is about us, which is what decides if it belongs in a
      // feed of what this run is doing or in the room's chatter.
      ours: packet.receiving === this.slot || packet.item?.player === this.slot,
      mine: packet.receiving === this.slot,
    });
  }

  /** One node of a message, with every id it carries resolved to a name. */
  readPart(part) {
    const kind = part.type ?? 'text';
    if (kind === 'player_id') {
      const at = Number(part.text);
      return { kind: 'player', text: this.playerName(at), you: at === this.slot };
    }
    if (kind === 'item_id') {
      return {
        kind: 'item',
        text: this.itemName(Number(part.text), part.player),
        flags: part.flags ?? 0,
      };
    }
    if (kind === 'location_id') {
      return { kind: 'location', text: this.locationName(Number(part.text), part.player) };
    }
    if (kind === 'color') {
      return { kind: 'color', text: part.text ?? '', color: part.color ?? '' };
    }
    return { kind: 'text', text: part.text ?? '' };
  }

  playerName(at) {
    return this.players.get(`${this.team}/${at}`) ?? this.slots.get(at)?.name ?? `Player ${at}`;
  }

  /**
   * What an item is called, in the game that defines it.
   *
   * Which game that is comes off the slot it belongs to, because names and
   * numbers are only unique inside one game: two worlds in a room may both
   * have an item numbered 40 and they are not the same item.
   */
  itemName(id, player) {
    return this.nameIn(player, 'items', id) ?? `Item ${id}`;
  }

  locationName(id, player) {
    return this.nameIn(player, 'locations', id) ?? `Location ${id}`;
  }

  nameIn(player, which, id) {
    const game = this.slots.get(player)?.game;
    return game ? this.names.get(game)?.[which]?.[id] : undefined;
  }

  // ---- what the run does, on its way out ----

  /**
   * Called every frame. Sends what is new and nothing else.
   *
   * The engine's list of checks only ever grows and never repeats, so this is
   * a length compared against how far we have got, and it allocates nothing at
   * all on the overwhelming majority of frames where nothing was checked.
   *
   * That is the whole point of it. Checking a location is idempotent in the
   * engine and has to be, because it runs on every frame a board sits won, so
   * the naive version of this sends the same list thirty times a second for as
   * long as a player looks at a finished level. The server drops the
   * duplicates, and it is still rude.
   */
  poll() {
    if (this.state !== State.PLAYING) {
      return;
    }
    if (this.engine.checkedCount > this.sentUpTo) {
      const fresh = this.engine.checked.slice(this.sentUpTo);
      this.sentUpTo = this.engine.checkedCount;
      this.send({ cmd: 'LocationChecks', locations: fresh.map((id) => id + this.base) });
    }
    if (!this.toldThemWeWon && this.engine.goalMet) {
      this.toldThemWeWon = true;
      this.send({ cmd: 'StatusUpdate', status: ClientStatus.GOAL });
    }
  }

  /**
   * Everything this run has checked, whether or not the server has heard it.
   *
   * Once per connection. A reconnection is the only time this is worth doing:
   * the socket may have been down while a level was cleared, and the server's
   * own list is the thing being caught up with. After this the count above
   * takes over and nothing is ever sent twice.
   */
  syncChecks() {
    this.sentUpTo = this.engine.checkedCount;
    const all = this.engine.checked.map((id) => id + this.base);
    if (all.length > 0) {
      this.send({ cmd: 'LocationChecks', locations: all });
    }
  }

  /** Says something in the room. */
  say(text) {
    const said = String(text).trim();
    if (said) {
      this.send({ cmd: 'Say', text: said });
    }
  }
}

/** Turns a name to id mapping into an id to name one. */
function flip(mapping) {
  const flipped = Object.create(null);
  for (const [name, id] of Object.entries(mapping)) {
    flipped[id] = name;
  }
  return flipped;
}
