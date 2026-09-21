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
  canvas: document.getElementById('board'),
  stage: document.getElementById('stage'),
  levelNumber: document.getElementById('level-number'),
  levelName: document.getElementById('level-name'),
  score: document.getElementById('score'),
  moves: document.getElementById('moves'),
  objectives: document.getElementById('objectives'),
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
  const fallback = { seed: Math.floor(Math.random() * 2 ** 32), unlocked: 1, level: 0 };
  try {
    const raw = window.localStorage.getItem(SAVE_KEY);
    if (!raw) {
      return fallback;
    }
    const save = JSON.parse(raw);
    return {
      seed: Number.isFinite(save.seed) ? save.seed : fallback.seed,
      unlocked: Number.isInteger(save.unlocked) ? save.unlocked : 1,
      level: Number.isInteger(save.level) ? save.level : 0,
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
      JSON.stringify({ seed, unlocked: engine.unlocked, level: engine.levelIndex }),
    );
  } catch (error) {
    console.warn('could not save progress', error);
  }
}

async function boot() {
  const hud = new Hud(null, dom);
  let engine;
  const save = readSave();

  try {
    engine = await loadEngine(WASM_URL, save.seed);
  } catch (error) {
    console.error(error);
    hud.showError(
      `${error.message}. Build it with "make wasm", and serve this folder over http rather than opening the file directly.`,
    );
    return;
  }

  hud.engine = engine;
  engine.setUnlocked(save.unlocked);
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
  // the handler itself, so this runs on the first touch anywhere.
  const openAudio = () => audio.unlock();
  window.addEventListener('pointerdown', openAudio, { once: true, capture: true });
  window.addEventListener('keydown', openAudio, { once: true, capture: true });

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
    // step itself rather than off gems going away — a rocket landing takes a
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
      } else if (event.kind === EventKind.LAND) {
        // The engine works out when each column touches down, so the thud is
        // scheduled for the moment the gems actually stop rather than for the
        // moment they set off.
        audio.play('thud', {
          delay: event.value / 1000,
          pan: ((event.c / spread) * 2 - 1) * 0.5,
        });
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

  let hintAt = performance.now() + HINT_DELAY_MS;
  let resultShown = false;

  const onLevelChanged = () => {
    renderer.layout();
    renderer.reset();
    renderer.hint = null;
    hud.rebuild();
    hud.hideOverlay();
    hintAt = performance.now() + HINT_DELAY_MS;
    resultShown = false;
    writeSave(engine, save.seed);
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

  dom.levelsButton.addEventListener('click', () => {
    hud.showLevels({
      onPick: (index) => {
        if (engine.loadLevel(index)) {
          onLevelChanged();
        }
      },
      onClose: () => hud.hideOverlay(),
    });
  });

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

  let last = performance.now();
  const frame = (now) => {
    const dt = Math.min(now - last, MAX_FRAME_MS);
    last = now;

    engine.update(dt);
    const events = engine.drainEvents();
    if (events.length > 0) {
      renderer.addEvents(events, now);
      playEvents(events);
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
      writeSave(engine, save.seed);
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
        onLevels: () =>
          hud.showLevels({
            onPick: (index) => {
              if (engine.loadLevel(index)) {
                onLevelChanged();
              }
            },
            onClose: () => hud.hideOverlay(),
          }),
      });
    }

    requestAnimationFrame(frame);
  };

  requestAnimationFrame(frame);
}

boot();
