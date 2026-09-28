// Bootstrap: load the engine, wire input and the HUD to it, and run the frame
// loop. All gameplay decisions live in wasm; this file only feeds it time and
// input and asks what to draw.

import { AIMED, Consumable, EventKind, Special, loadEngine, Phase, Status } from './engine.js';
import { ArchipelagoClient, State as Link } from './archipelago.js';
import { Audio } from './audio.js';
import { GOAL_EFFECT_MS, Renderer } from './render.js';
import { attachInput } from './input.js';
import { Hud } from './hud.js';

const WASM_URL = 'twiddlygems.wasm';
const SAVE_KEY = 'twiddlygems.save.v1';
const SOUND_KEY = 'twiddlygems.sound.v1';
/**
 * Where the last room was, so a returning player types nothing but a password.
 *
 * The only thing about a multiworld this browser writes down. Everything else
 * about the run lives on the server, which is what lets a player open the same
 * slot somewhere else and carry on. The password is deliberately not here.
 */
const ROOM_KEY = 'twiddlygems.ap.room.v1';
/**
 * What the room says that belongs in the feed.
 *
 * Nearly everything: the feed is where the room is heard from, and a player
 * wants to know somebody joined as much as they want to know what they found.
 * The two left out are addressed to the client rather than to the player.
 */
const WORTH_SAYING = new Set([
  'ItemSend',
  'ItemCheat',
  'Hint',
  'Join',
  'Part',
  'Chat',
  'ServerChat',
  'Goal',
  'Release',
  'Collect',
  'Countdown',
  'Text',
]);
/**
 * How long a player may stare at the board before it offers a move.
 *
 * The only way a hint appears. There was a button for it too, which asked the
 * player to admit to wanting one and took up a corner of the bar for something
 * that happens by itself a few seconds later.
 */
const HINT_DELAY_MS = 6000;
/** A backgrounded tab hands back one enormous frame; cap what we feed in. */
const MAX_FRAME_MS = 100;

const dom = {
  app: document.getElementById('app'),
  canvas: document.getElementById('board'),
  fx: document.getElementById('fx'),
  stage: document.getElementById('stage'),
  title: document.getElementById('title'),
  soloButton: document.getElementById('solo-button'),
  soloNote: document.getElementById('solo-note'),
  archipelagoButton: document.getElementById('archipelago-button'),
  setup: document.getElementById('setup'),
  setupOptions: document.getElementById('setup-options'),
  setupStart: document.getElementById('setup-start'),
  setupBack: document.getElementById('setup-back'),
  levelNumber: document.getElementById('level-number'),
  levelName: document.getElementById('level-name'),
  score: document.getElementById('score'),
  scoreBox: document.getElementById('score-box'),
  scoreMarks: document.getElementById('score-marks'),
  moves: document.getElementById('moves'),
  objectives: document.getElementById('objectives'),
  inventory: document.getElementById('inventory'),
  feed: document.getElementById('feed'),
  overlay: document.getElementById('overlay'),
  overlayTitle: document.getElementById('overlay-title'),
  overlayBody: document.getElementById('overlay-body'),
  overlayButtons: document.getElementById('overlay-buttons'),
  tracker: document.getElementById('tracker'),
  trackerItems: document.getElementById('tracker-items'),
  levelList: document.getElementById('level-list'),
  levelsButton: document.getElementById('levels-button'),
  retryButton: document.getElementById('retry-button'),
  soundButton: document.getElementById('sound-button'),
  connect: document.getElementById('connect'),
  connectForm: document.getElementById('connect-form'),
  connectHost: document.getElementById('connect-host'),
  connectPort: document.getElementById('connect-port'),
  connectSlot: document.getElementById('connect-slot'),
  connectPassword: document.getElementById('connect-password'),
  connectStatus: document.getElementById('connect-status'),
  connectBack: document.getElementById('connect-back'),
  link: document.getElementById('link'),
};

/** The last room joined, so the form opens mostly filled in. */
function readRoom() {
  try {
    const raw = window.localStorage.getItem(ROOM_KEY);
    const room = raw ? JSON.parse(raw) : {};
    return room && typeof room === 'object' ? room : {};
  } catch (error) {
    console.warn('could not read the last room', error);
    return {};
  }
}

function writeRoom({ host, port, slot }) {
  try {
    // No password. It is the one thing here worth keeping out of a store that
    // any script on this origin can read, and it is also the one thing short
    // enough to type again.
    window.localStorage.setItem(ROOM_KEY, JSON.stringify({ host, port, slot }));
  } catch (error) {
    console.warn('could not remember the room', error);
  }
}

/** Progress lives in the browser; the engine is told about it on start. */
function readSave() {
  const fallback = {
    seed: freshSeed(),
    unlocked: 1,
    level: 0,
    checked: [],
    bestScores: [],
    consumables: {},
    options: {},
  };
  try {
    const raw = window.localStorage.getItem(SAVE_KEY);
    if (!raw) {
      return fallback;
    }
    const save = JSON.parse(raw);
    // The seed dealt the run's progression, so the location ids below only
    // mean anything alongside it. Without one, what was found cannot be looked
    // back up: the levels the player opened are kept and the finds are not,
    // rather than handing them items from a layout they never played.
    const seeded = Number.isFinite(save.seed);
    return {
      seed: seeded ? save.seed : fallback.seed,
      unlocked: Number.isInteger(save.unlocked) ? save.unlocked : 1,
      level: Number.isInteger(save.level) ? save.level : 0,
      checked: seeded && Array.isArray(save.checked) ? save.checked.filter(Number.isInteger) : [],
      // What each level was beaten with, by level. Not derivable from the
      // locations: a level cleared below its silver checks the same one
      // whatever it scored, so the number has to be written down.
      bestScores: Array.isArray(save.bestScores) ? save.bestScores.filter(Number.isFinite) : [],
      // What the run still has to spend. Not derivable from the locations
      // either, and for the opposite reason to the scores: these are the one
      // holding that goes down, so what was found says nothing about what is
      // left. Under Archipelago this will live in the multiworld's own data
      // store instead, so a player logging in from another browser gets their
      // run back; in solo there is nowhere but here.
      consumables:
        save.consumables && typeof save.consumables === 'object' ? save.consumables : {},
      // By key rather than by position, because a setting added later would
      // shift the positions and quietly hand a returning run somebody else's
      // settings. A key the engine no longer has is simply skipped.
      options: save.options && typeof save.options === 'object' ? save.options : {},
    };
  } catch (error) {
    // A corrupt or unavailable store should cost a save, not the game.
    console.warn('could not read saved progress', error);
    return fallback;
  }
}

function writeSave(engine, seed) {
  try {
    window.localStorage.setItem(
      SAVE_KEY,
      JSON.stringify({
        seed,
        unlocked: engine.unlocked,
        level: engine.levelIndex,
        // What the run has found. Without these a reload keeps the levels a
        // player unlocked and quietly takes back everything they earned on
        // the way, which is worse than losing both.
        checked: engine.checked,
        // And what each level was beaten with, which no location records: a
        // level cleared below its silver checks the same one whatever it
        // scored.
        bestScores: engine.bestScores,
        // And what it still has to spend, by the engine's own code for each
        // kind. By code rather than by position for the same reason the
        // settings are saved by key: a fifth kind appended later must not
        // hand a returning run somebody else's stock.
        consumables: Object.fromEntries(
          Object.values(Consumable).map((kind) => [kind, engine.consumables(kind)]),
        ),
        // And what sort of run it is, since the settings decide how many
        // items there are and what the rules ask for. A reload that forgot
        // them would rebuild a different game around the same saved finds.
        options: Object.fromEntries(
          engine.options.map((option) => [option.key, engine.optionValue(option.index)]),
        ),
      }),
    );
  } catch (error) {
    console.warn('could not save progress', error);
  }
}

function clearSave() {
  try {
    window.localStorage.removeItem(SAVE_KEY);
  } catch (error) {
    console.warn('could not clear saved progress', error);
  }
}

function freshSeed() {
  return Math.floor(Math.random() * 2 ** 32);
}

async function boot() {
  const hud = new Hud(null, dom);
  let engine;
  const save = readSave();
  // Not const: ending a run deals a new one from a new seed.
  let seed = save.seed;

  try {
    engine = await loadEngine(WASM_URL, seed);
  } catch (error) {
    console.error(error);
    // The title screen sits above the overlay, so it has to go or the failure
    // is announced behind a menu whose one button leads nowhere.
    dom.title.classList.add('hidden');
    hud.showError(
      `${error.message}. Build it with "make wasm", and serve this folder over http rather than opening the file directly.`,
    );
    return;
  }

  hud.engine = engine;
  // A beaten level holds still until the goals have finished showing
  // themselves met, and how long that takes is this file's business rather
  // than the engine's: it is the flight time of the motes. Handed over before
  // anything else, so it survives the run being dealt again below.
  engine.setGoalHold(GOAL_EFFECT_MS);

  /**
   * Puts a saved solo run back into a session that has none.
   *
   * Used twice: once at startup, and once on the way back from a multiworld,
   * which plays in a session of its own and leaves this save untouched. One
   * piece of code rather than two, because the order below is the whole of it
   * and getting it wrong in one place only would be a bug nobody could see.
   */
  const restoreSolo = (from) => {
    // First of all, because setting one deals the run again: anything restored
    // before this would be thrown away with the session it was restored into.
    for (const option of engine.options) {
      const saved = from.options[option.key];
      if (Number.isInteger(saved)) {
        engine.setOption(option.index, saved);
      }
    }
    engine.setUnlocked(from.unlocked);
    // Before the level is loaded, so it opens holding what the run had earned
    // rather than being dealt bare and corrected a moment later.
    for (const id of from.checked) {
      engine.restore(id);
    }
    from.bestScores.forEach((score, index) => engine.restoreBestScore(index, score));
    for (const [kind, held] of Object.entries(from.consumables)) {
      if (Number.isInteger(held) && held > 0) {
        engine.restoreConsumables(Number(kind), held);
      }
    }
    if (from.level > 0) {
      engine.loadLevel(from.level);
    }
  };

  restoreSolo(save);

  const renderer = new Renderer(dom.canvas, engine, dom.fx);

  /// The HUD and the renderer both have to be told when the level changes, and
  /// in that order: the chips are what a clear's motes fly to, so the renderer
  /// cannot be handed them until they exist.
  const rebuildHud = () => {
    hud.rebuild();
    renderer.setGoals(hud.goals());
  };

  rebuildHud();

  const audio = new Audio();
  let soundOn = true;
  try {
    soundOn = window.localStorage.getItem(SOUND_KEY) !== 'off';
  } catch (error) {
    console.warn('could not read the sound setting', error);
  }
  audio.setEnabled(soundOn);
  dom.soundButton.textContent = soundOn ? '\u{1F50A}' : '\u{1F507}';
  dom.soundButton.setAttribute('aria-pressed', String(!soundOn));

  // Audio may only be opened from a gesture, and on iOS it must happen inside
  // the handler itself, so this runs on a touch anywhere.
  //
  // It keeps trying until the context is actually running, rather than taking
  // one shot at it. A browser can accept the call and leave the context
  // suspended anyway: Firefox on Android does not count a gesture as having
  // happened until it finishes, so opening on `pointerdown` alone gets a
  // context that never starts, and the game plays in silence while the button
  // still says the sound is on. Listening for the end of the gesture as well
  // covers the same ground from the other side.
  // Kept for the life of the page rather than taken off the moment the device
  // opens. A context that has been running can be suspended again long
  // afterwards: the phone locks, the browser goes to the background, a call
  // arrives. Nothing else would ever bring it back, so the rest of the session
  // played in silence with the button still saying the sound was on.
  //
  // Cheap to leave in place: once there is a running context `unlock` is a
  // property read and a return. And a browser will only let a context resume
  // from a gesture anyway, so the next tap is exactly the moment to try.
  const gestures = ['pointerdown', 'pointerup', 'keydown'];
  const openAudio = () => audio.unlock();
  for (const gesture of gestures) {
    window.addEventListener(gesture, openAudio, { capture: true, passive: true });
  }

  dom.soundButton.addEventListener('click', () => {
    soundOn = !soundOn;
    audio.unlock();
    audio.setEnabled(soundOn);
    dom.soundButton.textContent = soundOn ? '\u{1F50A}' : '\u{1F507}';
    dom.soundButton.setAttribute('aria-pressed', String(!soundOn));
    try {
      window.localStorage.setItem(SOUND_KEY, soundOn ? 'on' : 'off');
    } catch (error) {
      console.warn('could not save the sound setting', error);
    }
  });

  /// Each cleared gem gets its pop, scheduled on the audio clock with the delay
  /// the engine gave it, and placed left to right by the column it was in.
  const playEvents = (events) => {
    const spread = Math.max(1, engine.cols - 1);

    // One chord per step of the chain, climbing as it goes. This keys off the
    // step itself rather than off gems going away: a rocket landing takes a
    // gem with it, but it is not a beat of the music. The engine resets the
    // count when the board settles, so a fresh chain starts at the bottom of
    // the progression on its own.
    const step = events.find((event) => event.kind === EventKind.MATCH);
    if (step) {
      audio.play('chime', { stage: Math.max(0, step.value - 1) });
    }

    for (const event of events) {
      if (event.kind === EventKind.CLEAR) {
        const pan = ((event.c / spread) * 2 - 1) * 0.55;
        audio.play('pop', { delay: event.value / 1000, pan });
        // And the shimmer it leaves behind, ringing on after the pop.
        audio.play('sparkle', { delay: event.value / 1000, pan });
      } else if (event.kind === EventKind.SPECIAL_MADE) {
        // A voice for the gem turning into something. Quiet enough (a single
        // sparkle peaks around 0.002) that the extra one on an ordinary match,
        // where the clears are already sparkling, cannot be heard.
        audio.play('sparkle', { pan: ((event.c / spread) * 2 - 1) * 0.5 });
      } else if (event.kind === EventKind.CASH_IN) {
        // One struck bell per leftover move, placed or not. The run down is a
        // sequence of these and nothing else, so it carries the whole sound of
        // a level ending.
        const pan = ((event.c / spread) * 2 - 1) * 0.5;
        audio.play('bell', { pan });
        audio.play('sparkle', { pan });
      } else if (event.kind === EventKind.LAND) {
        // The engine works out when each column touches down, so the thud is
        // scheduled for the moment the gems actually stop rather than for the
        // moment they set off.
        audio.play('thud', {
          delay: event.value / 1000,
          pan: ((event.c / spread) * 2 - 1) * 0.5,
        });
      } else if (event.kind === EventKind.BRICK) {
        // Louder when the hit was the one that broke it than when it only
        // cracked, which is the whole difference between the two states.
        audio.play('crack', {
          gain: event.value === 0 ? 1 : 0.6,
          pan: ((event.c / spread) * 2 - 1) * 0.45,
        });
      } else if (event.kind === EventKind.SHUFFLE) {
        // Centered: this one is the whole board, not a place on it.
        audio.play('shuffle');
      } else if (event.kind === EventKind.LOW_MOVES) {
        audio.play('ding');
      } else if (event.kind === EventKind.CLEARED) {
        // The goals are met, so the score can start wearing the color of what
        // it has reached. Not the same as the level being over: the flourish
        // is still adding, and the color climbs with it.
        hud.cleared = true;
        // And the fanfare, alongside the toast that says the level is cleared.
        // The renderer raises that off this same event a moment earlier in the
        // frame, so the two land together without either having to know about
        // the other.
        audio.play('fanfare');
      } else if (event.kind === EventKind.REVERT) {
        audio.play('clack', { pan: ((event.c / spread) * 2 - 1) * 0.4 });
      } else if (event.kind === EventKind.ROCKET_HIT) {
        audio.play('boom', { pan: ((event.c / spread) * 2 - 1) * 0.4 });
      } else if (event.kind === EventKind.SPECIAL_FIRED && event.special === Special.ROCKET) {
        // The engine sends the flight time, so the whistle lasts exactly as
        // long as the rocket is in the air.
        audio.play('rocket', {
          duration: event.value / 1000,
          pan: ((event.c / spread) * 2 - 1) * 0.4,
        });
      }
    }
  };

  /// Anything the run was given goes in the feed. In solo these come from
  /// clearing levels; under Archipelago the same events will carry what the
  /// multiworld sent, which is why this reads the stream rather than asking
  /// the engine what it happens to be holding.
  const logItems = (events) => {
    // Not in a multiworld. There the same events still fire, and they are the
    // run being told what it holds rather than the player being told what
    // happened: a reconnection replays every item the server ever sent, which
    // through here would be a wall of news about nothing. What the player
    // reads comes from the server's own messages instead, which are only ever
    // sent live. See `onSaid`.
    if (mode === 'multiworld') {
      return;
    }
    for (const event of events) {
      if (event.kind !== EventKind.ITEM) {
        continue;
      }
      const said = hud.describeItem(event);
      if (said) {
        hud.logItem(said);
        audio.play('sparkle');
      }
    }
  };

  /// Something the room said, which in a multiworld is the whole of the feed.
  const onSaid = (message) => {
    if (mode !== 'multiworld' || !WORTH_SAYING.has(message.type)) {
      return;
    }
    hud.logParts(message.parts);
    // The same chime a solo find gets, and only for the ones that are ours:
    // a busy room would otherwise be a metronome.
    if (message.mine) {
      audio.play('sparkle');
    }
  };

  /// Everything the last call into the engine raised: drawn, sounded, logged.
  ///
  /// Called for the frame's own tick and for anything the player sets off in
  /// between, because the engine reports what the last call raised and nothing
  /// else. Spending a rainbow raises its whole first clear inside that call,
  /// so leaving it for the next tick to notice would drop every pop, every
  /// piece of debris and every mote of it on the floor.
  const consume = (now) => {
    const events = engine.drainEvents();
    if (events.length === 0) {
      return;
    }
    renderer.addEvents(events, now);
    playEvents(events);
    logItems(events);
  };

  let hintAt = performance.now() + HINT_DELAY_MS;
  let resultShown = false;
  /**
   * Where the page is: 'title' or 'setup' or 'connect' while a menu is up,
   * 'solo' or 'multiworld' once a run is being played.
   *
   * The two playing modes differ in exactly three places, all of them here:
   * the clock runs for both, the save is only written for one, and the feed is
   * fed from a different direction.
   */
  let mode = 'title';

  /**
   * The connection to a multiworld, made once and kept for the page's life.
   *
   * Built further down, because what it reports goes to handlers that are not
   * written yet at this point in the file.
   */
  let client = null;

  /** Whether a run is being played, whoever is dealing it. */
  const playing = () => mode === 'solo' || mode === 'multiworld';

  /**
   * Writes the solo save, unless this is not a solo run.
   *
   * A multiworld run must never land in here. Everything it holds belongs to
   * the server and none of it means anything without the room it came from, so
   * writing it would both corrupt the solo run waiting underneath and leave a
   * save that could not be loaded.
   */
  const saveRun = () => {
    if (mode !== 'multiworld') {
      writeSave(engine, seed);
    }
  };

  const onLevelChanged = () => {
    renderer.layout();
    renderer.reset();
    renderer.hint = null;
    rebuildHud();
    hud.hideOverlay();
    // Whatever was armed was armed at the board that just went away.
    hud.disarm();
    hintAt = performance.now() + HINT_DELAY_MS;
    resultShown = false;
    saveRun();
  };

  const showTitle = () => {
    mode = 'title';
    hud.hideOverlay();
    hud.hideSetup();
    hud.hideConnect();
    hud.showLink('');
    dom.title.classList.remove('hidden');
    // The board is still laid out underneath so the canvas keeps its size;
    // hiding it from assistive tech is what stops it being read as content.
    dom.app.setAttribute('aria-hidden', 'true');
    const levels = engine.levelCount;
    dom.soloNote.textContent =
      engine.unlocked > 1 ? `Continue: ${engine.unlocked} of ${levels} levels unlocked` : `${levels} levels`;
  };

  /// The title screen's Solo button: set the run up before it starts.
  ///
  /// A separate step rather than a button that begins immediately, because
  /// the settings are fixed for the run's whole length, the way a
  /// multiworld's yaml is. Backing out returns to the title.
  const setUpSolo = () => {
    mode = 'setup';
    dom.title.classList.add('hidden');
    hud.showSetup();
  };

  const startSolo = () => {
    mode = 'solo';
    hud.hideSetup();
    dom.title.classList.add('hidden');
    dom.app.removeAttribute('aria-hidden');
    renderer.layout();
    hintAt = performance.now() + HINT_DELAY_MS;
    // Pin the seed now rather than at the first level change, so reloading
    // part way through the opening level deals the same board again.
    saveRun();
  };

  // ---- the multiworld ----

  /// The title screen's Archipelago button: ask where the room is.
  const setUpMultiworld = () => {
    mode = 'connect';
    dom.title.classList.add('hidden');
    hud.showConnect(readRoom());
  };

  /**
   * Joins a room, on a session of its own.
   *
   * The engine is restarted first, which throws away whatever solo run was
   * loaded. That run is still in its save and untouched: a multiworld deals
   * its own progression and the two must not be mixed, and coming back to the
   * title puts the solo one back.
   */
  const joinMultiworld = async () => {
    const room = hud.connectDetails();
    if (!room.host || !room.slot) {
      hud.setConnectStatus('A server and a slot name are both needed', 'bad');
      return;
    }
    hud.setConnectStatus('Connecting…');
    seed = freshSeed();
    engine.restart(seed);
    hud.clearFeed();
    try {
      await client.connect(room);
      writeRoom(room);
    } catch (error) {
      // Failing to reach the room at all. Being refused by it is a different
      // thing that happens later and arrives through `onLinkState`.
      hud.setConnectStatus(error.message, 'bad');
    }
  };

  /// The handshake finished, so there is a run to play.
  const startMultiworld = () => {
    mode = 'multiworld';
    hud.hideConnect();
    dom.title.classList.add('hidden');
    dom.app.removeAttribute('aria-hidden');
    // The run was dealt again on the way in, by every setting the room sent,
    // so everything drawn from it has to be built from scratch.
    renderer.reset();
    renderer.hint = null;
    rebuildHud();
    renderer.layout();
    hud.hideOverlay();
    hud.disarm();
    resultShown = false;
    hintAt = performance.now() + HINT_DELAY_MS;
  };

  /**
   * How the connection is doing, from the client.
   *
   * The same handler covers the way in and everything after it, because being
   * refused by a room is not a different sort of event from being dropped by
   * one an hour later.
   */
  const onLinkState = (state, detail) => {
    if (state === Link.PLAYING) {
      hud.setConnectStatus('Connected', 'good');
      hud.showLink('');
      if (mode !== 'multiworld') {
        startMultiworld();
      }
      return;
    }
    if (state === Link.REFUSED) {
      const said = detail ?? 'The room refused the connection';
      hud.setConnectStatus(said, 'bad');
      // Already playing when it happened, which a version mismatch cannot be
      // but a room being shut down mid-game can.
      if (mode === 'multiworld') {
        hud.showLink(said, 'bad');
      }
      return;
    }
    if (mode === 'multiworld') {
      hud.showLink(state === Link.LOST ? 'Connection lost, retrying…' : 'Reconnecting…', 'bad');
    }
  };

  client = new ArchipelagoClient(engine, { onFeed: onSaid, onState: onLinkState });

  /// Leaves the room and puts the solo run back, which was never disturbed.
  const leaveMultiworld = () => {
    client.disconnect();
    hud.showLink('');
    hud.clearFeed();
    const saved = readSave();
    seed = saved.seed;
    // A fresh session, which is also what clears the engine of the multiworld:
    // a new one is its own run again until something says otherwise.
    engine.restart(seed);
    restoreSolo(saved);
    mode = 'title';
    renderer.reset();
    renderer.hint = null;
    rebuildHud();
    hud.disarm();
    showTitle();
  };

  /// Ends the run: the saved progress goes, and the engine opens a new session
  /// so the next run is dealt from a different seed rather than replaying this
  /// one from the start.
  const endRun = () => {
    clearSave();
    seed = freshSeed();
    engine.restart(seed);
    renderer.reset();
    renderer.hint = null;
    resultShown = false;
    rebuildHud();
    hud.disarm();
    hud.clearFeed();
    showTitle();
  };

  /// A move was made, so the hint goes and the clock on the next one starts
  /// again.
  const moved = () => {
    renderer.hint = null;
    hintAt = performance.now() + HINT_DELAY_MS;
  };

  /**
   * Spends whatever is armed on the cell that was tapped, or says the tap was
   * not ours.
   *
   * Taking the tap either way while something is armed. A refusal means the
   * cell can do nothing with it, a rainbow pointed at an Archipelago gem being
   * the one that matters: the run is not charged, and it stays armed so the
   * next cell gets it. What it must not do is fall through and select the gem,
   * which would put the player mid-swap having meant to aim.
   */
  const spendOn = (cell) => {
    if (hud.armed === null) {
      return false;
    }
    const kind = hud.armed;
    if (engine.useConsumable(kind, cell)) {
      hud.disarm();
      // Before anything else looks at the engine: the whole first clear was
      // raised inside that call and the next tick would clear it away.
      consume(performance.now());
      moved();
      spent(kind);
      saveRun();
    }
    return true;
  };

  /**
   * One of the things the run was carrying has been used up.
   *
   * In a multiworld this goes to the server rather than into a file here. The
   * server knows what it sent and has no idea any of it was spent, so without
   * this a reload would hand the player back everything they had fired. In
   * solo the save covers it, which is what the line above does.
   */
  const spent = (kind) => {
    if (mode === 'multiworld') {
      client.spend(kind);
    }
  };

  /// Tapping a slot arms it, or puts it away if it was already armed. The
  /// cluster aims itself, so there is nothing to arm: it goes off where it
  /// stands.
  hud.buildInventory((kind) => {
    // An empty slot is nothing to arm. The button is disabled too, so this is
    // the belt to that brace: the count comes off the engine and the slot is
    // only redrawn once a frame, so a tap can land on a slot that emptied
    // between the two.
    if (engine.consumables(kind) === 0) {
      return;
    }
    if (!AIMED.has(kind)) {
      hud.disarm();
      if (engine.useConsumable(kind)) {
        consume(performance.now());
        moved();
        spent(kind);
        saveRun();
      }
      return;
    }
    hud.arm(kind);
  });

  // Anywhere but the board puts an armed item away, which is the way out of
  // having armed one. The board is where it goes, and the slot it came out of
  // has its own handler that toggles it, so neither of those counts.
  window.addEventListener(
    'pointerdown',
    (event) => {
      if (hud.armed === null || dom.canvas.contains?.(event.target)) {
        return;
      }
      if (hud.armedSlot()?.contains?.(event.target)) {
        return;
      }
      hud.disarm();
    },
    { capture: true },
  );

  attachInput(dom.canvas, renderer, engine, moved, spendOn);

  // The board is re-laid-out rather than stretched, so a rotation or a
  // keyboard appearing keeps whole pixels per cell.
  const observer = new ResizeObserver(() => renderer.layout());
  observer.observe(dom.stage);
  window.addEventListener('orientationchange', () => renderer.layout());

  const openLevels = () => {
    hud.showLevels({
      onPick: (index) => {
        if (engine.loadLevel(index)) {
          onLevelChanged();
        }
      },
      onClose: () => hud.hideOverlay(),
      // Leaving a room is not the same as throwing a solo run away: the room
      // keeps everything this run has done, so there is nothing to lose by
      // going and the solo save is still sitting where it was.
      onQuit: () =>
        hud.confirmQuit({
          onCancel: openLevels,
          onConfirm: mode === 'multiworld' ? leaveMultiworld : endRun,
        }),
    });
  };

  dom.levelsButton.addEventListener('click', openLevels);

  // Tapping the score shows what the level can be beaten to; tapping anywhere
  // else puts it away. The dismissal is a capture-phase listener on the window
  // rather than a backdrop over the page, because a backdrop would swallow the
  // first tap on the board, and putting a popover away is not worth a move.
  dom.scoreBox.addEventListener('click', () => hud.toggleScoreMarks());
  window.addEventListener(
    'pointerdown',
    (event) => {
      if (hud.marksVisible && !dom.scoreBox.contains?.(event.target)) {
        hud.showScoreMarks(false);
      }
    },
    { capture: true },
  );

  dom.soloButton.addEventListener('click', () => {
    // Straight in for a run already under way: the settings it was dealt with
    // are part of it now, and offering them again would only be offering to
    // throw it away.
    if (engine.unlocked > 1 || engine.checked.length > 0) {
      startSolo();
      return;
    }
    setUpSolo();
  });

  dom.setupStart.addEventListener('click', startSolo);
  dom.setupBack.addEventListener('click', showTitle);

  dom.archipelagoButton.addEventListener('click', setUpMultiworld);
  dom.connectForm.addEventListener('submit', (event) => {
    // A form rather than a bare button, so a phone keyboard shows Go and
    // pressing it connects. Which means stopping the page from reloading.
    event.preventDefault();
    joinMultiworld();
  });
  dom.connectBack.addEventListener('click', () => {
    client.disconnect();
    hud.hideConnect();
    showTitle();
  });

  dom.retryButton.addEventListener('click', () => {
    engine.retry();
    onLevelChanged();
  });

  // A hand-hold for development: with ?debug on the URL the engine and the
  // renderer are reachable from the console, and from the screenshot tooling,
  // which is how the animation timings get checked.
  if (new URLSearchParams(window.location?.search ?? '').has('debug')) {
    window.twiddlygems = { engine, renderer, hud, audio };
  }

  showTitle();

  let last = performance.now();
  const frame = (now) => {
    const dt = Math.min(now - last, MAX_FRAME_MS);
    last = now;

    // The title screen holds the clock rather than running a level nobody can
    // see behind it. `last` is still moved on above, so choosing a mode does
    // not hand the engine the whole time the menu was up as one frame.
    if (!playing()) {
      requestAnimationFrame(frame);
      return;
    }

    engine.update(dt);
    consume(now);

    // What this frame checked, if anything, and whether the run is over. Both
    // are a number compared against a number on a frame where nothing
    // happened, which is almost all of them: see `poll`.
    if (mode === 'multiworld') {
      client.poll();
    }

    if (engine.phase !== Phase.IDLE) {
      hintAt = now + HINT_DELAY_MS;
      if (renderer.hint) {
        renderer.hint = null;
      }
    } else if (!renderer.hint && now >= hintAt && engine.acceptsInput) {
      renderer.hint = engine.hint();
    }

    renderer.draw(now);
    // After the draw, so what the chips say about what is still on its way is
    // this frame's answer rather than the last one's.
    hud.update(renderer);

    if (engine.status !== Status.PLAYING && !resultShown && !hud.overlayVisible) {
      resultShown = true;
      saveRun();
      hud.showResult(engine.status, {
        onRetry: () => {
          engine.retry();
          onLevelChanged();
        },
        onLevels: openLevels,
        // Leaves the finished board on screen, which is the one thing the
        // panel is in the way of. It does not come back on its own: the panel
        // is shown once per attempt, and the way on from here is the level
        // select or the restart button.
        onClose: () => hud.hideOverlay(),
      });
    }

    requestAnimationFrame(frame);
  };

  requestAnimationFrame(frame);
}

boot();
