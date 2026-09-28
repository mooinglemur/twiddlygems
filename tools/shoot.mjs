#!/usr/bin/env node
// Plays the game in a real headless browser and writes screenshots.
//
// This is the only way to see the thing from a terminal, and it is a real end
// to end check: a served page, a real wasm module, real input events, and the
// frame loop running in real time. It reports anything the page threw.
//
// Note for anyone reaching for `--screenshot` instead: Chrome's
// `--virtual-time-budget` freezes the compositor, so requestAnimationFrame
// fires two or three times and the game never advances. Screenshots taken that
// way show a first frame and nothing else. Hence the DevTools Protocol.
//
//   make shots

import { writeFile, mkdir } from 'node:fs/promises';
import { spawn } from 'node:child_process';

// Said here rather than found out three frames deep. Node gained a global
// `WebSocket` in 22.4, and this talks to the browser over one; on an older
// one the failure is a bare ReferenceError from inside the CDP setup, which
// reads like the browser refusing to start.
if (typeof WebSocket === 'undefined') {
  console.error(`this needs Node 22.4 or newer for its WebSocket; this is ${process.version}.`);
  process.exit(1);
}

const OUT = process.argv[2] ?? 'shots';
const BROWSER = process.env.BROWSER ?? 'google-chrome-stable';
const PORT = Number(process.env.SHOT_PORT ?? 8099);
const DEBUG_PORT = Number(process.env.SHOT_DEBUG_PORT ?? 9333);
// Which level to photograph. Not the opening one: First Light is a three move
// puzzle that has to be solved rather than swiped at, and the shots below play
// a level out by following hints, which is no way to solve a puzzle. The
// second level is an ordinary board with room on it, which is what most of the
// game looks like.
// Which level the shots are taken on.
//
// Pillars rather than one of the early ones, and it is a practical choice
// rather than an aesthetic one: these shots include the end-of-level run down,
// so the level has to be one this tool can actually finish. It plays by
// following the engine's hints, which is a poor player, and the ladder is now
// tuned so that an *attentive* one clears about half the time. Holding
// everything the save below grants, a hint follower takes this one almost
// every attempt and most of the ladder hardly ever.
const LEVEL = Number(process.env.SHOT_LEVEL ?? 8);

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
await mkdir(OUT, { recursive: true });

const server = spawn('python3', ['-m', 'http.server', String(PORT), '--bind', '127.0.0.1', '--directory', 'web'], {
  stdio: 'ignore',
});
const browser = spawn(
  BROWSER,
  [
    // Muted because this plays the game for real, sound and all, and headless
    // or not it comes out of whatever speakers the machine is using.
    '--headless', '--no-sandbox', '--disable-gpu', '--hide-scrollbars', '--mute-audio',
    `--remote-debugging-port=${DEBUG_PORT}`,
    '--user-data-dir=/tmp/twiddlygems-shots',
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
    } catch {}
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
const thrown = [];
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
    thrown.push(data.params.exceptionDetails);
  } else if (data.method === 'Runtime.consoleAPICalled' && data.params.type === 'error') {
    thrown.push(data.params.args.map((a) => a.value ?? a.description).join(' '));
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
    // So an expression can wait on frames going by and hand back what it
    // found, rather than returning a promise nobody unwraps.
    awaitPromise: true,
  });
  // Without this a script that throws comes back as `undefined` and the run
  // carries on: the page is never driven, every shot is of whatever was on
  // screen before, and nothing anywhere says so.
  if (exceptionDetails) {
    const thrown = exceptionDetails.exception?.description ?? exceptionDetails.text;
    throw new Error(`evaluating ${expression.trim().slice(0, 80)}: ${thrown}`);
  }
  return result.value;
};

async function shoot(name) {
  const { data } = await send('Page.captureScreenshot', { format: 'png' });
  await writeFile(`${OUT}/${name}.png`, Buffer.from(data, 'base64'));
}

/** A press, an optional drag, and a release: a tap or a swipe. */
async function gesture(x, y, dx = 0, dy = 0) {
  const shared = { button: 'left', clickCount: 1, buttons: 1 };
  await send('Input.dispatchMouseEvent', { type: 'mousePressed', x, y, ...shared });
  if (dx || dy) {
    await sleep(20);
    await send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: x + dx, y: y + dy, ...shared });
  }
  await sleep(20);
  await send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: x + dx, y: y + dy, ...shared });
}

const readout = () =>
  evaluate(`({
    score: document.getElementById('score').textContent,
    moves: document.getElementById('moves').textContent,
    objective: document.querySelector('.objective-count')?.textContent ?? null,
    level: document.getElementById('level-name').textContent,
  })`);

await send('Page.enable');
await send('Runtime.enable');
// The profile lives across runs, and only the wasm module is fetched
// no-cache, so without this the page comes back with yesterday's JS and the
// screenshot quietly shows the last version of the front end.
await send('Network.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });

for (const [name, metrics] of [
  ['phone', { width: 390, height: 844, deviceScaleFactor: 3, mobile: true }],
  ['desktop', { width: 1280, height: 860, deviceScaleFactor: 1, mobile: false }],
]) {
  await send('Emulation.setDeviceMetricsOverride', metrics);
  // A fresh seed each run would make shots incomparable, so pin one.
  await send('Page.navigate', { url: `http://127.0.0.1:${PORT}/index.html` });
  await sleep(600);
  await evaluate(
    // Unlocked past the end of the ladder, so any level can be photographed
    // without playing up to it, and holding the five unlocks, without which
    // the board makes no specials at all and the shots are of a much plainer
    // game than anyone past the opening level plays.
    //
    // Which locations those are is the seed's business now, so rather than
    // naming five the run claims every level clear (ids 0 up) and every chain
    // short enough to hold an item (1002 to 1006). The five unlocks are placed
    // first, before anything a run holds opens a score mark, so they can only
    // ever be among those: claiming the lot is what makes the shots show the
    // same game whatever seed is pinned.
    //
    // Plus a silver and a gold on a couple of levels (2000 and 3000 up), so
    // the level picker has one of each to show rather than a grid of green.
    // The cost is that the picker is photographed as a finished ladder: no
    // row in these shots is an unlocked level nobody has cleared yet.
    //
    // Deliberately not on the level being photographed, so the marks popover
    // is shot with something still to reach rather than with both behind it.
    // The AP gems are claimed too (4000 up, one to a level at the default
    // setting). An unlock can land in one, because collecting one asks only
    // for being able to play its level, so a run claiming every clear and
    // chain but not these can come up short of the five.
    //
    // All but the last three levels', which are left in the ground on
    // purpose: the picker carries a mark for them now, and a ladder with
    // every gem taken photographs that mark in one state only. Those three
    // are deep enough that nothing else in these shots waits on them.
    // And a few of each thing there is to spend, because an empty bottom bar
    // is the one state of it these shots would otherwise always be of. Keyed
    // by the engine's own code for each kind, the same as the save writes it,
    // and ten of them altogether, which is what a run is dealt by default.
    // And the ladder set to open by clearing, which is not the default. A run
    // opening it by item is only as far up as its items have carried it, and
    // which location holds the first unlock is the seed's business, so a save
    // claiming to be on the second level would be a save the engine is right
    // to refuse. These shots are of the game's screens; the ladder's own rule
    // is checked in the engine's tests and the world's.
    `localStorage.setItem('twiddlygems.save.v1', JSON.stringify({ seed: 20260920, unlocked: 99, level: ${LEVEL}, options: { progressive_levels: 0 }, consumables: { 0: 3, 1: 1, 2: 2, 3: 4 }, checked: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 1002, 1003, 1004, 1005, 1006, 2002, 3002, 2003, 4000, 4010, 4020, 4030, 4040, 4050, 4060, 4070, 4080, 4090] }))`,
  );
  // `?debug` puts the engine, renderer and HUD on `window.twiddlygems`, which
  // is how the shots below reach past the board to things an ordinary run only
  // gets to by playing for a while.
  await send('Page.navigate', { url: `http://127.0.0.1:${PORT}/index.html?debug` });
  await sleep(2200);

  // The game opens on its menu now, so that gets photographed and then
  // dismissed: nothing below can reach the board until a mode is chosen.
  await shoot(`${name}-00-title`);

  // The screen where a run is set up. A saved run goes straight past it, and
  // these shots are of a run well along, so it is opened directly rather than
  // by clearing the save and reloading. It is the real screen either way:
  // built by walking the engine's table, from the same call the button makes.
  await evaluate(`window.twiddlygems.hud.showSetup()`);
  await sleep(200);
  await shoot(`${name}-01-setup`);
  await evaluate(`window.twiddlygems.hud.hideSetup()`);

  // And the other way in, which asks for a room rather than for settings:
  // under a multiworld the settings come from the room. Shot with something
  // in the boxes, because an empty form is a picture of placeholder text.
  await evaluate(
    `window.twiddlygems.hud.showConnect({ host: 'archipelago.gg', port: '38281', slot: 'twiddly' })`,
  );
  await sleep(200);
  await shoot(`${name}-02-connect`);
  await evaluate(`window.twiddlygems.hud.hideConnect()`);

  await evaluate(`document.getElementById('solo-button').click()`);
  await sleep(400);

  const box = await evaluate(`
    (() => {
      const r = document.getElementById('board').getBoundingClientRect();
      return { left: r.left, top: r.top, width: r.width };
    })()
  `);
  const cell = box.width / 8.4;
  const pad = cell * 0.2;
  const at = (r, c) => ({
    x: box.left + pad + (c + 0.5) * cell,
    y: box.top + pad + (r + 0.5) * cell,
  });

  await shoot(`${name}-01-fresh`);

  // Something out of the bottom bar: armed and waiting for a cell, then
  // spent on one and caught in the air on its way up out of the bar. The
  // board is put back afterwards, so everything below is photographed on the
  // same deal it always was.
  const slot = await evaluate(`
    (() => {
      const r = document.querySelectorAll('.consumable')[0].getBoundingClientRect();
      return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
    })()
  `);
  await gesture(slot.x, slot.y);
  await sleep(200);
  await shoot(`${name}-01-armed`);

  const aimed = at(2, 4);
  await gesture(aimed.x, aimed.y);
  // Part way up: a rocket spent out of the bar has the whole board to cross,
  // so this catches it over the gems rather than at either end.
  await sleep(700);
  await shoot(`${name}-01-spent`);
  await sleep(1600);
  await evaluate(`document.getElementById('retry-button').click()`);
  await evaluate(`window.twiddlygems.engine.restoreConsumables(0, 3)`);
  await sleep(400);

  const selected = at(4, 3);
  await gesture(selected.x, selected.y);
  await sleep(300);
  await shoot(`${name}-02-selected`);
  await gesture(selected.x, selected.y);
  await sleep(200);

  // Swipe around until the board has some score on it.
  for (let i = 0; i < 40; i += 1) {
    const from = at(i % 8, (i * 3) % 7);
    await gesture(from.x, from.y, cell, 0);
    await sleep(420);
  }
  await shoot(`${name}-03-played`);
  const state = await readout();

  // The item feed holds its height empty, so the shot above is the case that
  // matters most. This one is what it looks like with something in it, which
  // an ordinary run only reaches by clearing a level.
  //
  // Each line carries a `worth`, which is what colors the item's name: one
  // logged without it wears the plain accent, and these stand in for lines the
  // game raised itself. 2 is progression and 1 is useful; see `ItemClass`.
  await evaluate(`
    (() => {
      const { hud } = window.twiddlygems;
      hud.logItem({ said: 'Found ', what: 'Horizontal Line Clear', where: 'Level 2 Clear', worth: 2 });
      hud.logItem({ said: 'Found ', what: 'Level 3 Moves Upgrade', where: 'Level 2 Gold', worth: 2 });
      hud.logItem({ said: 'Received ', what: 'Inventory Item: Rocket', where: null, worth: 1 });
    })()
  `);
  await sleep(200);
  await shoot(`${name}-04-feed`);

  // What the level can be beaten to, which lives behind a tap of the score
  // now. Nothing an ordinary run does opens it, so it is opened here.
  await evaluate(`document.getElementById('score-box').click()`);
  await sleep(200);
  await shoot(`${name}-04-marks`);
  await evaluate(`window.twiddlygems.hud.showScoreMarks(false)`);
  await sleep(120);

  // The end of a level: the goal met, the moves left over being spent one at
  // a time, and each gem turning into a special throwing motes. Played out
  // through the engine's own hints rather than by gesture, because forty more
  // swipes is a minute of screenshot run time.
  const cashingIn = await evaluate(`
    (async () => {
      const { engine, Phase, Status } = {
        ...window.twiddlygems,
        Phase: { CASHING_IN: 7 },
        Status: { LOST: 2 },
      };
      // Retried the way a player would, through the page's own button so the
      // board and the HUD are reset together. One attempt was enough when the
      // budgets were loose; measured, even a level this bot is good at is not
      // a certainty, and a screenshot run that falls over one time in twenty
      // is a build that falls over one time in twenty.
      for (let attempt = 0; attempt < 12; attempt += 1) {
        // A safety net rather than a budget: the loop leaves as soon as the
        // attempt resolves either way, so this only has to be longer than the
        // longest a level can take. Wide, because that is now a level with
        // fifty-odd moves rather than the eight this was first sized for, and
        // a cap shorter than the level does not fail an attempt, it abandons
        // every attempt unfinished.
        for (let i = 0; i < 4000; i += 1) {
          if (engine.phase === Phase.CASHING_IN) {
            return true;
          }
          if (engine.status === Status.LOST) {
            break;
          }
          if (engine.acceptsInput) {
            const move = engine.hint();
            if (move) { engine.swap(...move); }
          }
          await new Promise((done) => requestAnimationFrame(done));
        }
        if (engine.phase === Phase.CASHING_IN) {
          return true;
        }
        document.getElementById('retry-button').click();
        await new Promise((done) => requestAnimationFrame(done));
      }
      return false;
    })()
  `);
  if (!cashingIn) {
    console.error('never reached the end-of-level run down, so there is no shot of it');
    stop();
    process.exit(1);
  }
  await sleep(600);
  await shoot(`${name}-05-cashing-in`);

  // And what the flourish leads to: the panel that says how the level went.
  // It comes up on its own once the board stops, so this waits for it rather
  // than asking for it, which is also a check that it appears at all.
  const panel = await evaluate(`
    (async () => {
      for (let i = 0; i < 4000; i += 1) {
        if (!document.getElementById('overlay').classList.contains('hidden')) {
          return true;
        }
        await new Promise((done) => requestAnimationFrame(done));
      }
      // Said rather than a bare false: a level that never ended and a panel
      // that never came up are different faults with the same symptom.
      return 'status ' + window.twiddlygems.engine.status
        + ', overlay ' + document.getElementById('overlay').className;
    })()
  `);
  if (panel !== true) {
    console.error(`the level never raised its finished panel (${panel})`);
    stop();
    process.exit(1);
  }
  await sleep(300);
  await shoot(`${name}-05-finished`);

  await evaluate(`document.getElementById('levels-button').click()`);
  await sleep(400);
  await shoot(`${name}-06-levels`);

  // `hidden` is a utility class, and every panel it goes on is an id selector
  // that sets its own `display`, which outweighs a bare class. When that goes
  // wrong the JS looks right and nothing throws: the panel simply stays on
  // screen, which is how a stale level picker sat under the end-of-level
  // buttons unnoticed. Only a real browser computes this, so it is checked
  // here rather than in the stubbed page smoke.
  const stuck = await evaluate(`
    (() => {
      const bad = [];
      for (const element of document.querySelectorAll('[id]')) {
        const had = element.classList.contains('hidden');
        element.classList.add('hidden');
        if (getComputedStyle(element).display !== 'none') {
          bad.push(element.id);
        }
        if (!had) {
          element.classList.remove('hidden');
        }
      }
      return bad;
    })()
  `);
  if (stuck.length) {
    console.error(`the "hidden" class does not hide: ${stuck.join(', ')}`);
    stop();
    process.exit(1);
  }

  await evaluate(`document.querySelector('#overlay-buttons button').click()`);
  await sleep(200);

  console.log(`${name}: ${JSON.stringify(state)}`);
}

if (thrown.length) {
  console.error(`the page reported ${thrown.length} error(s):`);
  console.error(JSON.stringify(thrown, null, 2).slice(0, 3000));
  stop();
  process.exit(1);
}

console.log(`shots ok: no page errors, written to ${OUT}/`);
socket.close();
stop();
