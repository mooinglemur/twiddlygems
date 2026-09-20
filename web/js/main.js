// Bootstrap: load the engine, wire input and the HUD to it, and run the frame
// loop. All gameplay decisions live in wasm; this file only feeds it time and
// input and asks what to draw.

import { loadEngine, Phase, Status } from './engine.js';
import { Renderer } from './render.js';
import { attachInput } from './input.js';
import { Hud } from './hud.js';

const WASM_URL = 'twiddlygems.wasm';
const SAVE_KEY = 'twiddlygems.save.v1';
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

  let hintAt = performance.now() + HINT_DELAY_MS;
  let resultShown = false;

  const onLevelChanged = () => {
    renderer.layout();
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

  let last = performance.now();
  const frame = (now) => {
    const dt = Math.min(now - last, MAX_FRAME_MS);
    last = now;

    engine.update(dt);
    engine.drainEvents(); // Read and dropped for now; sound and particles hook in here.

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
