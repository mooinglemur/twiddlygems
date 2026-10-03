// Bootstrap: load the engine, wire input and the HUD to it, and run the frame
// loop. All gameplay decisions live in wasm; this file only feeds it time and
// input and asks what to draw.

import {
  AIMED,
  Consumable,
  EventKind,
  ItemKind,
  Special,
  loadEngine,
  Phase,
  Status,
} from './engine.js';
import { ArchipelagoClient, State as Link } from './archipelago.js';
import { Audio } from './audio.js';
import { NOISES, STEAMBOAT, VICTORY_PARTS } from './sounds.js';
import { GOAL_EFFECT_MS, Renderer } from './render.js';
import { attachInput } from './input.js';
import { Hud } from './hud.js';

/**
 * The engine, found relative to this module rather than to the page.
 *
 * `fetch('twiddlygems.wasm')` would resolve against the document, which is
 * the one URL that moves: in production everything but the page is served
 * under a prefix carrying a fingerprint of the build, so that the HTML and
 * the module a player runs can never come from two different deploys. Asking
 * from `import.meta.url` lands beside the modules either way, which is right
 * under the prefix in production and right at the root in development.
 */
const WASM_URL = new URL('../twiddlygems.wasm', import.meta.url).href;
const SAVE_KEY = 'twiddlygems.save.v1';
const SOUND_KEY = 'twiddlygems.sound.v1';
/**
 * What the player has turned on or off, as one JSON object.
 *
 * One key for all of them rather than a key apiece, so a setting can be added
 * without a new entry here and without a migration for the people who have
 * none of it written down yet. Sound is deliberately not in here: it has its
 * own button in the bar and its own key, written long before this, and moving
 * it would silently un-mute everybody who had muted the game.
 */
const SETTINGS_KEY = 'twiddlygems.settings.v1';
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
/**
 * The voice the victory tune is booked under, so it can be taken away again.
 *
 * A name rather than a handle because a figure is one voice however many notes
 * it has: see `Audio.sequence`.
 */
const VICTORY_TUNE = 'victory';
/**
 * How long the screen is up before the tune starts.
 *
 * The panel should land first. Starting them together reads as the tune being
 * something the board did, rather than as the screen having its own music.
 */
const VICTORY_LEAD_IN = 0.35;
/**
 * The voice the tune-shaped noises are booked under, and how many may run.
 *
 * One name for all of them rather than one each, because what the cap is for
 * is the mix: two tunes over each other is already a lot and three is mush.
 * A multiworld can hand over a fistful of items at once, so this is a real
 * case rather than a defensive one.
 */
const NOISE_TUNE = 'noise';
const NOISE_TUNES_AT_ONCE = 2;
/**
 * How long after the find's own chime a noise starts.
 *
 * The chime says a location gave something up and the noise is the something,
 * so they want to be heard in that order. Together they land as one odd sound.
 */
const NOISE_LEAD_IN = 0.14;
/**
 * How long the shuffle sound runs, which is how long the shuffle takes.
 *
 * Has to match `SHUFFLE_MS` in the engine. Kept here as a number rather than
 * asked for over the ABI because it is the only phase length the page needs
 * and an export for one number is not worth the surface; if a second one turns
 * up, that trade changes. A mismatch is a sound that stops while the board is
 * still moving, which is audible, so this is the kind of coupling a person
 * notices rather than one that rots quietly.
 */
const SHUFFLE_SOUND_S = 1.4;
/**
 * How many taps on an objective open the testing menu.
 *
 * High enough that nobody reaches it by fidgeting, and on something that is
 * already on screen in every run, so a phone can get there without a keyboard
 * or a query string. The count resets on a tap anywhere else, which is what
 * makes it a deliberate act rather than a total accumulated over a session.
 */
const DEBUG_TAPS = 20;
/**
 * What the testing menu offers, and the order it is applied in.
 *
 * The order is this list's, not the order they were tapped: `clear` ends the
 * level, so anything that has to happen to a level being played has to happen
 * before it, and `moves` has to come after anything that deals a fresh board.
 */
const DEBUG_CHOICES = [
  { key: 'levels', label: 'Unlock all levels', hint: 'Opens the whole ladder' },
  { key: 'specials', label: 'Unlock all specials', hint: 'All five, on the next board' },
  { key: 'moves', label: 'Set moves to 99', hint: 'This level only' },
  { key: 'inventory', label: 'Fill inventory', hint: '99 of every bonus item' },
  { key: 'shuffle', label: 'Spring a Shuffle Trap', hint: 'Rearranges the board' },
  { key: 'strip', label: 'Spring a Remove Specials Trap', hint: 'Takes the markings off' },
  { key: 'slow', label: 'Spring a Slow Trap', hint: 'Swaps and drops crawl for 30s' },
  { key: 'clear', label: 'Set objectives as met', hint: 'Wins this level now' },
];
/** What "fill the inventory" fills it to. */
const DEBUG_CONSUMABLES = 99;
/** What "set moves to 99" sets them to. */
const DEBUG_MOVES = 99;

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
  overlayCard: document.getElementById('overlay-card'),
  overlayTitle: document.getElementById('overlay-title'),
  overlayBody: document.getElementById('overlay-body'),
  overlayButtons: document.getElementById('overlay-buttons'),
  overlaySwitches: document.getElementById('overlay-switches'),
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
  linkWord: document.getElementById('link-word'),
  linkAside: document.getElementById('link-aside'),
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

/**
 * Whether this browser can buzz.
 *
 * The Vibration API, which Android has and iOS does not, in any browser: on an
 * iPhone this is simply absent. It decides whether the setting is offered at
 * all, because a switch for something the device cannot do is not a setting,
 * it is a lie with a switch on it.
 *
 * Asked each time rather than answered once at load. It costs nothing, and a
 * constant settled before the page has finished starting is the sort of thing
 * that is only ever wrong in the environment nobody tested in.
 */
function canBuzz() {
  return typeof navigator !== 'undefined' && typeof navigator.vibrate === 'function';
}

/**
 * How long the buzz on a swap is, in milliseconds.
 *
 * Short enough to read as a click rather than a buzz. This fires on every
 * swap, which on a good board is several a second, so anything long enough to
 * feel like a vibration would be a nuisance within one level.
 */
const BUZZ_MS = 12;

/// What the settings are and what they default to, for somebody who has never
/// opened the menu.
///
/// Hints on, because a player who does not know hints exist should still be
/// offered them and one who does not want them will go and say so. Haptics
/// off, because a phone buzzing without being asked is a surprise, and this
/// one fires on every swap rather than once in a while.
const SETTING_DEFAULTS = { hints: true, haptics: false };

/// The settings menu's rows. Only offered where the device can honor them.
const SETTING_CHOICES = [
  { key: 'hints', label: 'Hints', hint: 'Shows a move if you pause for a few seconds' },
  {
    key: 'haptics',
    label: 'Haptic feedback',
    hint: 'A small click when gems swap, if supported by your device'
  },
];

function readSettings() {
  try {
    const raw = window.localStorage.getItem(SETTINGS_KEY);
    const saved = raw ? JSON.parse(raw) : null;
    if (!saved || typeof saved !== 'object') {
      return { ...SETTING_DEFAULTS };
    }
    // Read key by key against the defaults rather than spread over them, so a
    // stored file from an older version is missing settings rather than
    // carrying junk into this one, and a value that is not a boolean falls
    // back instead of making a switch that shows one state and means another.
    const settings = { ...SETTING_DEFAULTS };
    for (const key of Object.keys(SETTING_DEFAULTS)) {
      if (typeof saved[key] === 'boolean') {
        settings[key] = saved[key];
      }
    }
    return settings;
  } catch (error) {
    console.warn('could not read the settings', error);
    return { ...SETTING_DEFAULTS };
  }
}

function writeSettings(settings) {
  try {
    window.localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings));
  } catch (error) {
    console.warn('could not remember the settings', error);
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

  /// What the player has turned on or off, read once and written on each
  /// change. Read before anything that acts on one.
  const settings = readSettings();

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
        // Centered: this one is the whole board, not a place on it. Stretched
        // to the length of the animation, so the rattle runs for exactly as
        // long as the gems are moving.
        audio.play('shuffle', { duration: SHUFFLE_SOUND_S });
      } else if (event.kind === EventKind.SLOW) {
        // Dragged down and stretched, which is the sound of the board being
        // put in treacle.
        audio.play('shuffle', { detune: -1400, duration: 1.8 });
      } else if (event.kind === EventKind.SLOW_OVER) {
        audio.play('ding', { detune: -200 });
      } else if (event.kind === EventKind.SPECIALS_LOST) {
        // The shuffle's rattle pitched well down, which reads as something
        // being taken away rather than rearranged. One sound for a trap that
        // has no animation of its own: the board simply changes.
        audio.play('shuffle', { detune: -900, duration: 0.9 });
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

  /**
   * Whether a swap has been made and its gems have not landed yet.
   *
   * Set by `moved` the moment the swipe is accepted, and spent by the buzz
   * below. Kept here rather than read off the phase because the engine reports
   * a swap and the revert that undoes it as the same phase, so a run of them
   * cannot be told apart from the outside.
   */
  let swapLanding = false;

  /**
   * The haptic click, on the frame the swapped gems arrive.
   *
   * Not on the swipe, which is where this started and which was too early by
   * the whole length of the animation: the gems take `SWAP_MS` to slide past
   * each other, so a click at the gesture is a click at two gems that have not
   * moved yet. What is waited for is the swap resolving, which is either the
   * clear it made or the revert that put it back, both raised at the moment
   * the gems land.
   *
   * Armed from the input rather than from an `EV_SWAP`, because that one is
   * raised outside `update` and thrown away by the next one before the page
   * ever drains it. Tracked whether or not haptics are on, so turning them on
   * midway through a swap does not buzz for a swipe made before they were.
   */
  const feelEvents = (events) => {
    for (const event of events) {
      if (!swapLanding) {
        continue;
      }
      if (event.kind !== EventKind.CLEAR && event.kind !== EventKind.REVERT) {
        continue;
      }
      swapLanding = false;
      // Nothing is checked beyond the setting: `vibrate` reports a refusal by
      // returning false rather than by throwing, and a browser that wants a
      // user gesture has had one a fifth of a second ago.
      if (settings.haptics && canBuzz()) {
        navigator.vibrate(BUZZ_MS);
      }
    }
  };

  /**
   * The sound a named filler item makes, if the item at this index is one.
   *
   * Everything about which sound belongs to which item is split between the
   * two sides that each own half of it: the engine owns the names and their
   * order, `NOISES` owns what they sound like, and this is the one line that
   * joins them. Nothing here reads a name, so renaming an item is free and
   * reordering the list is caught by `make abi`.
   *
   * An index this build has no item for reads as an unlock, so it falls out
   * here rather than needing a check of its own.
   */
  const playNoise = (index) => {
    if (engine.itemKind(index) !== ItemKind.NOISE) {
      return;
    }
    playNoiseSound(engine.itemValue(index));
  };

  /**
   * One named filler sound, by its place in `NOISES`.
   *
   * Split from the lookup above so the testing menu can audition one without
   * having to invent an item for it.
   */
  const playNoiseSound = (at) => {
    const noise = NOISES[at];
    if (!noise) {
      return;
    }
    if (noise.figure) {
      audio.sequence(noise.figure.parts, {
        tempo: noise.figure.tempo,
        delay: NOISE_LEAD_IN,
        name: NOISE_TUNE,
        cap: NOISE_TUNES_AT_ONCE,
      });
      return;
    }
    for (const play of noise.plays) {
      audio.play(play.sound, { ...play, delay: NOISE_LEAD_IN + (play.delay ?? 0) });
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
        playNoise(event.value);
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
      // And the noise, if that is what arrived. Off the server's own message
      // rather than off the engine's item event, for the same reason the feed
      // line is: a reconnection replays every item the room ever sent us, and
      // through the events that would be a wall of noises about nothing.
      if (message.itemId !== null && message.itemId !== undefined) {
        playNoise(engine.itemAtId(message.itemId));
      }
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
    feelEvents(events);
    logItems(events);
  };

  let hintAt = performance.now() + HINT_DELAY_MS;
  let resultShown = false;
  /**
   * Whether the run's goal was already met when this frame began, and whether
   * the clear that met it still owes the player a victory screen.
   *
   * An edge rather than a flag anybody writes down. The victory screen belongs
   * to the one clear that meets the goal, so what has to be recognized is the
   * moment it becomes true, and a run whose goal was already met when the page
   * loaded has had its moment. Reading it off the engine each frame means no
   * new state in the save and nothing to keep in step with a reconnection: the
   * engine is asked, rather than the page remembering.
   *
   * `goalOwed` is separate from the edge because the goal is met during the
   * flourish, while the screen is not shown until the level is over.
   */
  let goalWasMet = false;
  let goalOwed = false;
  /**
   * Where the page is: 'title' or 'setup' or 'connect' while a menu is up,
   * 'solo' or 'multiworld' once a run is being played.
   *
   * The two playing modes differ in exactly three places, all of them here:
   * the clock runs for both, the save is only written for one, and the feed is
   * fed from a different direction.
   */
  let mode = 'title';

  /// What the connection overlay is already saying, so a frame that would say
  /// the same thing touches nothing. See `showLink`.
  let linkSaid = '';

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

  /**
   * Takes the goal as read where the run stands now.
   *
   * Called as a run begins, so that a solo save loaded with its goal long
   * since met, or a multiworld slot rejoined after finishing, does not open on
   * a victory screen for something that happened days ago. The screen is for
   * the moment the goal falls, and that moment has to be watched for rather
   * than inferred from the goal being true.
   */
  const armGoalWatch = () => {
    goalWasMet = engine.goalMet;
    goalOwed = false;
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
    hideLink();
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
    armGoalWatch();
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
    armGoalWatch();
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
      // Before the overlay is drawn, because until this has run there is no
      // multiworld to draw it over.
      if (mode !== 'multiworld') {
        startMultiworld();
      }
    } else if (state === Link.REFUSED) {
      hud.setConnectStatus(detail ?? 'The room refused the connection', 'bad');
    }
    showLink();
  };

  /**
   * What the overlay over the board should say about the connection.
   *
   * Every state says something, including the good one: a player who watched
   * it fail wants to see it come back, so "Connected." is shown and then
   * fades, which is the one message that goes away on its own.
   */
  const linkNow = () => {
    if (client.state === Link.PLAYING) {
      return { text: 'Connected.', kind: 'good', fade: true };
    }
    if (client.state === Link.REFUSED) {
      return { text: client.error ?? 'The room refused the connection', kind: 'bad' };
    }
    if (client.state === Link.CONNECTING || client.state === Link.HANDSHAKING) {
      return { text: 'Connecting…', kind: 'bad' };
    }
    const seconds = client.retrySeconds;
    return {
      text: 'Disconnected.',
      kind: 'bad',
      // Read off the client's own clock rather than one kept here, and held
      // apart from the words: it changes every second, and a live region that
      // changed every second would be read out every second.
      aside: seconds === null ? '' : ` (reconnecting in ${seconds}s)`,
    };
  };

  /**
   * Keeps that overlay saying what is true now.
   *
   * Called on every state change and on every frame, for the countdown. The
   * page is only written when the words change, which is once a second at the
   * very most and on the overwhelming majority of frames is never.
   */
  const showLink = () => {
    if (mode !== 'multiworld') {
      return;
    }
    const now = linkNow();
    const said = `${now.kind}/${now.text}${now.aside ?? ''}`;
    if (said === linkSaid) {
      return;
    }
    linkSaid = said;
    hud.showLink(now.text, now);
  };

  /// Puts it away, for leaving the room: nothing about a connection is worth
  /// saying once there is not one.
  const hideLink = () => {
    linkSaid = '';
    hud.showLink('');
  };

  client = new ArchipelagoClient(engine, { onFeed: onSaid, onState: onLinkState });

  /// Leaves the room and puts the solo run back, which was never disturbed.
  const leaveMultiworld = () => {
    client.disconnect();
    hideLink();
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
  const moved = (what) => {
    renderer.hint = null;
    hintAt = performance.now() + HINT_DELAY_MS;
    // Armed here and felt later, by `feelEvents`. Only on a swap: a tap that
    // selects a gem has not moved anything yet.
    if (what === 'swap') {
      swapLanding = true;
    }
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
      onSettings: openSettings,
    });
  };

  /**
   * The settings, from the gear in the levels menu.
   *
   * Back returns to the levels menu rather than to the board, because that is
   * where this was opened from and leaving somebody somewhere they did not ask
   * to be is worse than one extra tap.
   *
   * A setting takes effect the moment it is tapped. Nothing here has to be
   * handed to the engine: both are read where they are acted on, the hint by
   * the frame loop and the buzz by the swap that causes it, so writing the new
   * value down is the whole of applying it.
   */
  const openSettings = () => {
    hud.showSettings(
      // Only what this device can actually do. A phone that cannot buzz is not
      // shown a switch for buzzing.
      SETTING_CHOICES.filter((choice) => choice.key !== 'haptics' || canBuzz()).map((choice) => ({
        ...choice,
        on: settings[choice.key],
      })),
      {
        onChange: (key, on) => {
          settings[key] = on;
          writeSettings(settings);
          // Hints turned off should go now rather than at the end of the
          // countdown that is already running, and one already on the board
          // has to be taken off it by hand.
          if (key === 'hints' && !on) {
            renderer.hint = null;
          }
        },
        onBack: openLevels,
      },
    );
  };

  /**
   * The run is over and won: the victory panel, and the tune that goes with it.
   *
   * The tune is started here rather than from the panel, because the HUD draws
   * what it is told to and has never made a sound. It is also the one sound in
   * the game that outlasts what started it, so every way off this screen takes
   * it away again: twenty seconds of music playing on over the level select
   * would be worse than not playing it at all.
   */
  const showVictory = () => {
    audio.sequence(VICTORY_PARTS, {
      tempo: STEAMBOAT.tempo,
      name: VICTORY_TUNE,
      delay: VICTORY_LEAD_IN,
    });
    const leaving = (go) => () => {
      audio.hush(VICTORY_TUNE);
      go();
    };
    hud.showVictory({
      onLevels: leaving(openLevels),
      onClose: leaving(() => hud.hideOverlay()),
    });
  };

  /**
   * The testing menu, and what its switches do on the way out.
   *
   * Applied in `DEBUG_CHOICES` order rather than the order they were tapped,
   * because two of these interact: a forced clear ends the level, so it has to
   * be last, and the moves have to be set after anything that deals a board,
   * since dealing one takes its move count from the level rather than from
   * whatever the last one was left holding.
   *
   * Unlocking the specials re-deals the level, because a special the run did
   * not hold when the board was built cannot appear on it: the rules a board
   * is dealt under are fixed when it is dealt. Not when the level is also
   * being cleared, where a fresh board would throw away the thing being asked
   * for.
   */
  const applyDebug = (chosen) => {
    const picked = new Set(chosen);
    hud.hideOverlay();
    if (picked.has('levels')) {
      engine.unlockAllLevels();
    }
    if (picked.has('specials')) {
      engine.unlockAllSpecials();
      if (!picked.has('clear')) {
        engine.retry();
        onLevelChanged();
      }
    }
    if (picked.has('moves')) {
      engine.setMoves(DEBUG_MOVES);
    }
    if (picked.has('inventory')) {
      for (const kind of Object.values(Consumable)) {
        engine.restoreConsumables(kind, DEBUG_CONSUMABLES);
      }
    }
    // Before the clear below, and it does not matter: a trap armed on the
    // frame a level is won waits for a board that is never idle again, which
    // is the engine's own rule about not springing one into a flourish.
    if (picked.has('shuffle')) {
      engine.springShuffle();
    }
    if (picked.has('strip')) {
      engine.springRemoveSpecials();
    }
    if (picked.has('slow')) {
      engine.springSlow();
    }
    // Last, and on its own frame's terms: this hands the level to the ordinary
    // settle, which announces the clear and starts the flourish.
    if (picked.has('clear')) {
      engine.forceClear();
    }
    rebuildHud();
    saveRun();
  };

  /**
   * The testing menu, and the sound list behind it.
   *
   * The two are one call because they lead to each other: the sound list hands
   * the switches back when it opens and returns them when it closes, so a
   * tester can set up a clear, go and listen to something, and come back to
   * find the clear still armed.
   *
   * The sounds are auditioned here rather than in a panel of their own because
   * this is where a tester already is, and because the page is the side that
   * knows what a noise sounds like: the HUD is handed a list of names and
   * hands back which one was pressed.
   */
  const openDebug = (picked = []) => {
    hud.showDebug(DEBUG_CHOICES, {
      remote: mode === 'multiworld',
      picked,
      onClose: applyDebug,
      onSounds: (stillPicked) => {
        hud.showAudition(
          NOISES.map((noise) => noise.name),
          { onPlay: playNoiseSound, onBack: () => openDebug(stillPicked) },
        );
      },
    });
  };

  /**
   * Tapping an objective twenty times over opens the testing menu.
   *
   * Counted on the list rather than on a chip, because the chips are rebuilt
   * every level and a listener on one would go with it. Any objective counts:
   * a level may have three of them and insisting on the same one would make
   * the gesture harder on a phone without making it harder to reach by
   * accident, which twenty taps already handles.
   */
  let objectiveTaps = 0;
  window.addEventListener(
    'pointerdown',
    (event) => {
      if (!dom.objectives.contains?.(event.target)) {
        // Anywhere else at all, which is what "in a row" means.
        objectiveTaps = 0;
        return;
      }
      objectiveTaps += 1;
      if (objectiveTaps < DEBUG_TAPS) {
        return;
      }
      objectiveTaps = 0;
      openDebug();
    },
    { capture: true },
  );

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

    // Above the return below rather than beside the rest of the multiworld's
    // frame: a connection can drop while the player is reading a finished
    // level, and the countdown has to keep counting there too.
    showLink();

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

    // The goal falling is watched for here rather than read at the end of the
    // level, because by then it is only "the goal is met", which is equally
    // true of every level cleared afterwards. The clear that meets it is the
    // one frame where it changes.
    if (!goalWasMet && engine.goalMet) {
      goalWasMet = true;
      goalOwed = true;
    }

    if (engine.phase !== Phase.IDLE) {
      hintAt = now + HINT_DELAY_MS;
      if (renderer.hint) {
        renderer.hint = null;
      }
    } else if (settings.hints && !renderer.hint && now >= hintAt && engine.acceptsInput) {
      renderer.hint = engine.hint();
    }

    renderer.draw(now);
    // After the draw, so what the chips say about what is still on its way is
    // this frame's answer rather than the last one's.
    hud.update(renderer);

    if (engine.status !== Status.PLAYING && !resultShown && !hud.overlayVisible) {
      resultShown = true;
      saveRun();
      if (goalOwed && engine.status === Status.WON) {
        goalOwed = false;
        showVictory();
      } else {
        // A goal met on a level that was then lost is not a thing that can
        // happen, but if it ever became one the clear panel is the honest
        // answer and the victory screen is simply not owed any more.
        goalOwed = false;
        hud.showResult(engine.status, {
          onRetry: () => {
            engine.retry();
            onLevelChanged();
          },
          onLevels: openLevels,
          // Leaves the finished board on screen, which is the one thing the
          // panel is in the way of. It does not come back on its own: the
          // panel is shown once per attempt, and the way on from here is the
          // level select or the restart button.
          onClose: () => hud.hideOverlay(),
        });
      }
    }

    requestAnimationFrame(frame);
  };

  requestAnimationFrame(frame);
}

boot();
