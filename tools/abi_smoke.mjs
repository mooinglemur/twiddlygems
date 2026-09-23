#!/usr/bin/env node
// Drives the built wasm module the way the browser does, without a browser.
//
// The engine's own tests run natively, so what this checks is the part they
// cannot: that the module instantiates with no imports, that the exports are
// callable across the wasm boundary, and that the buffers the renderer reads
// line up with the board the engine thinks it has.
//
//   make smoke

import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';

const path = process.argv[2] ?? 'web/twiddlygems.wasm';
const decoder = new TextDecoder();

const wasmBytes = await readFile(path);
const compiled = await WebAssembly.compile(wasmBytes);

const imports = WebAssembly.Module.imports(compiled);
assert.equal(
  imports.length,
  0,
  `the module should need no imports, but wants: ${imports.map((i) => `${i.module}.${i.name}`).join(', ')}`,
);

const { exports } = await WebAssembly.instantiate(compiled, {});
const handle = exports.tg_create(20260920, 0);
assert.ok(handle, 'tg_create returned a null session');

const rows = exports.tg_rows(handle);
const cols = exports.tg_cols(handle);
const colors = exports.tg_colors(handle);
assert.ok(rows > 0 && cols > 0, 'the board has no size');

const text = (ptr, len) => decoder.decode(new Uint8Array(exports.memory.buffer, ptr, len));
const levelName = () => text(exports.tg_level_name_ptr(handle), exports.tg_level_name_len(handle));
const names = text(exports.tg_level_names_ptr(handle), exports.tg_level_names_len(handle)).split('\n');
assert.equal(names.length, exports.tg_level_count(handle), 'the name list does not cover the ladder');
assert.equal(names[0], levelName());

/** The board as the renderer sees it, checked against what it should be. */
function checkSnapshot(where) {
  const count = exports.tg_rows(handle) * exports.tg_cols(handle);
  const cells = new Uint8Array(exports.memory.buffer, exports.tg_cells_ptr(handle), count * 4);
  const offsets = new Float32Array(exports.memory.buffer, exports.tg_offsets_ptr(handle), count * 3);
  const seen = new Set();
  for (let i = 0; i < count; i += 1) {
    const color = cells[i * 4];
    const isWall = (cells[i * 4 + 3] & 1) === 1;
    if (isWall) {
      assert.equal(color, 255, `${where}: a wall is holding a gem`);
    } else if (color !== 255) {
      // 255 is an empty cell. Anything else is a gem, and it has to be one of
      // the eight the gem set has a shape for.
      assert.ok(color < 8, `${where}: cell ${i} has color ${color}`);
      seen.add(color);
    }
    assert.ok(Number.isFinite(offsets[i * 3]), `${where}: cell ${i} has a broken offset`);
    assert.ok(offsets[i * 3 + 2] >= 0, `${where}: cell ${i} has a negative scale`);
  }
  // A level deals as many colors as it says, but not necessarily the first
  // few: it may name any set of the eight, so what can be checked from here is
  // how many turned up rather than which.
  assert.ok(
    seen.size <= exports.tg_colors(handle),
    `${where}: ${seen.size} colors on a board that deals ${exports.tg_colors(handle)}`,
  );
}

checkSnapshot('a fresh board');

// Play the level with a bot that always takes the hint.
let frames = 0;
let moves = 0;
while (exports.tg_status(handle) === 0 && frames < 40000) {
  if (exports.tg_phase(handle) === 0) {
    const hint = exports.tg_hint(handle);
    assert.notEqual(hint, 0xffffffff, 'the board went idle with no legal move');
    const accepted = exports.tg_swap(handle, hint >>> 24, (hint >>> 16) & 255, (hint >>> 8) & 255, hint & 255);
    assert.equal(accepted, 1, 'the engine refused its own hint');
    moves += 1;
  } else {
    exports.tg_update(handle, 16);
    if (exports.tg_phase(handle) === 0) {
      checkSnapshot('after a resolve');
    }
  }
  frames += 1;
}

const finalScore = exports.tg_score(handle);
const outcome = ['still playing', 'won', 'lost'][exports.tg_status(handle)];
assert.ok(finalScore > 0, 'a whole level of play scored nothing');
assert.notEqual(exports.tg_status(handle), 0, 'the level never ended');

// Progress restore, the one piece of state the page owns.
exports.tg_set_unlocked(handle, 3);
assert.equal(exports.tg_unlocked(handle), 3);
assert.equal(exports.tg_load_level(handle, 2), 1, 'an unlocked level refused to load');
assert.equal(exports.tg_level_index(handle), 2);
assert.equal(exports.tg_load_level(handle, 9), 0, 'a locked level loaded anyway');
checkSnapshot('after a level change');

exports.tg_destroy(handle);

console.log(
  `smoke ok: ${(wasmBytes.length / 1024).toFixed(0)} KiB module, no imports, ` +
    `${rows}x${cols} board, ${colors} colors, ${names.length} levels, ` +
    `bot played ${moves} moves over ${frames} frames, scored ${finalScore} and ${outcome}`,
);
