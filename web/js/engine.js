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

export const Flag = { WALL: 1, CLEARING: 2, SELECTED: 4 };

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
};

export const ObjectiveKind = { SCORE: 0, COLOR: 1, JELLY: 2 };

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
    const packed = this.wasm.tg_hint(this.handle);
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
  get levelCount() { return this.wasm.tg_level_count(this.handle); }
  get unlocked() { return this.wasm.tg_unlocked(this.handle); }

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
}
