// Thin wrapper over the wasm module's C ABI.
//
// The engine owns the rules and the clock; this file only translates. Per-frame
// board data is read as typed-array views straight into wasm memory rather than
// one call per cell, so a frame costs a handful of calls no matter the board.

const CELL_STRIDE = 4;
const OFFSET_STRIDE = 3;
const EVENT_SIZE = 8;

export const Phase = {
  IDLE: 0,
  SWAPPING: 1,
  CLEARING: 2,
  LAUNCHING: 3,
  FALLING: 4,
  SHUFFLING: 5,
  FINISHED: 6,
  /// The goal is met and the leftover moves are being spent, one per step.
  CASHING_IN: 7,
  /// A beat on a won board before the level is declared over.
  FINISHING: 8,
};

export const Status = { PLAYING: 0, WON: 1, LOST: 2 };

export const Special = {
  NONE: 0,
  LINE_H: 1,
  LINE_V: 2,
  CROSS: 3,
  RAINBOW: 4,
  ROCKET: 5,
};

export const Flag = {
  WALL: 1,
  CLEARING: 2,
  SELECTED: 4,
  BRICK: 8,
  CRACKED: 16,
  /// The blocker here is keyed to a color, which the cell's color byte holds.
  SEAL: 32,
};

export const EventKind = {
  CLEAR: 1,
  SPECIAL_MADE: 2,
  SPECIAL_FIRED: 3,
  SWAP: 4,
  REVERT: 5,
  CASCADE: 6,
  SHUFFLE: 7,
  WON: 8,
  LOST: 9,
  ROCKET_HIT: 10,
  MATCH: 11,
  LAND: 12,
  LOW_MOVES: 13,
  BRICK: 14,
  /// An item reached the run. `color` is its `ItemKind`, `value` its one
  /// parameter. Raised by the session, not the board.
  ITEM: 15,
  /// The level is won. `value` is how many moves were left over for the
  /// flourish to spend. Raised once, before any of it happens.
  CLEARED: 16,
  /// A leftover move was spent on this cell. `special` is what it left there,
  /// or `Special.NONE` when the run had nothing to place.
  CASH_IN: 17,
};

/// What sort of item an `EventKind.ITEM` is about. Its parameter is the
/// special's code for an unlock, and the level for moves.
export const ItemKind = { UNLOCK: 0, MOVES: 1 };

/// Where an `EventKind.ITEM` came from. Its parameter is the level for a
/// clear and the length for a chain. `NONE` is an item that came from no
/// location here at all, which is what a multiworld sending one looks like.
/// How well a level has been beaten, at best. Ordered, so the larger number is
/// always the better result.
export const Tier = { NONE: 0, CLEAR: 1, SILVER: 2, GOLD: 3 };

export const LocationKind = {
  LEVEL_CLEAR: 0,
  CHAIN: 1,
  LEVEL_SILVER: 2,
  LEVEL_GOLD: 3,
  NONE: 255,
};

export const ObjectiveKind = { SCORE: 0, COLOR: 1, JELLY: 2, BRICK: 3, SEAL: 4 };

export const EMPTY_CELL = 255;

/** Fetches and instantiates the wasm module, then opens a session on it. */
export async function loadEngine(url, seed) {
  const response = await fetch(url, { cache: 'no-cache' });
  if (!response.ok) {
    throw new Error(`could not fetch ${url}: ${response.status} ${response.statusText}`);
  }
  // Plain instantiate rather than instantiateStreaming: some static servers
  // hand .wasm back as octet-stream, which streaming refuses.
  const { instance } = await WebAssembly.instantiate(await response.arrayBuffer(), {});
  return new Engine(instance.exports, seed);
}

export class Engine {
  constructor(exports, seed) {
    this.wasm = exports;
    this.memory = exports.memory;
    this.handle = exports.tg_create(seed >>> 0, Math.floor(seed / 2 ** 32) >>> 0);
    if (!this.handle) {
      throw new Error('the engine refused to start a session');
    }
    this.decoder = new TextDecoder();
    this.readGeometry();
  }

  /** Board size can change with the level, so it is re-read on every load. */
  readGeometry() {
    this.rows = this.wasm.tg_rows(this.handle);
    this.cols = this.wasm.tg_cols(this.handle);
    this.colors = this.wasm.tg_colors(this.handle);
    this.cellCount = this.rows * this.cols;
  }

  // ---- the clock and input ----

  update(dtMs) {
    this.wasm.tg_update(this.handle, dtMs);
  }

  tap(r, c) {
    return this.wasm.tg_tap(this.handle, r, c);
  }

  swap(r1, c1, r2, c2) {
    return this.wasm.tg_swap(this.handle, r1, c1, r2, c2) === 1;
  }

  clearSelection() {
    this.wasm.tg_clear_selection(this.handle);
  }

  // ---- reading the board ----

  /**
   * Views onto this frame's board state. They are only valid until the next
   * call into the engine, so take them fresh each frame and do not keep them.
   */
  snapshot() {
    const cellsPtr = this.wasm.tg_cells_ptr(this.handle);
    const offsetsPtr = this.wasm.tg_offsets_ptr(this.handle);
    return {
      cells: new Uint8Array(this.memory.buffer, cellsPtr, this.cellCount * CELL_STRIDE),
      offsets: new Float32Array(this.memory.buffer, offsetsPtr, this.cellCount * OFFSET_STRIDE),
    };
  }

  /** Everything that happened during the last call into the engine. */
  drainEvents() {
    const count = this.wasm.tg_events_len(this.handle);
    if (count === 0) {
      return [];
    }
    const bytes = new Uint8Array(
      this.memory.buffer,
      this.wasm.tg_events_ptr(this.handle),
      count * EVENT_SIZE,
    );
    const events = [];
    for (let i = 0; i < count; i += 1) {
      const at = i * EVENT_SIZE;
      events.push({
        kind: bytes[at],
        r: bytes[at + 1],
        c: bytes[at + 2],
        color: bytes[at + 3],
        special: bytes[at + 4],
        cascade: bytes[at + 5],
        value: bytes[at + 6] | (bytes[at + 7] << 8),
      });
    }
    return events;
  }

  /** A legal move as `[r1, c1, r2, c2]`, or null when the board is stuck. */
  hint() {
    // `>>> 0` because wasm hands an i32 back signed: the engine's `u32::MAX`
    // arrives as -1, and comparing that to 0xffffffff is never true. Without
    // it a stuck board answers with the cell (255, 255), which is nowhere.
    const packed = this.wasm.tg_hint(this.handle) >>> 0;
    if (packed === 0xffffffff) {
      return null;
    }
    return [packed >>> 24, (packed >>> 16) & 0xff, (packed >>> 8) & 0xff, packed & 0xff];
  }

  // ---- state a HUD shows ----

  get score() { return this.wasm.tg_score(this.handle); }
  get movesLeft() { return this.wasm.tg_moves_left(this.handle); }
  get movesTotal() { return this.wasm.tg_moves_total(this.handle); }
  get phase() { return this.wasm.tg_phase(this.handle); }
  get status() { return this.wasm.tg_status(this.handle); }
  get cascade() { return this.wasm.tg_cascade(this.handle); }
  get acceptsInput() { return this.wasm.tg_accepts_input(this.handle) === 1; }
  get levelIndex() { return this.wasm.tg_level_index(this.handle); }

  /**
   * Which specials this run may make, as a set of `Special` codes.
   *
   * Empty on a new run: matching still clears, it just leaves nothing behind.
   * The unlocks arrive as items, in solo from clearing levels.
   */
  get unlockedSpecials() {
    const mask = this.wasm.tg_unlocked_specials(this.handle);
    return new Set(Object.values(Special).filter((code) => (mask >> code) & 1));
  }

  /**
   * Which locations this run has checked, as ids to write into a save.
   *
   * Opaque numbers on purpose: what each one is worth is the engine's business
   * and will be the multiworld's later. The page only has to hand them back.
   */
  get checked() {
    const count = this.wasm.tg_checked_len(this.handle);
    const ptr = this.wasm.tg_checked_ptr(this.handle);
    if (count === 0) {
      return [];
    }
    return [...new Uint32Array(this.memory.buffer, ptr, count)];
  }

  /** How well a level has been beaten, at best, as a `Tier`. */
  levelBest(index) {
    return this.wasm.tg_level_best(this.handle, index);
  }

  /** Hands one back on load, rebuilding what it gave without announcing it. */
  restore(id) {
    this.wasm.tg_restore(this.handle, id);
  }

  get levelCount() { return this.wasm.tg_level_count(this.handle); }
  get unlocked() { return this.wasm.tg_unlocked(this.handle); }

  /**
   * The scores worth coming back for on this level, or 0 for neither.
   *
   * Not goals: the level ends on its objectives whatever the score. Each is
   * somewhere an item is found, so passing one is worth more than the number.
   */
  get tiers() {
    return {
      silver: this.wasm.tg_level_silver(this.handle),
      gold: this.wasm.tg_level_gold(this.handle),
    };
  }

  get levelName() {
    const length = this.wasm.tg_level_name_len(this.handle);
    const bytes = new Uint8Array(this.memory.buffer, this.wasm.tg_level_name_ptr(this.handle), length);
    return this.decoder.decode(bytes);
  }

  /** Every level's name, for the level picker. */
  levelNames() {
    const length = this.wasm.tg_level_names_len(this.handle);
    if (length === 0) {
      return [];
    }
    const bytes = new Uint8Array(this.memory.buffer, this.wasm.tg_level_names_ptr(this.handle), length);
    return this.decoder.decode(bytes).split('\n');
  }

  objectives() {
    const count = this.wasm.tg_objective_count(this.handle);
    const list = [];
    for (let i = 0; i < count; i += 1) {
      list.push({
        kind: this.wasm.tg_objective_kind(this.handle, i),
        color: this.wasm.tg_objective_color(this.handle, i),
        need: this.wasm.tg_objective_need(this.handle, i),
        have: this.wasm.tg_objective_have(this.handle, i),
      });
    }
    return list;
  }

  // ---- level navigation ----

  loadLevel(index) {
    const ok = this.wasm.tg_load_level(this.handle, index) === 1;
    if (ok) {
      this.readGeometry();
    }
    return ok;
  }

  nextLevel() {
    const ok = this.wasm.tg_next_level(this.handle) === 1;
    if (ok) {
      this.readGeometry();
    }
    return ok;
  }

  retry() {
    this.wasm.tg_retry(this.handle);
    this.readGeometry();
  }

  setUnlocked(count) {
    this.wasm.tg_set_unlocked(this.handle, count);
  }

  /**
   * Throws the whole run away and opens a fresh session on the same module:
   * a new seed, back to the first level, everything else locked again.
   *
   * The new session is opened before the old one is dropped, so a refusal
   * leaves the run that is already going still playable.
   */
  restart(seed) {
    const next = this.wasm.tg_create(seed >>> 0, Math.floor(seed / 2 ** 32) >>> 0);
    if (!next) {
      throw new Error('the engine refused to start a session');
    }
    this.wasm.tg_destroy(this.handle);
    this.handle = next;
    this.readGeometry();
  }
}
