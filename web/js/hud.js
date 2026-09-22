// The DOM around the board: level heading, score, moves, objective chips, and
// the overlay used for results and level selection.

import { NO_LOCATION, ObjectiveKind, Special, Status, Tier } from './engine.js';
import { PALETTE } from './render.js';

/// How many lines the feed keeps. Well past what fits, so scrolling back a
/// little works, and far short of a session's worth.
const FEED_LIMIT = 40;

/// The class marking each tier, matched in the stylesheet. Indexed by `Tier`.
const TIER_CLASS = [null, 'tier-clear', 'tier-silver', 'tier-gold'];
const TIER_CLASSES = TIER_CLASS.filter(Boolean);

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
    /// Whether this level's goals have been met, which is not the same as the
    /// level being over: the flourish runs in between, and the score is still
    /// climbing through it. Set from the `CLEARED` event.
    this.cleared = false;
    this.shownTier = -1;
  }

  /**
   * How well this level stands, as a `Tier`.
   *
   * The better of what the run managed here before and what this attempt has
   * reached, so it only ever moves up: a gold level replayed for a worse score
   * should not look as though the gold were taken away. This attempt counts
   * only once the goals are met, because until then there may be no clear at
   * all.
   */
  tier() {
    const { engine } = this;
    const best = engine.levelBest(engine.levelIndex);
    if (!this.cleared) {
      return best;
    }
    const { silver, gold } = engine.tiers;
    const score = engine.score;
    const now =
      gold > 0 && score >= gold
        ? Tier.GOLD
        : silver > 0 && score >= silver
          ? Tier.SILVER
          : Tier.CLEAR;
    return Math.max(best, now);
  }

  /** Rebuilds everything that only changes when the level does. */
  rebuild() {
    const { engine, dom } = this;
    dom.levelNumber.textContent = `Level ${engine.levelIndex + 1}`;
    dom.levelName.textContent = engine.levelName;
    this.shownScore = engine.score;
    this.lastMoves = -1;
    this.cleared = false;
    this.shownTier = -1;
    dom.score.textContent = Math.round(this.shownScore).toLocaleString();
    this.showTier();

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

  /**
   * Colors the score by how well the level stands, and names the next mark up
   * beside it.
   *
   * The mark is the nearest one still out of reach, and once both are behind
   * there is nothing left to aim at, so it says nothing at all rather than
   * repeating a number already beaten.
   */
  showTier() {
    const { engine, dom } = this;
    const tier = this.tier();
    if (tier === this.shownTier) {
      return;
    }
    this.shownTier = tier;

    dom.score.classList.remove(...TIER_CLASSES);
    if (TIER_CLASS[tier]) {
      dom.score.classList.add(TIER_CLASS[tier]);
    }

    const { silver, gold } = engine.tiers;
    const next =
      tier < Tier.SILVER && silver > 0
        ? { at: silver, mark: 'silver', cls: 'tier-silver' }
        : tier < Tier.GOLD && gold > 0
          ? { at: gold, mark: 'gold', cls: 'tier-gold' }
          : null;
    dom.scoreTarget.classList.remove(...TIER_CLASSES);
    dom.scoreTarget.textContent = next ? `${next.mark} ${next.at.toLocaleString()}` : '';
    if (next) {
      dom.scoreTarget.classList.add(next.cls);
    }
  }

  /** Per-frame refresh; touches the DOM only where something moved. */
  update() {
    const { engine, dom } = this;
    this.showTier();

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

  /**
   * The end-of-level panel.
   *
   * `found` is whatever the clear turned up, described, or null. It comes from
   * the caller rather than from the engine because the event stream is the one
   * place items are announced: the panel and the feed should never be able to
   * disagree about what was found.
   */
  showResult(status, actions, found) {
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
      const score = Math.round(engine.score);
      // What the level was worth beating well, said before the number, since
      // the tier is the achievement and the score is only the evidence.
      const { silver, gold } = engine.tiers;
      const tier = gold > 0 && score >= gold ? 'Gold' : silver > 0 && score >= silver ? 'Silver' : null;
      lines.push(
        tier
          ? `${tier}: ${score.toLocaleString()} points on ${engine.levelName}.`
          : `${score.toLocaleString()} points on ${engine.levelName}.`,
      );
      // How far off the next mark up is, which is the whole reason to play a
      // level again once it is cleared. The nearer one first: someone short of
      // silver wants to hear about silver, not gold.
      const next = [
        { name: 'silver', at: silver },
        { name: 'gold', at: gold },
      ].find((mark) => mark.at > 0 && score < mark.at);
      if (next) {
        lines.push(`${(next.at - score).toLocaleString()} more for ${next.name}.`);
      }
      if (found) {
        lines.push(found.where ? `${found.said}${found.what} (${found.where}).` : `${found.said}${found.what}.`);
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
      // How well it has been beaten, so a grid of them can be scanned for
      // where the gold is and what is still only cleared.
      const best = TIER_CLASS[engine.levelBest(i)];
      if (best) {
        chip.classList.add(best);
      }

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
  logItem({ said, what, where }) {
    const { dom } = this;
    const line = document.createElement('li');
    const name = document.createElement('span');
    name.className = 'what';
    name.textContent = what;
    line.append(said, name);
    if (where) {
      const place = document.createElement('span');
      place.className = 'where';
      place.textContent = ` (${where})`;
      line.append(place);
    }
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

  /**
   * Turns an item event into the words for it: what happened, what the item
   * is, and where it came from.
   *
   * Item and location are named separately because they are separate things
   * to a multiworld: the same unlock can turn up at any location, and the same
   * location can be holding anything. Anything unrecognized gets no line
   * rather than a wrong one.
   */
  describeItem(event) {
    const { engine } = this;
    const name = engine.itemNames[event.value];
    if (!name) {
      return null;
    }
    // The location is two bytes of the event, or 65535 for an item that came
    // from nowhere here, which a multiworld handing one over looks like.
    const at = event.color | (event.special << 8);
    const where = at === NO_LOCATION ? null : (engine.locationNames[at] ?? null);
    // "Found" for something this run turned up itself, the way Archipelago
    // distinguishes it from an item another world sent over.
    return { said: where ? 'Found ' : 'Received ', what: name, where };
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
