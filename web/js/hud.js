// The DOM around the board: level heading, score, moves, objective chips, and
// the overlay used for results and level selection.

import { ItemKind, ObjectiveKind, Special, Status } from './engine.js';
import { PALETTE } from './render.js';

/// How many lines the feed keeps. Well past what fits, so scrolling back a
/// little works, and far short of a session's worth.
const FEED_LIMIT = 40;

/// What each special is called, and the match that leaves one behind. The
/// second half is the point: an unlock is being announced to someone who has
/// never seen that gem, so it says how to make one.
const SPECIALS = {
  [Special.LINE_H]: { name: 'Horizontal Line Clear', from: 'four in a column' },
  [Special.LINE_V]: { name: 'Vertical Line Clear', from: 'four in a row' },
  [Special.CROSS]: { name: 'Cross Clear', from: 'an L or a T' },
  [Special.RAINBOW]: { name: 'Rainbow', from: 'five in a line' },
  [Special.ROCKET]: { name: 'Rocket', from: 'a 2x2 square' },
};

export class Hud {
  constructor(engine, dom) {
    this.engine = engine;
    this.dom = dom;
    this.shownScore = 0;
    this.objectiveViews = [];
    this.lastMoves = -1;
  }

  /** Rebuilds everything that only changes when the level does. */
  rebuild() {
    const { engine, dom } = this;
    dom.levelNumber.textContent = `Level ${engine.levelIndex + 1}`;
    dom.levelName.textContent = engine.levelName;
    this.shownScore = engine.score;
    this.lastMoves = -1;
    dom.score.textContent = Math.round(this.shownScore).toLocaleString();

    dom.objectives.replaceChildren();
    this.objectiveViews = engine.objectives().map((objective) => {
      const item = document.createElement('li');
      item.className = 'objective';

      const label = document.createElement('div');
      label.className = 'objective-label';
      if (objective.kind === ObjectiveKind.COLOR || objective.kind === ObjectiveKind.SEAL) {
        const swatch = document.createElement('span');
        swatch.className = 'objective-swatch';
        swatch.style.background = PALETTE[objective.color % PALETTE.length].fill;
        label.append(swatch, describe(objective));
      } else {
        label.textContent = describe(objective);
      }

      const count = document.createElement('div');
      count.className = 'objective-count';

      const bar = document.createElement('div');
      bar.className = 'objective-bar';
      const fill = document.createElement('div');
      fill.className = 'objective-fill';
      bar.append(fill);

      item.append(label, count, bar);
      dom.objectives.append(item);
      return { item, count, fill };
    });
  }

  /** Per-frame refresh; touches the DOM only where something moved. */
  update() {
    const { engine, dom } = this;

    const score = engine.score;
    if (this.shownScore !== score) {
      // Ease toward the real score so a cascade reads as a run-up, but snap
      // backwards instantly when a restart zeroes it.
      const gap = score - this.shownScore;
      this.shownScore = gap < 0 || gap < 1 ? score : this.shownScore + Math.max(1, gap * 0.18);
      dom.score.textContent = Math.round(this.shownScore).toLocaleString();
    }

    const moves = engine.movesLeft;
    if (moves !== this.lastMoves) {
      this.lastMoves = moves;
      dom.moves.textContent = String(moves);
      dom.moves.classList.toggle('low', moves <= 3);
    }

    const objectives = engine.objectives();
    for (let i = 0; i < this.objectiveViews.length && i < objectives.length; i += 1) {
      const objective = objectives[i];
      const view = this.objectiveViews[i];
      const text = `${objective.have.toLocaleString()} / ${objective.need.toLocaleString()}`;
      if (view.count.textContent !== text) {
        view.count.textContent = text;
        const ratio = objective.need === 0 ? 1 : objective.have / objective.need;
        view.fill.style.width = `${Math.min(100, ratio * 100).toFixed(1)}%`;
        view.item.classList.toggle('met', objective.have >= objective.need);
      }
    }
  }

  /** The end-of-level panel. */
  showResult(status, actions) {
    const { engine, dom } = this;
    const won = status === Status.WON;
    const lastLevel = engine.levelIndex + 1 >= engine.levelCount;

    dom.overlayTitle.textContent = won
      ? lastLevel
        ? 'Ladder complete'
        : 'Level complete'
      : 'Out of moves';
    const lines = [];
    if (won) {
      lines.push(`${Math.round(engine.score).toLocaleString()} points on ${engine.levelName}.`);
      const granted = SPECIALS[engine.grantedSpecial];
      if (granted) {
        lines.push(`Unlocked: ${granted.name}, from ${granted.from}.`);
      }
    } else {
      lines.push(unmetSummary(engine));
    }
    dom.overlayBody.textContent = lines.join(' ');

    const buttons = [];
    if (won && !lastLevel) {
      buttons.push(button('Next level', actions.onNext, true));
    }
    buttons.push(button(won ? 'Play again' : 'Try again', actions.onRetry, !won));
    buttons.push(button('Levels', actions.onLevels, false));
    dom.overlayButtons.replaceChildren(...buttons);

    dom.levelGrid.classList.add('hidden');
    dom.overlay.classList.remove('hidden');
  }

  /**
   * The level the picker marks: the one being played, or, once it has been
   * won, the one that "Next level" beside it would start.
   *
   * Marking the level just finished reads as though that is where the player
   * still is, which is wrong the moment the panel offers to move them on.
   */
  focusedLevel() {
    const { engine } = this;
    const next = engine.levelIndex + 1;
    return engine.status === Status.WON && next < engine.levelCount ? next : engine.levelIndex;
  }

  /** The level picker. */
  showLevels(actions) {
    const { engine, dom } = this;
    dom.overlayTitle.textContent = 'Levels';
    dom.overlayBody.textContent = `${engine.unlocked} of ${engine.levelCount} unlocked.`;

    const focused = this.focusedLevel();
    const names = engine.levelNames();
    const chips = [];
    for (let i = 0; i < engine.levelCount; i += 1) {
      const chip = document.createElement('button');
      chip.type = 'button';
      chip.className = 'level-chip';
      if (i === focused) {
        chip.classList.add('current');
      }
      const unlocked = i < engine.unlocked;
      chip.disabled = !unlocked;

      const number = document.createElement('span');
      number.className = 'n';
      number.textContent = `Level ${i + 1}`;
      const title = document.createElement('span');
      title.className = 't';
      title.textContent = unlocked ? names[i] ?? '' : 'Locked';
      chip.append(number, title);
      chip.addEventListener('click', () => actions.onPick(i));
      chips.push(chip);
    }

    dom.levelGrid.replaceChildren(...chips);
    dom.levelGrid.classList.remove('hidden');
    dom.overlayButtons.replaceChildren(
      button('Close', actions.onClose, true),
      button('Title screen', actions.onQuit, false),
    );
    dom.overlay.classList.remove('hidden');
  }

  /**
   * Quitting throws the run away, so it asks first. The harmless answer is the
   * primary one and comes first, because this is reached from a menu rather
   * than from a deliberate "wipe my progress" button.
   */
  confirmQuit(actions) {
    const { engine, dom } = this;
    dom.overlayTitle.textContent = 'Back to the title screen?';
    dom.overlayBody.textContent =
      `This ends the run. All ${engine.unlocked} unlocked levels go back to just the first, ` +
      'and the next run deals fresh boards.';
    dom.levelGrid.classList.add('hidden');
    dom.overlayButtons.replaceChildren(
      button('Keep playing', actions.onCancel, true),
      button('End the run', actions.onConfirm, false),
    );
    dom.overlay.classList.remove('hidden');
  }

  /**
   * Writes a line into the item feed, newest at the bottom.
   *
   * `what` is picked out from the words around it, so a glance finds the item
   * rather than the sentence. Old lines are dropped rather than kept forever:
   * this is a feed, and only the recent end of it is ever read.
   */
  logItem(said, what) {
    const { dom } = this;
    const line = document.createElement('li');
    const name = document.createElement('span');
    name.className = 'what';
    name.textContent = what;
    line.append(said, name);
    dom.feed.append(line);

    while (dom.feed.children.length > FEED_LIMIT) {
      dom.feed.children[0].remove();
    }
    // Follow the newest line. Not smooth: several items can land in one frame
    // and an animated scroll would be chasing a target that keeps moving.
    dom.feed.scrollTop = dom.feed.scrollHeight;
  }

  /** Empties the feed, for a run that is starting over. */
  clearFeed() {
    this.dom.feed.replaceChildren();
  }

  /** Says what an item was, for the feed. Unknown items still get a line. */
  describeItem(event) {
    if (event.color === ItemKind.UNLOCK) {
      const special = SPECIALS[event.value];
      return special ? ['Unlocked ', special.name] : null;
    }
    return null;
  }

  /** A load failure has to be visible; the board never appears otherwise. */
  showError(message) {
    const { dom } = this;
    dom.overlayTitle.textContent = 'Could not start';
    dom.overlayBody.textContent = message;
    dom.overlayButtons.replaceChildren();
    dom.levelGrid.classList.add('hidden');
    dom.overlay.classList.remove('hidden');
  }

  hideOverlay() {
    this.dom.overlay.classList.add('hidden');
  }

  get overlayVisible() {
    return !this.dom.overlay.classList.contains('hidden');
  }
}

function button(label, onClick, primary) {
  const element = document.createElement('button');
  element.type = 'button';
  element.textContent = label;
  if (primary) {
    element.classList.add('primary');
  }
  element.addEventListener('click', onClick);
  return element;
}

function describe(objective) {
  switch (objective.kind) {
    case ObjectiveKind.SCORE:
      return 'Score';
    case ObjectiveKind.COLOR:
      return `Clear ${PALETTE[objective.color % PALETTE.length].name}`;
    case ObjectiveKind.JELLY:
      return 'Clear jelly';
    case ObjectiveKind.BRICK:
      return 'Break the bricks';
    case ObjectiveKind.SEAL:
      return `${PALETTE[objective.color % PALETTE.length].name} seals`;
    default:
      return 'Goal';
  }
}

/** Says which goal ran out of road, which is what a losing player wants to know. */
function unmetSummary(engine) {
  const missed = engine.objectives().filter((objective) => objective.have < objective.need);
  if (missed.length === 0) {
    return 'So close.';
  }
  const parts = missed.map((objective) => `${describe(objective)} ${objective.have} / ${objective.need}`);
  return `Still needed: ${parts.join(', ')}.`;
}
