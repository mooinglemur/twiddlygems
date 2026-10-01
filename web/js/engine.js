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
  /// A beat on a won board before the flourish starts, so the goals can be
  /// seen reaching their totals rather than being talked over. How long is
  /// this file's business: see `tg_set_goal_hold`.
  TALLYING: 9,
};

export const Status = { PLAYING: 0, WON: 1, LOST: 2 };

export const Special = {
  NONE: 0,
  LINE_H: 1,
  LINE_V: 2,
  CROSS: 3,
  RAINBOW: 4,
  ROCKET: 5,
  /// An Archipelago gem: a check sitting on the board rather than a gem to
  /// match. Never something a match leaves behind, and never unlocked.
  ARCHIPELAGO: 6,
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
  /// An item reached the run. `value` is its place in `itemNames`, and
  /// `color` and `special` are the low and high bytes of the location's place
  /// in `locationNames`, or `NO_LOCATION`. Raised by the session, not the
  /// board.
  ITEM: 15,
  /// The level is won. `value` is how many moves were left over for the
  /// flourish to spend. Raised once, before any of it happens.
  CLEARED: 16,
  /// A leftover move was spent on this cell. `special` is what it left there,
  /// or `Special.NONE` when the run had nothing to place.
  CASH_IN: 17,
  /// An Archipelago gem was collected. Distinct from `CLEAR`, which says a gem
  /// left the board: one struck by a beam raises both.
  AP_CLEAR: 18,
};

/// The location an item event names when it came from no location here at
/// all, which is what a multiworld sending one over looks like.
export const NO_LOCATION = 65535;

/// How well a level has been beaten, at best. Ordered, so the larger number is
/// always the better result.
export const Tier = { NONE: 0, CLEAR: 1, SILVER: 2, GOLD: 3 };

export const ObjectiveKind = { SCORE: 0, COLOR: 1, JELLY: 2, BRICK: 3, SEAL: 4 };

/// How much the world cares about an item, which is what its name is colored
/// by in the feed. Archipelago's own four; see `Class` in the engine.
export const ItemClass = { FILLER: 0, USEFUL: 1, PROGRESSION: 2, TRAP: 3 };

/// Which sort of item it is, which is a different question from what it is
/// worth: a noise and a bonus item are both things the rules never ask for,
/// and only one of them makes a sound. See `Item::kind` in the engine.
export const ItemKind = {
  UNLOCK: 0,
  MOVES: 1,
  NOISE: 2,
  CONSUMABLE: 3,
  LEVEL_UNLOCK: 4,
};

/**
 * The things a run can be handed to spend, by the code the engine numbers them
 * with.
 *
 * Three of them are aimed: the player picks one and then picks a cell. The
 * cluster is not, because what a rocket aims at is the rocket's own business
 * and a handful at once is the point of it.
 */
export const Consumable = { ROCKET: 0, RAINBOW: 1, CROSS_CLEAR: 2, ROCKET_CLUSTER: 3 };

/** Which of them want a cell before they will go off. */
export const AIMED = new Set([Consumable.ROCKET, Consumable.RAINBOW, Consumable.CROSS_CLEAR]);

/// Three floats per rocket in `flights`: a column, a row, and whether it is
/// still in the air.
const FLIGHT_STRIDE = 3;

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

  /**
   * Rockets spent out of the inventory, as `{c, r, flying}` in board
   * coordinates: a row of `rows` is the one below the bottom, where the bar
   * they come out of is.
   *
   * A list of their own because no cell holds one. Every other gem in the air
   * is a gem in a cell riding that cell's offset, and one of these sets off
   * from under the board, where there is no cell to ride.
   *
   * Landed ones stay in the list, at the place they have always been. Where a
   * rocket sits is how the renderer tells it from the next one across frames,
   * so a rocket dropping out as it lands would hand every rocket behind it
   * somebody else's heading.
   */
  flights() {
    const count = this.wasm.tg_flights_len(this.handle);
    if (count === 0) {
      return [];
    }
    const values = new Float32Array(
      this.memory.buffer,
      this.wasm.tg_flights_ptr(this.handle),
      count * FLIGHT_STRIDE,
    );
    const list = [];
    for (let i = 0; i < count; i += 1) {
      const at = i * FLIGHT_STRIDE;
      list.push({ c: values[at], r: values[at + 1], flying: values[at + 2] === 1 });
    }
    return list;
  }

  /** How many of one thing the run is carrying. */
  consumables(kind) {
    return this.wasm.tg_consumables(this.handle, kind);
  }

  /**
   * Spends one on a cell, answering whether it happened.
   *
   * `null` for the cluster, which aims itself. A refusal costs nothing: the
   * engine asks the board first and only takes the item out if the board took
   * it, so pointing an aimed one somewhere it can do nothing leaves the run
   * still carrying it.
   */
  useConsumable(kind, cell = null) {
    const { r, c } = cell ?? { r: -1, c: -1 };
    return this.wasm.tg_use_consumable(this.handle, kind, r, c) === 1;
  }

  /** Hands a count back on load. Sets rather than raises; see the engine. */
  restoreConsumables(kind, held) {
    this.wasm.tg_restore_consumables(this.handle, kind, held);
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

  /**
   * What the world makes of the item at `index` in `itemNames`, as an
   * `ItemClass`. What the feed colors a name by.
   */
  itemClass(index) {
    return this.wasm.tg_item_class(this.handle, index);
  }

  /**
   * Which sort of item sits at `index`, as an `ItemKind`, and its one
   * parameter alongside it.
   *
   * The pair that lets the page do something with an item besides print its
   * name. A noise is what needs it: the sound to play is `NOISES[value]`, and
   * matching on the name instead would put a second copy of sixteen strings
   * in the sound bank for the engine's list to drift away from.
   *
   * An index off the end is an unlock of nothing, so a caller that asks about
   * an item this build does not have gets an answer that sets nothing off.
   */
  itemKind(index) {
    return this.wasm.tg_item_kind(this.handle, index);
  }

  itemValue(index) {
    return this.wasm.tg_item_value(this.handle, index);
  }

  /**
   * Where the item numbered `id` sits in `itemNames`, or a number off the end
   * if this build has no item with that number.
   *
   * For the multiworld side. A find of this run's own arrives as an event
   * carrying an index already; what the server says about an item coming from
   * somewhere else carries its number, and the page wants to ask the same
   * questions about both.
   */
  itemAtId(id) {
    return this.wasm.tg_item_at_id(this.handle, id);
  }

  /**
   * What a level still has waiting in it, for the tracker: how many of its
   * Archipelago gems have been taken, and how many of its moves upgrades have
   * turned up. Each is a pair, because a mark that fills partway has to know
   * what it is a fraction of.
   */
  levelGems(index) {
    return {
      found: this.wasm.tg_level_gems(this.handle, index),
      total: this.wasm.tg_gems_per_level(this.handle),
    };
  }

  levelMoves(index) {
    return {
      found: this.wasm.tg_level_moves(this.handle, index),
      total: this.wasm.tg_level_moves_total(this.handle, index),
    };
  }

  /** The best score this run has beaten a level with, or 0 if it never has. */
  levelBestScore(index) {
    return this.wasm.tg_level_best_score(this.handle, index);
  }

  /** Every level's best score, for the save. */
  get bestScores() {
    const scores = [];
    for (let i = 0; i < this.levelCount; i += 1) {
      scores.push(this.levelBestScore(i));
    }
    return scores;
  }

  /** Hands one back on load. Quiet, and never lowers what the run has done. */
  restoreBestScore(index, score) {
    this.wasm.tg_restore_best_score(this.handle, index, score);
  }

  /** Hands one back on load, rebuilding what it gave without announcing it. */
  restore(id) {
    this.wasm.tg_restore(this.handle, id);
  }

  /**
   * How many locations the run has checked.
   *
   * Cheap, unlike `checked`, which builds an array every time it is asked. A
   * client watching for new checks asks this every frame and only reads the
   * list when the number has moved.
   */
  get checkedCount() {
    return this.wasm.tg_checked_len(this.handle);
  }

  // ---- under a multiworld ----

  /**
   * Hands what the locations hold over to a multiworld.
   *
   * From here on checking one records the check and pays out nothing: what
   * was in it is the server's to send, and it comes back through `receive`.
   * Everything read off the checked locations, the level marks among them,
   * carries on working.
   */
  setRemote(on) {
    this.wasm.tg_set_remote(this.handle, on ? 1 : 0);
  }

  /**
   * Hands the run an item the multiworld sent, by the engine's own id.
   *
   * Answers whether the run is better off for it. False for an id no item
   * has and for an unlock that had already arrived, both of which are
   * ordinary rather than a fault: a server resends its whole list every time
   * it says hello.
   */
  receive(id) {
    return this.wasm.tg_receive(this.handle, id) === 1;
  }

  /**
   * Empties what the run is holding, leaving what it has checked alone.
   *
   * For the one thing a multiworld says that nothing else does: this is your
   * whole inventory, forget what you had. Everything but the unlocks stacks,
   * so a resent list added to what was already held would double it.
   */
  forgetItems() {
    this.wasm.tg_forget_items(this.handle);
  }

  /** Whether the run has finished the game, by whatever its goal asks. */
  get goalMet() {
    return this.wasm.tg_goal_met(this.handle) === 1;
  }

  /**
   * What Archipelago's own item and location numbers are offset by.
   *
   * Read out of the engine rather than written down here, so the two cannot
   * drift. A number of ours plus this is a number the server knows.
   */
  get apIdBase() {
    return this.wasm.tg_ap_id_base() >>> 0;
  }

  /** How many AP gems every level of this run carries. */
  get gemsPerLevel() {
    return this.wasm.tg_gems_per_level(this.handle);
  }

  /** Which generation of the world's data this build writes. */
  get generator() {
    return this.wasm.tg_generator();
  }

  /**
   * Whether this build can play a seed that generator made.
   *
   * Asked rather than worked out from the number above, because what a build
   * supports is a list rather than everything up to a ceiling: being able to
   * read one format says nothing about being able to read an older one.
   */
  playsGenerator(version) {
    return this.wasm.tg_plays_generator(version) === 1;
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

  /**
   * Every item's name and every location's name, in the order the engine
   * numbers them, which is what an item event carries.
   *
   * Read out of the engine rather than built here: these are the strings a
   * tracker and a spoiler log show, and a second set assembled on this side
   * would drift from them silently.
   *
   * Cached, because they never change for a session and the feed asks on
   * every item.
   */
  get itemNames() {
    this.itemNameCache ??= this.blob(this.wasm.tg_item_names_ptr, this.wasm.tg_item_names_len);
    return this.itemNameCache;
  }

  get locationNames() {
    this.locationNameCache ??= this.blob(
      this.wasm.tg_location_names_ptr,
      this.wasm.tg_location_names_len,
    );
    return this.locationNameCache;
  }

  /**
   * What a run can be set to, as the engine describes it.
   *
   * Read rather than known: the screen is built by walking this, so a setting
   * added to the engine turns up on it without the front end being edited.
   * Cached because the table cannot change while the module is loaded.
   */
  get options() {
    this.optionCache ??= this.blob(this.wasm.tg_options_ptr, this.wasm.tg_options_len).map(
      (line, index) => {
        const [key, label, about, kind, fallback, ...rest] = line.split('\t');
        const option = { index, key, label, about, kind, default: Number(fallback) };
        if (kind === 'range') {
          option.low = Number(rest[0]);
          option.high = Number(rest[1]);
        } else if (kind === 'weight') {
          // One of a set a yaml takes as a single option. Nothing draws it;
          // what it carries is which set it belongs to.
          option.group = rest[0];
        } else {
          // A choice and a toggle both name their values. The toggle's two
          // are Off and On, which is why it needs no control of its own.
          option.choices = rest.map((choice) => {
            const at = choice.indexOf('=');
            return { value: Number(choice.slice(0, at)), label: choice.slice(at + 1) };
          });
        }
        return option;
      },
    );
    return this.optionCache;
  }

  /** What one setting is set to. */
  optionValue(index) {
    return this.wasm.tg_option_value(this.handle, index) >>> 0;
  }

  /**
   * The value one step along. The engine decides what a step means, because a
   * range stops at its ends and a choice goes round.
   */
  stepOption(index, by) {
    return this.wasm.tg_option_step(this.handle, index, by) >>> 0;
  }

  /**
   * Sets one, which deals the run again from the same seed. Only worth doing
   * before a run starts: a setting decides how many items there are and what
   * the rules ask for, so changing one part way through is not a run anybody
   * could describe.
   */
  setOption(index, value) {
    const ok = this.wasm.tg_set_option(this.handle, index, value) === 1;
    // Changing a setting deals the whole run again, which takes the hold with
    // it. See `setGoalHold`.
    this.pushGoalHold();
    return ok;
  }

  /**
   * How long a beaten level holds still before spending its leftover moves,
   * so the goals can be seen reaching their totals.
   *
   * The page's number rather than the engine's, because the page owns the
   * animation being waited for. Kept here as well as handed over, because a
   * run dealt again (a setting changed, a fresh run started) opens a session
   * that has never been told.
   */
  setGoalHold(ms) {
    this.goalHoldMs = ms;
    this.pushGoalHold();
  }

  pushGoalHold() {
    this.wasm.tg_set_goal_hold(this.handle, this.goalHoldMs ?? 0);
  }

  /**
   * Reads one of the engine's newline separated name tables.
   *
   * Takes the two exports themselves rather than their names: `make abi`
   * finds what this file calls by reading it, and a name looked up as a string
   * is invisible to that, so a rename on the engine side would surface as a
   * blank feed rather than as a failed build.
   */
  blob(ptrCall, lenCall) {
    const length = lenCall(this.handle);
    if (length === 0) {
      return [];
    }
    const bytes = new Uint8Array(this.memory.buffer, ptrCall(this.handle), length);
    return this.decoder.decode(bytes).split('\n');
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

  retry() {
    this.wasm.tg_retry(this.handle);
    this.readGeometry();
  }

  setUnlocked(count) {
    this.wasm.tg_set_unlocked(this.handle, count);
  }

  // ---- the debug menu ----
  //
  // Reachable in the shipped game, by tapping an objective chip twenty times
  // over. Each of these grants what a run would otherwise have found rather
  // than writing an answer over the top of it, so a forced run holds what an
  // ordinary one would and everything downstream still reads true.

  /// Opens every level, whichever way this run's ladder opens.
  unlockAllLevels() {
    this.wasm.tg_unlock_all_levels(this.handle);
  }

  /// Gives the run all five specials.
  unlockAllSpecials() {
    this.wasm.tg_unlock_all_specials(this.handle);
  }

  /// Hands the level in play a different number of moves, total and left.
  setMoves(moves) {
    this.wasm.tg_set_moves(this.handle, moves);
  }

  /// Declares the level in play won, wherever the board is. What follows is
  /// the ordinary end of a level: the clear, the beat, the flourish.
  forceClear() {
    this.wasm.tg_force_clear(this.handle);
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
    this.pushGoalHold();
    this.readGeometry();
  }
}
