// Bootstrap: load the engine, wire input and the HUD to it, and run the frame
// loop. All gameplay decisions live in wasm; this file only feeds it time and
// input and asks what to draw.

import { EventKind, Special, loadEngine, Phase, Status } from './engine.js';
import { Audio } from './audio.js';
import { Renderer } from './render.js';
import { attachInput } from './input.js';
import { Hud } from './hud.js';

const WASM_URL = 'twiddlygems.wasm';
const SAVE_KEY = 'twiddlygems.save.v1';
const SOUND_KEY = 'twiddlygems.sound.v1';
/** How long a player may stare at the board before it offers a move. */
const HINT_DELAY_MS = 6000;
/** A backgrounded tab hands back one enormous frame; cap what we feed in. */
const MAX_FRAME_MS = 100;

const dom = {
  app: document.getElementById('app'),
  canvas: document.getElementById('board'),
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
  scoreTarget: document.getElementById('score-target'),
  moves: document.getElementById('moves'),
  objectives: document.getElementById('objectives'),
  feed: document.getElementById('feed'),
  overlay: document.getElementById('overlay'),
  overlayTitle: document.getElementById('overlay-title'),
  overlayBody: document.getElementById('overlay-body'),
  overlayButtons: document.getElementById('overlay-buttons'),
  levelGrid: document.getElementById('level-grid'),
  levelsButton: document.getElementById('levels-button'),
  hintButton: document.getElementById('hint-button'),
  retryButton: document.getElementById('retry-button'),
  soundButton: document.getElementById('sound-button'),
};

/** Progress lives in the browser; the engine is told about it on start. */
function readSave() {
  const fallback = { seed: freshSeed(), unlocked: 1, level: 0, checked: [], options: {} };
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
  // First of all, because setting one deals the run again: anything restored
  // before this would be thrown away with the session it was restored into.
  for (const option of engine.options) {
    const saved = save.options[option.key];
    if (Number.isInteger(saved)) {
      engine.setOption(option.index, saved);
    }
  }
  engine.setUnlocked(save.unlocked);
  // Before the level is loaded, so it opens holding what the run had earned
  // rather than being dealt bare and corrected a moment later.
  for (const id of save.checked) {
    engine.restore(id);
  }
  if (save.level > 0) {
    engine.loadLevel(save.level);
  }

  const renderer = new Renderer(dom.canvas, engine);
  hud.rebuild();

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
  const gestures = ['pointerdown', 'pointerup', 'keydown'];
  const openAudio = () => {
    audio.unlock();
    if (audio.ready) {
      for (const gesture of gestures) {
        window.removeEventListener(gesture, openAudio, true);
      }
    }
  };
  for (const gesture of gestures) {
    window.addEventListener(gesture, openAudio, { capture: true });
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

  /// The last item this level turned up, for the panel that appears when it
  /// ends. Cleared when another level is dealt.
  let lastFound = null;

  /// Anything the run was given goes in the feed. In solo these come from
  /// clearing levels; under Archipelago the same events will carry what the
  /// multiworld sent, which is why this reads the stream rather than asking
  /// the engine what it happens to be holding.
  const logItems = (events) => {
    for (const event of events) {
      if (event.kind !== EventKind.ITEM) {
        continue;
      }
      const said = hud.describeItem(event);
      if (said) {
        hud.logItem(said);
        // Kept for the end-of-level panel, which says what this level turned
        // up. Taken from the stream rather than asked of the engine, so the
        // panel and the feed cannot disagree about what was found.
        lastFound = said;
        audio.play('sparkle');
      }
    }
  };

  let hintAt = performance.now() + HINT_DELAY_MS;
  let resultShown = false;
  /** 'title' while the menu is up, 'solo' once a run is being played. */
  let mode = 'title';

  const onLevelChanged = () => {
    renderer.layout();
    renderer.reset();
    renderer.hint = null;
    hud.rebuild();
    hud.hideOverlay();
    hintAt = performance.now() + HINT_DELAY_MS;
    resultShown = false;
    lastFound = null;
    writeSave(engine, seed);
  };

  const showTitle = () => {
    mode = 'title';
    hud.hideOverlay();
    hud.hideSetup();
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
    writeSave(engine, seed);
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
    hud.rebuild();
    hud.clearFeed();
    showTitle();
  };

  attachInput(dom.canvas, renderer, engine, () => {
    renderer.hint = null;
    hintAt = performance.now() + HINT_DELAY_MS;
  });

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
      onQuit: () => hud.confirmQuit({ onCancel: openLevels, onConfirm: endRun }),
    });
  };

  dom.levelsButton.addEventListener('click', openLevels);

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

  dom.hintButton.addEventListener('click', () => {
    renderer.hint = engine.hint();
    hintAt = performance.now() + HINT_DELAY_MS;
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
    if (mode !== 'solo') {
      requestAnimationFrame(frame);
      return;
    }

    engine.update(dt);
    const events = engine.drainEvents();
    if (events.length > 0) {
      renderer.addEvents(events, now);
      playEvents(events);
      logItems(events);
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
    hud.update();

    if (engine.status !== Status.PLAYING && !resultShown && !hud.overlayVisible) {
      resultShown = true;
      writeSave(engine, seed);
      hud.showResult(engine.status, {
        onNext: () => {
          if (engine.nextLevel()) {
            onLevelChanged();
          }
        },
        onRetry: () => {
          engine.retry();
          onLevelChanged();
        },
        onLevels: openLevels,
      }, lastFound);
    }

    requestAnimationFrame(frame);
  };

  requestAnimationFrame(frame);
}

boot();
