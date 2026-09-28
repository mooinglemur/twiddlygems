// The DOM around the board: level heading, score, moves, objective chips, and
// the overlay used for results and level selection.

import {
  Consumable,
  ItemClass,
  NO_LOCATION,
  ObjectiveKind,
  Special,
  Status,
  Tier,
} from './engine.js';
import {
  PALETTE,
  paintConsumableIcon,
  paintGemsIcon,
  paintGoalIcon,
  paintMovesIcon,
  paintSpecialIcon,
} from './render.js';

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
  [Special.LINE_H]: { name: 'Horizontal Line Clear', short: 'Horizontal', from: 'four in a column' },
  [Special.LINE_V]: { name: 'Vertical Line Clear', short: 'Vertical', from: 'four in a row' },
  [Special.CROSS]: { name: 'Cross Clear', short: 'Cross', from: 'an L or a T' },
  [Special.RAINBOW]: { name: 'Rainbow', short: 'Rainbow', from: 'five in a line' },
  [Special.ROCKET]: { name: 'Rocket', short: 'Rocket', from: 'a 2x2 square' },
};

/// The order the tracker shows the unlocks in: the three markings first, from
/// the plainest match to the fiddliest, then the two gems that replace the gem
/// entirely.
const TRACKED = [Special.LINE_H, Special.LINE_V, Special.CROSS, Special.RAINBOW, Special.ROCKET];

/// How big a tracker icon is drawn, in CSS pixels. Five of them and their
/// captions fit a phone's width; the stylesheet shrinks them below that.
const ICON_SIZE = 40;

/// How big the picture on a goal chip is. Smaller than a tracker icon and
/// smaller than a gem on the board: it is a label for a number, not a thing to
/// be looked at, and four of them have to fit across a phone.
const GOAL_ICON_SIZE = 26;

/// How big the picture in an inventory slot is. Larger than a goal chip's,
/// because this one is a thing to be looked at and chosen, and four of them
/// plus two buttons still have to fit across a phone.
const CONSUMABLE_ICON_SIZE = 30;

/// How big the two status marks on a level's row are. Small: they sit between
/// a name and three pips on a row that has to stay one line tall, and each is
/// a yes or no rather than something to study.
const STATUS_ICON_SIZE = 22;

/// What each thing the run can spend is called and what spending it does. The
/// second half is written for someone who has just been handed one: what it
/// asks of them, not what it is called again.
const SPENDABLE = {
  [Consumable.ROCKET]: { name: 'Rocket', does: 'strikes the cell you tap' },
  [Consumable.RAINBOW]: { name: 'Rainbow', does: 'takes every gem the color you tap' },
  [Consumable.CROSS_CLEAR]: { name: 'Cross Clear', does: 'blasts the row and column you tap' },
  [Consumable.ROCKET_CLUSTER]: {
    name: 'Rocket Cluster',
    does: 'a handful of rockets, each picking its own target',
  },
};

/// The class an item's name wears in the feed, by what the world makes of it.
/// The colors are Archipelago's own and live in the stylesheet: a player who
/// has seen a plum item name in any other game of theirs already knows this
/// one is worth having.
const WORTH_CLASS = {
  [ItemClass.FILLER]: 'filler',
  [ItemClass.USEFUL]: 'useful',
  [ItemClass.PROGRESSION]: 'progression',
  [ItemClass.TRAP]: 'trap',
};

/// The marks a level can be beaten to, in order, which is also the order the
/// three pips on a level row sit in. Each is a location an item is found at,
/// so a row of them is a row of checks.
///
/// `taken` and `missing` are what a pip says when you rest on it. A pip is a
/// nine pixel circle, which is enough to count and not enough to name, and the
/// difference between a hollow silver and a hollow gold is a color nobody
/// should have to learn from the picture alone.
const MARKS = [
  { tier: Tier.CLEAR, name: 'clear', taken: 'Cleared', missing: 'Uncleared' },
  {
    tier: Tier.SILVER,
    name: 'silver',
    taken: 'Cleared at Silver',
    missing: 'Silver not reached',
  },
  { tier: Tier.GOLD, name: 'gold', taken: 'Cleared at Gold', missing: 'Gold not reached' },
];

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
    /// The best score the open popover was filled with, so it is only rebuilt
    /// when that number moves. -1 because a level never beaten has 0.
    this.shownBest = -1;
    this.consumableViews = [];
    /// Which thing the run is carrying is waiting for a cell, or null.
    ///
    /// Held here rather than in the engine because nothing has happened yet:
    /// arming one is a state of the screen, and the engine is only told when
    /// a cell is actually tapped.
    this.armed = null;
    this.shownArmed = undefined;
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
    // A new level has its own marks, so anything the last one was showing goes.
    this.showScoreMarks(false);
    this.showTier();

    dom.objectives.replaceChildren();
    this.objectiveViews = engine.objectives().map((objective) => {
      const item = document.createElement('li');
      item.className = 'objective';
      // The chip carries the words, because the art is a picture of the thing
      // and the number beside it is what is left to do. Set once: neither the
      // goal nor what it is asking for changes while the level is being
      // played, only how far along it is.
      item.title = describe(objective);

      const count = document.createElement('span');
      count.className = 'objective-count';

      const icon = document.createElement('canvas');
      icon.className = 'objective-art';
      paintGoalIcon(icon, objective, GOAL_ICON_SIZE);

      item.append(icon, count);
      dom.objectives.append(item);

      // A score goal is a number to reach, and the score itself is already on
      // screen a few inches away, so this one says the target once and then
      // never changes. `target` is what says so: everything else counts down.
      const target = objective.kind === ObjectiveKind.SCORE;
      if (target) {
        count.textContent = objective.need.toLocaleString();
        item.setAttribute('aria-label', `${describe(objective)}: ${count.textContent}`);
      } else {
        // Room for the widest number it will ever hold, which is the one it
        // starts at, because these only count down. Without it a chip narrows
        // as its count loses a digit, and every chip to its right slides along
        // to take up the slack. `ch` is a digit's own width, which is a width
        // at all because the stylesheet sets tabular numerals.
        //
        // Digits and not characters, which is safe here and would not be a few
        // lines up: past a thousand the number on screen carries a comma, and
        // a comma is not a digit wide. The only goals with numbers that size
        // are scores, and a score never reaches this branch, because a score
        // is a target that is written once rather than a count coming down.
        count.style.minWidth = `${String(objective.need).length}ch`;
      }
      return { item, count, icon, target, objective };
    });
  }

  /**
   * The goals that clearing something can be seen to feed, for the renderer to
   * send motes to.
   *
   * The score is left out: everything on the board feeds it, so a mote going
   * there would say nothing, and the score is not in this row anyway.
   */
  goals() {
    return this.objectiveViews
      .map((view, at) => ({ view, at }))
      .filter(({ view }) => !view.target)
      .map(({ view, at }) => ({
        kind: view.objective.kind,
        color: view.objective.color,
        // Where the engine lists this one, which is not where the renderer
        // does: the score is a chip but not a destination, so the two lists
        // stop agreeing the moment a level has both.
        at,
        el: view.item,
        icon: view.icon,
      }));
  }

  /**
   * The slots along the bottom bar, built once for the whole run.
   *
   * Every kind gets one from the start, whether or not the run has any, the
   * same way the tracker shows the unlocks it is still waiting on. The empty
   * slots are half the information, and a slot that appeared when the first
   * one arrived would shove the rest along under a thumb already on its way
   * down.
   */
  buildInventory(onPick) {
    const { dom } = this;
    dom.inventory.replaceChildren();
    this.consumableViews = Object.values(Consumable).map((kind) => {
      const item = document.createElement('li');

      const slot = document.createElement('button');
      slot.type = 'button';
      slot.className = 'consumable';

      const art = document.createElement('canvas');
      art.className = 'consumable-art';
      paintConsumableIcon(art, kind, CONSUMABLE_ICON_SIZE);

      // How many are held, in a circle over the corner of the art. Hidden
      // while there are none: the dimmed slot already says so, and a badge
      // reading zero says it twice.
      const count = document.createElement('span');
      count.className = 'consumable-count';
      count.hidden = true;

      slot.append(art, count);
      slot.addEventListener('click', () => onPick(kind));
      item.append(slot);
      dom.inventory.append(item);
      // -1 rather than 0, so the first refresh writes the slot even when the
      // run is carrying nothing.
      return { kind, slot, count, held: -1 };
    });
    this.shownArmed = undefined;
    this.updateInventory();
  }

  /** The slot the armed item came out of, or null. */
  armedSlot() {
    return this.consumableViews.find((view) => view.kind === this.armed)?.slot ?? null;
  }

  /**
   * Arms one, or puts away the one that was armed.
   *
   * Tapping the armed item again is a way out of having armed it, which is
   * the same thing tapping anywhere off the board does. Answers what is armed
   * now, which is null for either of those.
   */
  arm(kind) {
    this.armed = this.armed === kind ? null : kind;
    return this.armed;
  }

  disarm() {
    this.armed = null;
  }

  /** Per-frame refresh of the slots; touches the DOM only where something moved. */
  updateInventory() {
    for (const view of this.consumableViews) {
      const held = this.engine.consumables(view.kind);
      if (held !== view.held) {
        view.held = held;
        const { name, does } = SPENDABLE[view.kind];
        view.count.textContent = String(held);
        view.count.hidden = held === 0;
        view.slot.classList.toggle('empty', held === 0);
        // Not just dimmed: an empty slot is nothing to tap, and a disabled
        // button says so to a screen reader and to a thumb alike.
        view.slot.disabled = held === 0;
        view.slot.title = held === 0 ? `${name}: none left` : `${name}: ${does}`;
        view.slot.setAttribute(
          'aria-label',
          held === 0 ? `${name}, none left` : `${name}, ${held} left: ${does}`,
        );
      }
      if (this.armed !== this.shownArmed) {
        view.slot.classList.toggle('armed', this.armed === view.kind);
        view.slot.setAttribute('aria-pressed', String(this.armed === view.kind));
      }
    }
    this.shownArmed = this.armed;
  }

  /** Colors the score by how well the level stands. */
  showTier() {
    const { dom } = this;
    const tier = this.tier();
    if (tier === this.shownTier) {
      return;
    }
    this.shownTier = tier;

    dom.score.classList.remove(...TIER_CLASSES);
    if (TIER_CLASS[tier]) {
      dom.score.classList.add(TIER_CLASS[tier]);
    }
    // The popover names which marks are behind, so it goes stale the moment
    // one of them is passed.
    if (this.marksVisible) {
      this.fillScoreMarks();
    }
  }

  /**
   * What this level can be beaten to, behind a tap of the score.
   *
   * Beside the score it was two numbers nobody reads most of the time, sat in
   * the one corner where the number that is being watched lives. Behind a tap
   * it is there for whoever wants it and out of the way of everyone else.
   */
  fillScoreMarks() {
    const { engine, dom } = this;
    const tier = this.tier();
    this.shownBest = engine.levelBestScore(engine.levelIndex);
    const { silver, gold } = engine.tiers;
    const rows = [
      { at: silver, name: 'Silver', tier: Tier.SILVER },
      { at: gold, name: 'Gold', tier: Tier.GOLD },
    ]
      .filter((mark) => mark.at > 0)
      .map((mark) => {
        const row = document.createElement('div');
        const taken = tier >= mark.tier;
        row.className = 'score-mark';
        row.classList.add(TIER_CLASS[mark.tier]);
        if (taken) {
          row.classList.add('taken');
        }

        const name = document.createElement('span');
        name.className = 'name';
        // The check is in the text rather than in the stylesheet, so a screen
        // reader is told which of the two are behind along with everyone else.
        name.textContent = taken ? `${mark.name} ✓` : mark.name;

        const at = document.createElement('span');
        at.className = 'at';
        at.textContent = mark.at.toLocaleString();

        row.append(name, at);
        return row;
      });

    if (rows.length === 0) {
      const none = document.createElement('div');
      none.className = 'score-mark';
      none.textContent = 'This level has no marks.';
      rows.push(none);
    }

    // What this run has actually beaten the level with, under the two numbers
    // it is being measured against. Only ever a score from an attempt that
    // won: a level is beaten from the moment it is cleared, and an attempt
    // that ran out of moves is not one of them, however well it was scoring.
    const best = this.shownBest;
    const yours = document.createElement('div');
    yours.className = 'score-mark';
    yours.classList.add('best');
    const label = document.createElement('span');
    label.className = 'name';
    label.textContent = 'Your best';
    const at = document.createElement('span');
    at.className = 'at';
    at.textContent = best > 0 ? best.toLocaleString() : 'not yet';
    yours.append(label, at);
    rows.push(yours);

    dom.scoreMarks.replaceChildren(...rows);
  }

  get marksVisible() {
    return !this.dom.scoreMarks.classList.contains('hidden');
  }

  showScoreMarks(on) {
    const { dom } = this;
    if (on) {
      this.fillScoreMarks();
    }
    dom.scoreMarks.classList.toggle('hidden', !on);
    dom.scoreBox.setAttribute('aria-expanded', String(on));
  }

  toggleScoreMarks() {
    this.showScoreMarks(!this.marksVisible);
  }

  /**
   * Per-frame refresh; touches the DOM only where something moved.
   *
   * `renderer` is asked what is still in the air, so a goal's number falls
   * because the motes reached it rather than a second before they set off.
   */
  update(renderer = null) {
    const { engine, dom } = this;
    this.showTier();
    this.updateInventory();
    // The best score climbs through the flourish the same way the live one
    // does, so a popover left open while a level is being beaten has to keep
    // up. Only when the number actually moves: this runs every frame.
    if (this.marksVisible && engine.levelBestScore(engine.levelIndex) !== this.shownBest) {
      this.fillScoreMarks();
    }

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
      // How much is left, not how far along: the number a player is counting
      // down is the one they are playing toward, and the total was never
      // theirs to do anything about.
      //
      // Plus whatever the motes have not delivered yet, so that the number
      // going down is something the motes are seen to do. Rounded up, so a
      // cell still has its last mote to land before its one comes off.
      //
      // The score chip is a target rather than a count, and says the same
      // thing all level, so it only takes the met check below.
      if (view.target) {
        view.item.classList.toggle('met', objective.have >= objective.need);
        continue;
      }
      const flying = renderer ? renderer.unitsInFlight(i) : 0;
      const left = Math.max(0, Math.ceil(objective.need - objective.have + flying));
      const said = left.toLocaleString();
      if (view.count.textContent !== said) {
        view.count.textContent = said;
        view.item.setAttribute('aria-label', `${describe(objective)}: ${said} left`);
      }
      // Green when the number says so, not a second before: the last mote
      // landing is what finishes the goal on screen, so it is what colors it.
      view.item.classList.toggle('met', left === 0);
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
    // A run opening the ladder by item can beat a level and have nowhere to
    // go: the next one is shut until a Progressive Level Unlock turns up. The
    // ladder opening by clearing never sees this, because clearing this level
    // is what opened the next one.
    const nextOpen = engine.levelIndex + 1 < engine.unlocked;

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
      // Said rather than left to be worked out from a missing button: a level
      // beaten with nothing to move on to is a state worth explaining, and the
      // thing to do about it is play something else and wait.
      if (!lastLevel && !nextOpen) {
        lines.push('The next level is shut until a Progressive Level Unlock turns up.');
      }
    } else {
      lines.push(unmetSummary(engine));
    }
    dom.overlayBody.textContent = lines.join(' ');

    const buttons = [];
    if (won && !lastLevel && nextOpen) {
      buttons.push(button('Next level', actions.onNext, true));
    }
    buttons.push(button(won ? 'Play again' : 'Try again', actions.onRetry, !won));
    buttons.push(button('Levels', actions.onLevels, false));
    dom.overlayButtons.replaceChildren(...buttons);

    dom.tracker.classList.add('hidden');
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

  /**
   * The screen where a run is set up, built by walking the engine's table.
   *
   * Nothing here knows what the settings are. A range gets a pair of buttons
   * that stop at its ends and a choice gets one that cycles, and which it is
   * comes from the engine, along with the label and the sentence underneath.
   * A setting added to the engine appears here, in the yaml and in the
   * apworld together, which is the whole point of the table being one table.
   *
   * Buttons rather than a slider or a select, because this is a phone screen
   * first: a tap target, no dragging, and no native picker to fight with.
   */
  showSetup() {
    const { engine, dom } = this;
    const rows = engine.options.map((option) => {
      const row = document.createElement('div');
      row.className = 'setup-option';

      const label = document.createElement('div');
      label.className = 'setup-label';
      label.textContent = option.label;

      const about = document.createElement('div');
      about.className = 'setup-about';
      about.textContent = option.about;

      const value = document.createElement('div');
      value.className = 'setup-value';
      value.setAttribute('aria-live', 'polite');

      const show = () => {
        const now = engine.optionValue(option.index);
        value.textContent =
          option.kind === 'choice'
            ? option.choices.find((choice) => choice.value === now)?.label ?? String(now)
            : String(now);
      };

      const nudge = (by) => {
        const next = engine.stepOption(option.index, by);
        engine.setOption(option.index, next);
        show();
      };

      const controls = document.createElement('div');
      controls.className = 'setup-controls';
      const back = button('−', () => nudge(-1), false);
      back.className = 'setup-step';
      back.setAttribute('aria-label', `${option.label}: previous`);
      const on = button('+', () => nudge(1), false);
      on.className = 'setup-step';
      on.setAttribute('aria-label', `${option.label}: next`);
      controls.append(back, value, on);

      show();
      row.append(label, controls, about);
      return row;
    });
    dom.setupOptions.replaceChildren(...rows);
    dom.setup.classList.remove('hidden');
  }

  hideSetup() {
    this.dom.setup.classList.add('hidden');
  }

  /**
   * The five unlocks along the top of the tracker, lit or grayed.
   *
   * Every one of them is shown from the start rather than appearing as it is
   * found, because the empty slots are half the information: what a run is
   * still waiting on is exactly as worth knowing as what it holds.
   */
  showItems() {
    const { engine, dom } = this;
    const held = engine.unlockedSpecials;
    const icons = TRACKED.map((code) => {
      const { name, short, from } = SPECIALS[code];
      const found = held.has(code);

      const item = document.createElement('div');
      item.className = 'tracked';
      if (found) {
        item.classList.add('found');
      }
      item.setAttribute('role', 'listitem');
      // The whole slot carries the words, because the art is a canvas and the
      // caption is only a nickname for it. Found, it says how to make one,
      // which is the thing a player who has just been handed it needs.
      item.setAttribute('aria-label', found ? `${name}, found: ${from}` : `${name}, not found`);
      item.title = found ? `${name}: ${from}` : `${name}: not found`;

      const art = document.createElement('canvas');
      art.className = 'tracked-art';
      paintSpecialIcon(art, code, ICON_SIZE);

      const caption = document.createElement('span');
      caption.className = 'tracked-name';
      caption.textContent = short;

      item.append(art, caption);
      return item;
    });
    dom.trackerItems.replaceChildren(...icons);
  }

  /**
   * The level picker, which is also the tracker: the items above, then one row
   * per level.
   *
   * A row rather than a chip in a grid, because a level's state is not one
   * thing. Cleared, silver and gold are three separate locations, and a chip
   * only had room to show the best of them, so a level cleared twice over and
   * a level whose gold is still out there looked the same. Three pips on a row
   * say which of the three have been taken, and the rows scroll, which a
   * fifty level ladder is going to need.
   */
  showLevels(actions) {
    const { engine, dom } = this;
    dom.overlayTitle.textContent = 'Levels';
    dom.overlayBody.textContent = `${engine.unlocked} of ${engine.levelCount} unlocked.`;
    this.showItems();

    const focused = this.focusedLevel();
    const names = engine.levelNames();
    const rows = [];
    for (let i = 0; i < engine.levelCount; i += 1) {
      const row = document.createElement('button');
      row.type = 'button';
      row.className = 'level-row';
      row.setAttribute('role', 'listitem');
      if (i === focused) {
        row.classList.add('current');
      }
      const unlocked = i < engine.unlocked;
      row.disabled = !unlocked;
      const best = engine.levelBest(i);
      // How well it has been beaten, as a wash across the row, so the list can
      // be scanned for where the gold is without reading any of it.
      const bestClass = TIER_CLASS[best];
      if (bestClass) {
        row.classList.add(bestClass);
      }

      const number = document.createElement('span');
      number.className = 'n';
      number.textContent = String(i + 1);

      const name = unlocked ? (names[i] ?? '') : 'Locked';
      const title = document.createElement('span');
      title.className = 't';
      title.textContent = name;

      // What the level still has in it, between its name and how well it has
      // been beaten. This is what makes the picker a tracker rather than a
      // menu: a run comes here to decide where to go next, and "that one still
      // has a gem in it" is the reason to go back to a level that is already
      // gold.
      const status = document.createElement('span');
      status.className = 'level-status';
      // Drawn, not read, like the pips. The row's own label says it in words.
      status.setAttribute('aria-hidden', 'true');
      const standing = [];
      for (const mark of [
        // "AP gems" rather than the whole word: it is what they are called
        // out loud, and it has to fit on a row beside a level's name.
        { ...engine.levelGems(i), paint: paintGemsIcon, of: 'AP gems' },
        { ...engine.levelMoves(i), paint: paintMovesIcon, of: 'moves upgrades' },
      ]) {
        // A run set up without any of something has nothing to track there,
        // and a mark that can never fill is worse than no mark.
        if (mark.total === 0) {
          continue;
        }
        const words = `${mark.found} of ${mark.total} ${mark.of}`;
        const art = document.createElement('canvas');
        art.className = 'status-art';
        art.title = words;
        mark.paint(art, STATUS_ICON_SIZE, mark.found, mark.total);
        status.append(art);
        standing.push(words);
      }

      const marks = document.createElement('span');
      marks.className = 'marks';
      // Drawn, not read: a screen reader gets the row's own label instead,
      // which says the same thing in words.
      marks.setAttribute('aria-hidden', 'true');
      for (const mark of MARKS) {
        const pip = document.createElement('span');
        pip.className = `pip ${TIER_CLASS[mark.tier]}`;
        const done = best >= mark.tier;
        if (done) {
          pip.classList.add('taken');
        }
        pip.title = done ? mark.taken : mark.missing;
        marks.append(pip);
      }

      const taken = MARKS.filter((mark) => best >= mark.tier).map((mark) => mark.name);
      row.setAttribute(
        'aria-label',
        [`Level ${i + 1}`, name, ...taken, ...standing].join(', '),
      );

      row.append(number, title, status, marks);
      row.addEventListener('click', () => actions.onPick(i));
      rows.push(row);
    }

    dom.levelList.replaceChildren(...rows);
    dom.tracker.classList.remove('hidden');
    dom.overlayButtons.replaceChildren(
      button('Close', actions.onClose, true),
      button('Quit game', actions.onQuit, false),
    );
    dom.overlay.classList.remove('hidden');
    // The level being played is somewhere down a list that scrolls, and on a
    // long ladder it is usually off the bottom of it.
    const current = dom.levelList.children[focused];
    current?.scrollIntoView?.({ block: 'center' });
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
    dom.tracker.classList.add('hidden');
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
  logItem({ said, what, where, worth }) {
    const { dom } = this;
    const line = document.createElement('li');
    const name = document.createElement('span');
    // Colored by what the world makes of it, which is the one thing about an
    // item a player wants to know before they have finished reading its name.
    // An item that says nothing about itself keeps the plain color rather than
    // borrowing one of the four.
    name.className = WORTH_CLASS[worth] ? `what ${WORTH_CLASS[worth]}` : 'what';
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
    return {
      said: where ? 'Found ' : 'Received ',
      what: name,
      where,
      worth: engine.itemClass(event.value),
    };
  }

  /** A load failure has to be visible; the board never appears otherwise. */
  showError(message) {
    const { dom } = this;
    dom.overlayTitle.textContent = 'Could not start';
    dom.overlayBody.textContent = message;
    dom.overlayButtons.replaceChildren();
    dom.tracker.classList.add('hidden');
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
