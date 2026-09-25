// Canvas rendering.
//
// The engine hands over a color, a special, an offset and a scale per cell; all
// this file decides is what that looks like. Gems differ by shape as well as
// hue so the board stays readable without relying on color alone.

import { EMPTY_CELL, EventKind, Flag, ObjectiveKind, Phase, Special } from './engine.js';

/// The game's gem set, indexed by the color numbers the engine deals. A level
/// usually takes the first few, but it may name any set instead, so this is a
/// lookup rather than a prefix: a four color level can be ruby, amber,
/// sapphire and emerald, and `engine.colors` is how many it deals rather than
/// how far along this list it reaches.
///
/// Every shape is drawn with its corners rounded off. Nothing here comes to a
/// point: a board of sharp silhouettes reads as spiky rather than as gems, and
/// the rounding is what keeps six different outlines looking like one set.
export const PALETTE = [
  { name: 'Ruby', fill: '#e5484d', edge: '#7e1f25', shape: 'triangle' },
  { name: 'Sapphire', fill: '#3f8cff', edge: '#1b3f8c', shape: 'drop' },
  { name: 'Emerald', fill: '#3fbf6f', edge: '#166534', shape: 'star' },
  { name: 'Topaz', fill: '#f5d742', edge: '#87720b', shape: 'circle' },
  { name: 'Amethyst', fill: '#a56bff', edge: '#4c2a8a', shape: 'diamond' },
  { name: 'Amber', fill: '#f0863c', edge: '#8f430f', shape: 'tomb' },
  // Past the six in play. They keep shapes of their own so that raising
  // `colors` deals something distinguishable rather than a repeat.
  { name: 'Rose', fill: '#ff8ac0', edge: '#a13a70', shape: 'pentagon' },
  { name: 'Aqua', fill: '#2dd4c8', edge: '#0e6f68', shape: 'hex' },
];

const TAU = Math.PI * 2;

/// Enough for a rainbow taking a whole color; past this the oldest simply stop
/// being replaced, which is cheaper than dropping frames on a phone.
const MAX_PARTICLES = 600;
/// Past 2x the extra pixels buy nothing you can see, and cost plenty: the
/// backing store grows with the square of this, and a phone's canvas is
/// fill-rate bound long before it is logic bound.
const MAX_DPR = 2;

/// How a rocket comes round to a new heading: an e-folding time, and a
/// ceiling on how far it may turn in one frame.
///
/// The ease alone settles small corrections nicely, which is most of a flight,
/// where it is following the bend of its own arc. Its first step is its
/// biggest, though, and a rocket launched back the way it is pointing has half
/// a turn to make: unbounded that lands most of the turn in one frame, which
/// is the snap this is here to avoid. The cap is about half a turn in a third
/// of a second, well inside the shortest flight.
const ROCKET_TURN_MS = 70;
const ROCKET_TURN_PER_MS = 0.0095;

/// A rocket wears no gem's colors, because it belongs to no color.
const ROCKET_BODY = '#eceaf6';
const ROCKET_EDGE = '#39325c';
const ROCKET_FIN = '#e5484d';
const ROCKET_PORT = '#8fd0ff';
const SHARDS_PER_GEM = 9;
const PUFFS_PER_GEM = 4;
/// What a cell throws loose when most of it is being sent to a goal instead.
/// The two together come to about what an ordinary clear throws, so a gem that
/// counts for something does not turn into a bigger explosion than one that
/// does not.
const SHARDS_PER_TRIBUTED_GEM = 4;
/// Motes ringing a gem as it becomes a special. Fewer than a burst's shards:
/// a couple of dozen of these can be on screen at once during the run down at
/// the end of a level.
const MOTES_PER_SPARKLE = 7;
/// A rocket strike is the loudest thing on the board, so it throws far more.
const SHARDS_PER_IMPACT = 28;
const PUFFS_PER_IMPACT = 12;

/// Brick, which is masonry rather than ground or gem and is colored like
/// neither: warm where the board is cold, so it reads as a thing put there
/// rather than as part of the frame.
const BRICK_FACE = '#9c5240';
const BRICK_CRACKED_FACE = '#7c4438';
const BRICK_MORTAR = 'rgba(32,18,14,0.55)';
const BRICK_DEBRIS = '#b96a4f';

/// How long a pop-over line of text lives, and the share of that spent fading
/// in and fading out. It outlasts the shuffle it announces, because a message
/// that has gone by the time the board settles is one nobody read.
const TOAST_MS = 1800;
const TOAST_IN = 0.18;
const TOAST_OUT = 0.4;

/// How many motes one cleared cell sends to each goal it counts toward, how
/// long the flight takes, and the ceiling on how many may be in the air. A
/// rainbow taking a whole color clears a dozen cells at once, and every one of
/// them may be feeding two goals, so the ceiling is what keeps a board-wide
/// clear from drawing two thousand of them.
const TRIBUTES_PER_GOAL = 9;
const TRIBUTE_MS = 1_240;
const MAX_TRIBUTES = 480;
/// The longest a mote can take to get where it is going: the latest one off
/// the cell, on the longest flight. What a beaten level waits out before it
/// starts spending its leftover moves, so the goals are seen reaching their
/// totals rather than being talked over. The engine is told this number
/// because the engine holds the beat and this file owns the animation.
export const GOAL_EFFECT_MS = 220 + TRIBUTE_MS * 1.15;
/// How long the glow around a goal lasts after something lands in it, and how
/// far past the chip it reaches.
const GOAL_FLASH_MS = 320;
const GOAL_FLASH_SPREAD = 5;

export class Renderer {
  /**
   * `fx` is the layer over the whole page, or null.
   *
   * A second canvas because the board's own ends at the board: what a cleared
   * gem counts toward is a chip above it, outside the board entirely, and a
   * mote sent there has to be drawn somewhere that reaches both.
   */
  constructor(canvas, engine, fx = null) {
    this.canvas = canvas;
    this.engine = engine;
    this.ctx = canvas.getContext('2d', { alpha: true });
    this.fx = fx;
    this.fxCtx = fx ? fx.getContext('2d', { alpha: true }) : null;
    /// Whether anything is on the fx layer, so it is cleared once when the
    /// last mote lands rather than every idle frame.
    this.fxPainted = false;
    this.cell = 0;
    this.pad = 0;
    this.hint = null;
    /// Bursts waiting for their moment: a blast spreads outward, so each cell's
    /// debris is held back until the clear actually reaches it.
    this.pendingBursts = [];
    this.particles = [];
    /// Motes in flight from a cleared cell to the goal it counted toward, and
    /// the goals they are flying to. Set by `setGoals` when the HUD rebuilds.
    this.tributes = [];
    this.goals = [];
    this.goalFlash = [];
    /// Motes in the air per goal, which is what one cell's worth of counting
    /// looks like on its way over. See `unitsInFlight`.
    this.goalInFlight = [];
    /// The board as it stood when it was last drawn: the jelly under each
    /// cell, and which goals were already met.
    ///
    /// Both are read when events arrive, which is one tick after the engine
    /// has moved them on. A clear peels its cell and moves its counter in the
    /// same tick as it raises the event, so asking the engine then would say a
    /// gem sat on nothing and paid a goal that was already met, when in fact
    /// it peeled the jelly and was the clear that met the goal.
    this.jellySeen = null;
    /// How much each goal still had to do, and how much of that has been
    /// promised to it already this frame. See `goalsFor`.
    this.goalsLeft = [];
    this.owedThisFrame = [];
    /// Where each rocket in the air was a frame ago, keyed by cell, which is
    /// how a rocket on an arc knows which way it is pointing.
    this.rocketWas = null;
    /// A line of text swelling and fading over the board, or null.
    this.toast = null;
    this.lastFrame = null;
    this.layout();
    this.captureBoard();
  }

  /**
   * The goals motes fly to: `{ kind, color, el, icon }` per chip, in the order
   * the engine lists them, with the score left out because nothing converges
   * on a number that is already climbing on its own.
   */
  setGoals(goals) {
    this.goals = goals;
    this.goalFlash = goals.map(() => 0);
    this.goalsLeft = goals.map(() => 0);
    this.owedThisFrame = goals.map(() => 0);
    this.goalInFlight = goals.map(() => 0);
    this.tributes.length = 0;
  }

  /**
   * How much of what the engine has already counted has not visibly arrived
   * yet, in whatever the goal counts, by the engine's own objective index.
   *
   * This is what lets the number on a chip go down because the motes reached
   * it rather than a second before they set off. The engine moves its counter
   * the moment the gem goes; the chip shows that counter plus whatever is
   * still in the air, so it only falls as the air clears, and the arithmetic
   * comes right on its own once nothing is flying.
   */
  unitsInFlight(at) {
    for (let i = 0; i < this.goals.length; i += 1) {
      if (this.goals[i].at === at) {
        return this.goalInFlight[i] / TRIBUTES_PER_GOAL;
      }
    }
    return 0;
  }

  /// Queues debris for everything the engine just cleared. Each event carries
  /// the delay the engine assigned it, which is what makes a row clear ripple.
  addEvents(events, now) {
    for (const event of events) {
      if (event.kind === EventKind.CLEAR) {
        // Worked out now rather than when the burst fires, because a jelly
        // goal is decided by what was under the gem, and by the time the burst
        // is due the engine has already peeled it.
        const goals = this.goalsFor(event);
        this.owe(goals);
        this.pendingBursts.push({
          at: now + event.value,
          r: event.r,
          c: event.c,
          color: event.color,
          goals,
        });
      } else if (event.kind === EventKind.ROCKET_HIT) {
        // A strike on a brick carries no gem color, which is how this knows to
        // throw masonry rather than a cyan gem that was never there.
        this.pendingBursts.push({
          at: now,
          r: event.r,
          c: event.c,
          color: event.color,
          impact: true,
          tint: event.color === EMPTY_CELL ? BRICK_DEBRIS : null,
        });
      } else if (event.kind === EventKind.BRICK) {
        // Chips when it cracks, a proper shower when it goes. A seal throws
        // its own color, a plain brick throws masonry.
        const sealed = event.color !== EMPTY_CELL;
        const goals = this.goalsFor(event);
        this.owe(goals);
        this.pendingBursts.push({
          at: now,
          r: event.r,
          c: event.c,
          color: sealed ? event.color : 0,
          impact: event.value === 0,
          tint: sealed ? null : BRICK_DEBRIS,
          goals,
        });
      } else if (event.kind === EventKind.SHUFFLE) {
        // The board is about to rearrange itself. Without a word about it the
        // player looks away and looks back at a different board.
        this.toast = { text: 'No moves, shuffling', at: now };
      } else if (event.kind === EventKind.LOW_MOVES) {
        const left = event.value;
        this.toast = { text: `${left} move${left === 1 ? '' : 's'} left`, at: now };
      } else if (event.kind === EventKind.CLEARED) {
        this.toast = { text: 'Level cleared', at: now };
      } else if (event.kind === EventKind.SPECIAL_MADE || event.kind === EventKind.CASH_IN) {
        // A gem gaining something rather than losing it, so motes rather than
        // debris. A spend that placed nothing gets the same motes: the point
        // of the run down at the end of a level is watching it happen, and a
        // cell that flashes and stays a plain gem says the run had nothing to
        // give far better than a cell that does nothing at all.
        this.pendingBursts.push({
          at: now,
          r: event.r,
          c: event.c,
          color: event.color,
          sparkle: true,
        });
      }
    }
  }

  /**
   * Which goals a cleared cell counted toward, as indices into `this.goals`.
   *
   * The same arithmetic the engine does, read from the other side: a gem pays
   * its color, a brick pays the brick count, a seal pays both that and its own
   * color, and any of them may pay a jelly goal as well by being the gem that
   * peeled the last layer off its cell. A cell can feed two goals at once, and
   * saying so is the point of the effect.
   *
   * **A goal takes only what it still has to take.** The engine stops counting
   * at the total, so clearing three of a color a goal wanted two more of
   * counts as two; a third mote sent anyway would be owed against a goal with
   * nothing left owing, and the chip would climb to three before falling,
   * which is the one direction it must never go. Counted against what was left
   * at the top of the frame, and against what this frame has promised out of
   * it already, so a clear that pays several cells at once stops at the right
   * one. A goal already met is simply the case where nothing is left.
   */
  goalsFor(event) {
    const hits = [];
    if (this.goals.length === 0) {
      return hits;
    }
    const cleared = event.kind === EventKind.CLEAR;
    // Only the layer that takes a cell out of the count: the goal counts cells
    // with jelly under them, so softening a double layer moves nothing.
    const lastLayer = cleared && this.jellyAt(event.r, event.c) === 1;
    // A brick that only cracked is still a brick, and still in the way.
    const broken = event.kind === EventKind.BRICK && event.value === 0;
    for (let i = 0; i < this.goals.length; i += 1) {
      if (this.goalsLeft[i] - this.owedThisFrame[i] < 1) {
        continue;
      }
      const goal = this.goals[i];
      const counts =
        goal.kind === ObjectiveKind.COLOR
          ? cleared && event.color === goal.color
          : goal.kind === ObjectiveKind.JELLY
            ? lastLayer
            : goal.kind === ObjectiveKind.BRICK
              ? broken
              : goal.kind === ObjectiveKind.SEAL && broken && event.color === goal.color;
      if (counts) {
        hits.push(i);
      }
    }
    return hits;
  }

  /**
   * Counts what a clear is about to deliver as already on its way.
   *
   * From the moment the burst is queued rather than the moment its motes
   * leave: a blast spreads outward, so a cell's burst is held back until the
   * clear reaches it, and in that gap the engine's counter has already moved.
   * A chip reading the counter alone would drop its number there and then
   * take it back when the motes finally set off.
   */
  owe(goals) {
    for (const goal of goals) {
      this.goalInFlight[goal] += TRIBUTES_PER_GOAL;
      this.owedThisFrame[goal] += 1;
    }
  }

  /// The jelly under a cell as the board last stood. See `jellySeen`.
  jellyAt(r, c) {
    const i = r * this.engine.cols + c;
    return this.jellySeen && i >= 0 && i < this.jellySeen.length ? this.jellySeen[i] : 0;
  }

  /// Takes the record the next clear will be read against. See `jellySeen`.
  captureBoard() {
    const { cells } = this.engine.snapshot();
    const count = cells.length / 4;
    if (!this.jellySeen || this.jellySeen.length !== count) {
      this.jellySeen = new Uint8Array(count);
    }
    for (let i = 0; i < count; i += 1) {
      this.jellySeen[i] = cells[i * 4 + 2];
    }

    if (this.goals.length === 0) {
      return;
    }
    const objectives = this.engine.objectives();
    for (let i = 0; i < this.goals.length; i += 1) {
      const objective = objectives[this.goals[i].at];
      this.goalsLeft[i] = objective === undefined ? 0 : Math.max(0, objective.need - objective.have);
      // A fresh frame, so nothing has been promised out of that yet.
      this.owedThisFrame[i] = 0;
    }
  }

  /// Drops everything in flight, for a restart or a level change.
  reset() {
    this.pendingBursts.length = 0;
    this.particles.length = 0;
    this.tributes.length = 0;
    this.goalFlash = this.goals.map(() => 0);
    this.goalInFlight = this.goals.map(() => 0);
    this.rocketWas = null;
    this.toast = null;
    this.lastFrame = null;
    this.backdrop = null;
    this.dirty = true;
    this.captureBoard();
  }

  updateParticles(now) {
    const dt = this.lastFrame === null ? 16 : Math.min(now - this.lastFrame, 50);
    this.lastFrame = now;
    // Kept for anything drawn later in the frame that has to move by time
    // rather than by where the engine says a cell is; see the rockets.
    this.frameMs = dt;

    if (this.pendingBursts.length > 0) {
      const due = [];
      const waiting = [];
      for (const burst of this.pendingBursts) {
        (burst.at <= now ? due : waiting).push(burst);
      }
      this.pendingBursts = waiting;
      for (const burst of due) {
        if (burst.sparkle) {
          this.sparkle(burst);
        } else {
          this.tribute(burst);
          this.burst(burst);
        }
      }
    }

    this.updateTributes(dt);

    let live = 0;
    for (const particle of this.particles) {
      particle.age += dt;
      if (particle.age >= particle.life) {
        continue;
      }
      particle.x += particle.vx * dt;
      particle.y += particle.vy * dt;
      particle.vy += particle.gravity * dt;
      particle.vx *= particle.drag;
      this.particles[live] = particle;
      live += 1;
    }
    this.particles.length = live;
  }

  /// A gem turning into a special: a ring of bright motes drawn inward, and a
  /// halo. Nothing is being destroyed here, so it throws no debris and no
  /// smoke: it should read as the cell gaining something rather than losing
  /// it, which is the opposite of what `burst` says.
  sparkle({ r, c, color }) {
    if (this.particles.length > MAX_PARTICLES) {
      return;
    }
    const { cell, pad } = this;
    const x = pad + (c + 0.5) * cell;
    const y = pad + (r + 0.5) * cell;
    const fill = PALETTE[color % PALETTE.length].fill;

    this.particles.push({
      kind: 'ring',
      x,
      y,
      vx: 0,
      vy: 0,
      gravity: 0,
      drag: 1,
      size: cell * 0.16,
      color: '#fff4cc',
      age: 0,
      life: 420,
    });

    for (let i = 0; i < MOTES_PER_SPARKLE; i += 1) {
      const angle = (i / MOTES_PER_SPARKLE) * TAU + Math.random() * 0.5;
      // Slowly outward and rising, so the motes hang around the gem instead
      // of being thrown off it.
      const speed = cell * (0.0004 + Math.random() * 0.0008);
      this.particles.push({
        kind: 'shard',
        x: x + Math.cos(angle) * cell * 0.34,
        y: y + Math.sin(angle) * cell * 0.34,
        vx: Math.cos(angle) * speed,
        vy: Math.sin(angle) * speed - cell * 0.00022,
        gravity: -cell * 0.0000006,
        drag: 0.985,
        size: cell * (0.04 + Math.random() * 0.05),
        color: Math.random() < 0.6 ? '#fff4cc' : fill,
        age: 0,
        life: 420 + Math.random() * 320,
      });
    }
  }

  /**
   * The part of a cleared cell that goes to the goal it counted toward.
   *
   * Drawn on the fx layer rather than the board, because it leaves the board:
   * a count going up somewhere above the playfield is the one thing a clear
   * means that the board itself cannot show, and a mote that arcs up to the
   * chip says which clear moved which number without a word.
   */
  tribute({ r, c, color, goals, tint = null }) {
    if (!this.fx || !goals || goals.length === 0 || this.tributes.length >= MAX_TRIBUTES) {
      return;
    }
    const { cell } = this;
    const origin = this.boardPoint(r, c);
    const fill = tint ?? PALETTE[color % PALETTE.length].fill;
    for (const goal of goals) {
      for (let i = 0; i < TRIBUTES_PER_GOAL; i += 1) {
        // The control point of the arc, thrown off the cell in a random
        // direction: the motes scatter the way debris does before the goal
        // gathers them in, so the flight reads as the clear being collected
        // rather than as a line drawn between two points.
        const angle = Math.random() * TAU;
        const reach = cell * (0.35 + Math.random() * 0.8);
        this.tributes.push({
          goal,
          x0: origin.x,
          y0: origin.y,
          cx: origin.x + Math.cos(angle) * reach,
          cy: origin.y + Math.sin(angle) * reach,
          x: origin.x,
          y: origin.y,
          size: cell * (0.07 + Math.random() * 0.06),
          color: Math.random() < 0.4 ? '#fff4cc' : fill,
          // Negative, so they leave in a trickle rather than as one clump.
          age: -Math.random() * 220,
          life: TRIBUTE_MS * (0.85 + Math.random() * 0.3),
        });
      }
    }
  }

  /// A cell's middle in fx-layer space, which is the board's own space shifted
  /// by wherever the board sits on the page.
  boardPoint(r, c) {
    const board = this.canvas.getBoundingClientRect();
    const layer = this.fx.getBoundingClientRect();
    return {
      x: board.left - layer.left + this.pad + (c + 0.5) * this.cell,
      y: board.top - layer.top + this.pad + (r + 0.5) * this.cell,
    };
  }

  /// Where each goal's chip is, in fx-layer space: the icon to aim at, and the
  /// chip around it to light up when something lands.
  goalPoints() {
    const layer = this.fx.getBoundingClientRect();
    return this.goals.map((goal) => {
      const chip = goal.el.getBoundingClientRect();
      const mark = (goal.icon ?? goal.el).getBoundingClientRect();
      return {
        x: mark.left - layer.left + mark.width / 2,
        y: mark.top - layer.top + mark.height / 2,
        chip: {
          x: chip.left - layer.left,
          y: chip.top - layer.top,
          w: chip.width,
          h: chip.height,
        },
      };
    });
  }

  updateTributes(dt) {
    for (let i = 0; i < this.goalFlash.length; i += 1) {
      this.goalFlash[i] = Math.max(0, this.goalFlash[i] - dt / GOAL_FLASH_MS);
    }
    if (this.tributes.length === 0) {
      this.settleDebt();
      return;
    }

    const points = this.goalPoints();
    let live = 0;
    for (const mote of this.tributes) {
      mote.age += dt;
      if (mote.age >= mote.life) {
        // It landed, so the goal it landed in lights up and is owed that much
        // less of what is on its way.
        this.goalFlash[mote.goal] = 1;
        this.goalInFlight[mote.goal] = Math.max(0, this.goalInFlight[mote.goal] - 1);
        continue;
      }
      const target = points[mote.goal];
      if (target) {
        // Read every frame rather than pinned at launch, because the chips
        // move: the goals wrap to a second row the moment one of them gets a
        // wider number, and the board is re-laid-out under them.
        mote.tx = target.x;
        mote.ty = target.y;
      }
      this.tributes[live] = mote;
      live += 1;
    }
    this.tributes.length = live;
    if (live === 0) {
      this.settleDebt();
    }
  }

  /// With nothing in the air and nothing waiting to go, nothing is owed. Said
  /// outright rather than left to the arithmetic: a mote never launched (the
  /// ceiling was reached, or the level changed under it) would otherwise leave
  /// a chip reading one too many for the rest of the level.
  settleDebt() {
    if (this.pendingBursts.length === 0) {
      this.goalInFlight.fill(0);
    }
  }

  /// The layer over the page: motes on their way to a goal, and the goals
  /// lighting up as they arrive. Cleared once when the last one lands rather
  /// than wiped every idle frame.
  paintFx() {
    if (!this.fx) {
      return;
    }
    const lit = this.goalFlash.some((flash) => flash > 0.01);
    if (this.tributes.length === 0 && !lit) {
      if (this.fxPainted) {
        this.fxCtx.setTransform(this.fxDpr, 0, 0, this.fxDpr, 0, 0);
        this.fxCtx.clearRect(0, 0, this.fxWidth, this.fxHeight);
        this.fxPainted = false;
      }
      return;
    }

    const ctx = this.fxCtx;
    ctx.setTransform(this.fxDpr, 0, 0, this.fxDpr, 0, 0);
    ctx.clearRect(0, 0, this.fxWidth, this.fxHeight);
    this.fxPainted = true;

    if (lit) {
      const points = this.goalPoints();
      for (let i = 0; i < this.goalFlash.length; i += 1) {
        const flash = this.goalFlash[i];
        if (flash <= 0.01 || !points[i]) {
          continue;
        }
        // A halo that swells outward as it fades, drawn around the chip
        // itself so it reads as that goal taking something in.
        const { chip } = points[i];
        const spread = GOAL_FLASH_SPREAD * (1 - flash);
        ctx.globalAlpha = 0.75 * flash;
        ctx.strokeStyle = '#fff4cc';
        ctx.lineWidth = 2;
        roundRect(
          ctx,
          chip.x - spread,
          chip.y - spread,
          chip.w + spread * 2,
          chip.h + spread * 2,
          12 + spread,
        );
        ctx.stroke();
      }
    }

    for (const mote of this.tributes) {
      if (mote.age <= 0 || mote.tx === undefined) {
        continue;
      }
      const t = mote.age / mote.life;
      // Ease in: it hangs over the cell long enough to be seen leaving, then
      // runs into the goal.
      const e = t * t;
      const here = bezier(mote, e);
      // The trail is a slice of the path rather than a slice of the clock, and
      // the ease puts most of the path at the end, so the slice narrows as it
      // goes. Without that the last stretch draws as a matchstick. It is a
      // streak of speed, so it is cut to how fast they actually travel: half
      // of what it was when the flight took half as long.
      const behind = bezier(mote, Math.max(0, e - 0.038 * (1 - 0.6 * t)));
      const size = mote.size * (1 - 0.45 * e);

      ctx.globalAlpha = Math.min(1, 6 * t) * Math.min(1, 5 * (1 - t));
      ctx.strokeStyle = mote.color;
      ctx.lineCap = 'round';
      ctx.lineWidth = size * 1.1;
      ctx.beginPath();
      ctx.moveTo(behind.x, behind.y);
      ctx.lineTo(here.x, here.y);
      ctx.stroke();

      ctx.fillStyle = mote.color;
      ctx.beginPath();
      ctx.arc(here.x, here.y, size, 0, TAU);
      ctx.fill();
    }
    ctx.globalAlpha = 1;
  }

  /// One cell's worth of debris: shards of the gem, and a puff of smoke. A
  /// rocket strike throws the same thing much harder, with a blast ring.
  ///
  /// A cell that is feeding a goal throws less of it loose, because the rest
  /// of it left for the chip: see `tribute`.
  burst({ r, c, color, impact = false, tint = null, goals = null }) {
    if (this.particles.length > MAX_PARTICLES) {
      return;
    }
    const { cell, pad } = this;
    const x = pad + (c + 0.5) * cell;
    const y = pad + (r + 0.5) * cell;
    // A tint for debris that is not a gem and so has no palette entry of its
    // own, which so far means brick.
    const gem = { fill: tint ?? PALETTE[color % PALETTE.length].fill };
    const tributed = this.fx !== null && goals !== null && goals.length > 0;
    const shards = impact
      ? SHARDS_PER_IMPACT
      : tributed
        ? SHARDS_PER_TRIBUTED_GEM
        : SHARDS_PER_GEM;
    const puffs = impact ? PUFFS_PER_IMPACT : PUFFS_PER_GEM;
    const force = impact ? 2.6 : 1;

    if (impact) {
      this.particles.push({
        kind: 'ring',
        x,
        y,
        vx: 0,
        vy: 0,
        gravity: 0,
        drag: 1,
        size: cell * 0.3,
        color: '#ffd9a0',
        age: 0,
        life: 340,
      });
    }

    for (let i = 0; i < shards; i += 1) {
      const angle = (i / shards) * TAU + Math.random() * 0.6;
      const speed = cell * (0.0016 + Math.random() * 0.0042) * force;
      this.particles.push({
        kind: 'shard',
        x,
        y,
        vx: Math.cos(angle) * speed,
        vy: Math.sin(angle) * speed,
        gravity: cell * 0.0000055,
        drag: 0.995,
        size: cell * (0.06 + Math.random() * 0.09) * (impact ? 1.5 : 1),
        color: Math.random() < (impact ? 0.45 : 0.25) ? '#ffe9b0' : gem.fill,
        age: 0,
        life: (360 + Math.random() * 320) * (impact ? 1.3 : 1),
      });
    }

    for (let i = 0; i < puffs; i += 1) {
      const angle = Math.random() * TAU;
      const speed = cell * 0.0004 * Math.random() * force;
      this.particles.push({
        kind: 'smoke',
        x: x + (Math.random() - 0.5) * cell * 0.3 * force,
        y: y + (Math.random() - 0.5) * cell * 0.3 * force,
        vx: Math.cos(angle) * speed,
        vy: Math.sin(angle) * speed - cell * 0.00018,
        gravity: -cell * 0.0000004,
        drag: 0.99,
        size: cell * (0.1 + Math.random() * 0.1) * (impact ? 1.6 : 1),
        color: '#cfc6e8',
        age: 0,
        life: (520 + Math.random() * 360) * (impact ? 1.25 : 1),
      });
    }
  }

  drawParticles(ctx) {
    for (const particle of this.particles) {
      const t = particle.age / particle.life;
      if (particle.kind === 'ring') {
        // The shock of the strike, thrown outward and thinning as it goes.
        ctx.globalAlpha = 0.7 * (1 - t);
        ctx.strokeStyle = particle.color;
        ctx.lineWidth = Math.max(1.5, particle.size * 0.22 * (1 - t));
        ctx.beginPath();
        ctx.arc(particle.x, particle.y, particle.size * (0.3 + t * 3.4), 0, TAU);
        ctx.stroke();
      } else if (particle.kind === 'smoke') {
        // Smoke swells as it thins out.
        ctx.globalAlpha = 0.34 * (1 - t) * (1 - t);
        ctx.fillStyle = particle.color;
        ctx.beginPath();
        ctx.arc(particle.x, particle.y, particle.size * (1 + t * 2.4), 0, TAU);
        ctx.fill();
      } else {
        ctx.globalAlpha = Math.min(1, 2.2 * (1 - t));
        ctx.fillStyle = particle.color;
        ctx.beginPath();
        ctx.arc(particle.x, particle.y, particle.size * (1 - t * 0.7), 0, TAU);
        ctx.fill();
      }
    }
    ctx.globalAlpha = 1;
  }

  /// Sizes the layer over the page to the page. Its CSS box is the whole
  /// viewport, so all this sets is the backing store behind it.
  layoutFx() {
    if (!this.fx) {
      return;
    }
    const rect = this.fx.getBoundingClientRect();
    const dpr = Math.min(window.devicePixelRatio || 1, MAX_DPR);
    this.fxWidth = rect.width;
    this.fxHeight = rect.height;
    this.fxDpr = dpr;
    this.fx.width = Math.round(rect.width * dpr);
    this.fx.height = Math.round(rect.height * dpr);
    this.fxPainted = false;
  }

  /** Sizes the canvas to the largest whole-cell board its container allows. */
  layout() {
    this.layoutFx();
    const stage = this.canvas.parentElement;
    const available = stage.getBoundingClientRect();
    const { rows, cols } = this.engine;
    if (rows === 0 || cols === 0) {
      return;
    }

    // Reserve a hair of padding so falling gems and selection rings have room.
    const cell = Math.max(
      12,
      Math.floor(Math.min(available.width / (cols + 0.4), available.height / (rows + 0.4))),
    );
    this.cell = cell;
    this.pad = Math.round(cell * 0.2);

    const width = cell * cols + this.pad * 2;
    const height = cell * rows + this.pad * 2;
    const dpr = Math.min(window.devicePixelRatio || 1, MAX_DPR);

    this.canvas.style.width = `${width}px`;
    this.canvas.style.height = `${height}px`;
    this.canvas.width = Math.round(width * dpr);
    this.canvas.height = Math.round(height * dpr);
    this.dpr = dpr;
    this.width = width;
    this.height = height;
    // Everything cached is sized in device pixels, so it all goes stale here.
    this.sprites = new Map();
    this.backdrop = null;
    this.dirty = true;
  }

  /// The gem art for one color and special, drawn once and kept.
  ///
  /// Filling and stroking a path per cell per frame is what makes a phone
  /// struggle; blitting a bitmap per cell does not. Rainbows and rockets are
  /// cached in their resting orientation and turned as they are blitted.
  sprite(color, special) {
    // Rockets and rainbows wear no gem colors, so each needs only one entry.
    const colorless = special === Special.ROCKET || special === Special.RAINBOW;
    const key = (colorless ? 0 : color) * 8 + special;
    const cached = this.sprites.get(key);
    if (cached) {
      return cached;
    }
    const radius = this.cell * 0.42 * this.dpr;
    const size = Math.max(8, Math.ceil(radius * 2.6));
    const canvas = document.createElement('canvas');
    canvas.width = size;
    canvas.height = size;
    paintGem(canvas.getContext('2d'), size / 2, size / 2, radius, color, special);
    const sprite = { canvas, size, radius };
    this.sprites.set(key, sprite);
    return sprite;
  }

  /// The board panel and its empty sockets, which never change between resizes.
  backdropCanvas() {
    if (this.backdrop) {
      return this.backdrop;
    }
    const { cell, pad, dpr } = this;
    const canvas = document.createElement('canvas');
    canvas.width = Math.round(this.width * dpr);
    canvas.height = Math.round(this.height * dpr);
    const ctx = canvas.getContext('2d');
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);

    ctx.fillStyle = '#1a1630';
    roundRect(ctx, 2, 2, this.width - 4, this.height - 4, Math.round(cell * 0.28));
    ctx.fill();

    const { cells } = this.engine.snapshot();
    const cols = this.engine.cols;
    for (let i = 0; i < cells.length / 4; i += 1) {
      const r = Math.floor(i / cols);
      const c = i % cols;

      if (cells[i * 4 + 3] & Flag.WALL) {
        // Ground, drawn as something solid rather than left as bare panel.
        // A cell can now legitimately be empty, so "nothing here" is a state
        // the player has to be able to tell from "nothing fits here", and two
        // shades of dark will not do it. Lit along the top and shadowed at the
        // foot, so it reads as a block with a thickness to it.
        const edge = Math.round(cell * 0.02);
        const x = pad + c * cell + edge;
        const y = pad + r * cell + edge;
        const side = cell - edge * 2;
        ctx.save();
        roundRect(ctx, x, y, side, side, cell * 0.14);
        ctx.clip();
        ctx.fillStyle = '#3c3560';
        ctx.fillRect(x, y, side, side);
        ctx.fillStyle = 'rgba(255,255,255,0.13)';
        ctx.fillRect(x, y, side, side * 0.36);
        ctx.fillStyle = 'rgba(0,0,0,0.3)';
        ctx.fillRect(x, y + side * 0.74, side, side * 0.26);
        ctx.restore();
        ctx.strokeStyle = 'rgba(10,8,22,0.55)';
        ctx.lineWidth = Math.max(1, cell * 0.035);
        roundRect(ctx, x, y, side, side, cell * 0.14);
        ctx.stroke();
        continue;
      }

      const inset = Math.round(cell * 0.04);
      ctx.fillStyle = (r + c) % 2 === 0 ? 'rgba(255,255,255,0.035)' : 'rgba(255,255,255,0.015)';
      roundRect(
        ctx,
        pad + c * cell + inset,
        pad + r * cell + inset,
        cell - inset * 2,
        cell - inset * 2,
        cell * 0.18,
      );
      ctx.fill();
    }

    this.backdrop = canvas;
    return canvas;
  }

  /** The cell under a client-space point, or null when outside the board. */
  cellFromPoint(clientX, clientY) {
    const rect = this.canvas.getBoundingClientRect();
    const c = Math.floor((clientX - rect.left - this.pad) / this.cell);
    const r = Math.floor((clientY - rect.top - this.pad) / this.cell);
    if (r < 0 || c < 0 || r >= this.engine.rows || c >= this.engine.cols) {
      return null;
    }
    return { r, c };
  }

  draw(timeMs) {
    const { ctx, cell, pad } = this;
    const cols = this.engine.cols;
    const { cells, offsets } = this.engine.snapshot();

    // With the board in hand and before the next tick touches it, which is
    // what makes it a record of the board as the last clear found it. Reading
    // it where the events arrive instead would be reading it one tick too
    // late, after the engine has already peeled what it cleared and counted
    // it toward the goal it finished.
    this.captureBoard();
    this.updateParticles(timeMs);
    // Before the board's own early return: what is going to a goal has left
    // the board, and an idle board is exactly when the last of it is landing.
    this.paintFx();

    // A board at rest is worth nothing to redraw, and redrawing it is most of
    // what a phone was being asked to do. But "at rest" has to account for
    // anything that animates on its own: a rainbow spins whether or not the
    // board is doing something, and skipping its frames freezes it.
    let selected = false;
    let spinning = false;
    for (let i = 0; i < cells.length / 4; i += 1) {
      if (cells[i * 4 + 3] & Flag.SELECTED) {
        selected = true;
      }
      const special = cells[i * 4 + 1];
      if (special === Special.RAINBOW || special === Special.ARCHIPELAGO) {
        spinning = true;
      }
      if (selected && spinning) {
        break;
      }
    }
    const busy =
      this.engine.phase !== Phase.IDLE ||
      this.particles.length > 0 ||
      this.pendingBursts.length > 0 ||
      this.hint !== null ||
      this.toast !== null ||
      selected ||
      spinning;
    if (!busy && !this.dirty) {
      return;
    }
    this.dirty = busy;

    ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
    ctx.clearRect(0, 0, this.width, this.height);
    ctx.drawImage(this.backdropCanvas(), 0, 0, this.width, this.height);

    // Jelly is the only part of the board under the gems that changes.
    for (let i = 0; i < cells.length / 4; i += 1) {
      const jelly = cells[i * 4 + 2];
      if (jelly === 0) {
        continue;
      }
      const r = Math.floor(i / cols);
      const c = i % cols;
      const inset = Math.round(cell * 0.04);
      drawJelly(
        ctx,
        pad + c * cell + inset,
        pad + r * cell + inset,
        cell - inset * 2,
        jelly > 1,
      );
    }

    // Bricks stand in the board rather than on it, and they change as they are
    // hit, so they cannot live in the backdrop with the walls.
    for (let i = 0; i < cells.length / 4; i += 1) {
      const flags = cells[i * 4 + 3];
      if (!(flags & Flag.BRICK)) {
        continue;
      }
      const inset = Math.round(cell * 0.03);
      const x = pad + (i % cols) * cell + inset;
      const y = pad + Math.floor(i / cols) * cell + inset;
      const side = cell - inset * 2;
      const cracked = (flags & Flag.CRACKED) !== 0;
      if (flags & Flag.SEAL) {
        const gem = PALETTE[cells[i * 4] % PALETTE.length];
        drawSeal(ctx, x, y, side, gem.fill, gem.edge, cracked);
      } else {
        drawBrick(ctx, x, y, side, cracked);
      }
    }

    // Rockets fly over the board, so they are held back and blitted last.
    const airborne = [];

    for (let i = 0; i < cells.length / 4; i += 1) {
      const color = cells[i * 4];
      const special = cells[i * 4 + 1];
      // A blocker holds no gem, and a seal puts the color it answers to in
      // this byte, so the flag is what says whether there is a gem here.
      //
      // An empty cell and an Archipelago gem both report no color, because
      // the gem genuinely has none, so the color byte alone cannot tell them
      // apart: what separates them is that an empty cell carries no special
      // either. Reading emptiness off the color alone drew the gem as a hole
      // in the board.
      const empty = color === EMPTY_CELL && special === Special.NONE;
      if (empty || cells[i * 4 + 3] & Flag.BRICK) {
        continue;
      }
      const r = Math.floor(i / cols);
      const c = i % cols;
      const flags = cells[i * 4 + 3];
      let scale = offsets[i * 3 + 2];
      if (scale <= 0.01) {
        continue;
      }

      const x = pad + (c + offsets[i * 3]) * cell + cell / 2;
      const y = pad + (r + offsets[i * 3 + 1]) * cell + cell / 2;

      if (flags & Flag.SELECTED) {
        scale *= 1.06 + Math.sin(timeMs / 140) * 0.04;
        ctx.strokeStyle = '#ffd166';
        ctx.lineWidth = Math.max(2, cell * 0.05);
        roundRect(ctx, pad + c * cell + 2, pad + r * cell + 2, cell - 4, cell - 4, cell * 0.2);
        ctx.stroke();
      }

      if (special === Special.ROCKET) {
        airborne.push([x, y, scale, color, offsets[i * 3], offsets[i * 3 + 1], i]);
      } else if (special === Special.ARCHIPELAGO) {
        // The one gem drawn from scratch every frame rather than blitted: its
        // turn is a rotation in three dimensions, which no amount of turning a
        // flat sprite reproduces. See `drawApGemBody`.
        drawApGemBody(ctx, x, y, cell * 0.42 * scale, (timeMs / AP_TURN_MS) * TAU);
      } else if (special === Special.RAINBOW) {
        // A cached gem turned as it is blitted, which a flat spin allows.
        this.blitTurned(x, y, scale, color, special, (timeMs / 1400) % TAU);
      } else {
        this.blit(x, y, scale, color, special);
      }
    }

    if (this.particles.length > 0) {
      this.drawParticles(ctx);
    }

    // A rocket comes round to its heading rather than snapping to it.
    //
    // Where it wants to point is a frame of its own travel, which is the
    // tangent of the arc it is flying. What it may not do is arrive there in
    // one frame: sitting in its cell it is upright, and the moment it moves it
    // would jump to whatever direction it set off in, then jump again as the
    // arc bit. Easing covers all of it with one rule, and because a launch
    // ramps up rather than starting at speed, the turn happens while the
    // rocket is still over its own cell, which is where a rocket turns.
    const heading = new Map();
    const turn = 1 - Math.exp(-this.frameMs / ROCKET_TURN_MS);
    const most = ROCKET_TURN_PER_MS * this.frameMs;
    for (const [x, y, scale, color, dx, dy, index] of airborne) {
      const traveling = Math.abs(dx) > 0.001 || Math.abs(dy) > 0.001;
      const was = this.rocketWas?.get(index);
      // Held from the last frame that moved, so a rocket barely under way
      // keeps the heading it has rather than reading no movement as upright.
      let wants = was ? was.wants : 0;
      if (was && Math.hypot(x - was.x, y - was.y) > 0.001) {
        wants = Math.atan2(y - was.y, x - was.x) + Math.PI / 2;
      }
      // The short way round, or a rocket turning from just west of north to
      // just east of it would take the long way to get there.
      const from = was ? was.angle : 0;
      const step = shortestTurn(wants - from) * turn;
      const angle = from + Math.max(-most, Math.min(most, step));
      heading.set(index, { x, y, angle, wants });

      if (traveling) {
        drawExhaust(ctx, x, y, cell * 0.42 * scale, angle);
      }
      this.blitTurned(x, y, scale, color, Special.ROCKET, angle);
    }
    this.rocketWas = heading;

    if (this.hint) {
      this.drawHint(timeMs);
    }
    if (this.toast) {
      this.drawToast(timeMs);
    }
  }

  /// A line of text that swells and fades over the middle of the board.
  ///
  /// It keeps growing the whole way through, fade-out included, which is what
  /// makes it read as something the board did rather than a label being shown
  /// and taken away. Drawn on the canvas rather than as an element over it, so
  /// it is sized in cells and lands in the same place on every screen.
  drawToast(timeMs) {
    const age = timeMs - this.toast.at;
    if (age >= TOAST_MS) {
      this.toast = null;
      return;
    }

    const t = age / TOAST_MS;
    // In, hold, out, as a trapezoid rather than a curve: the swell carries the
    // motion, and a fade that eases as well reads as sluggish.
    const fade = Math.max(0, Math.min(1, t / TOAST_IN, (1 - t) / TOAST_OUT));
    const scale = 0.84 + t * 0.3;

    const ctx = this.ctx;
    // The plate's padding and height are both in units of the text size, and
    // the text itself scales with it, so this one number sets the whole thing.
    // Multiplying it by the root of two doubles the area.
    const size = Math.max(20, this.cell * 0.59);
    const across = this.engine.cols * this.cell;
    ctx.save();
    ctx.globalAlpha = fade;
    ctx.translate(this.pad + across / 2, this.pad + (this.engine.rows * this.cell) / 2);
    // Measured before scaling, because the transform scales what is drawn and
    // not what the metrics say.
    ctx.font = `600 ${size}px system-ui, -apple-system, "Segoe UI", sans-serif`;
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    const width = ctx.measureText(this.toast.text).width;

    // Never wider than the board it sits on. The size above is in cells, so it
    // holds its proportions on any screen until the floor in it takes over, and
    // on a board narrow enough for that the text would hang off both sides.
    const plate = width + size * 2;
    const fitted = Math.min(scale, across / plate);
    ctx.scale(fitted, fitted);

    // On a plate, because this lands over a board full of bright gems and
    // white text alone would be unreadable across half of them.
    ctx.fillStyle = 'rgba(16,13,32,0.84)';
    roundRect(ctx, -width / 2 - size, -size, width + size * 2, size * 2, size);
    ctx.fill();
    ctx.strokeStyle = 'rgba(190,175,255,0.35)';
    ctx.lineWidth = Math.max(1, size * 0.05);
    ctx.stroke();

    ctx.fillStyle = '#f3effd';
    ctx.fillText(this.toast.text, 0, 0);
    ctx.restore();
  }

  /// Stamps a cached gem, centered.
  blit(x, y, scale, color, special) {
    const sprite = this.sprite(color, special);
    const size = (sprite.size * scale) / this.dpr;
    this.ctx.drawImage(sprite.canvas, x - size / 2, y - size / 2, size, size);
  }

  /// The same, for the two items that are drawn at an angle.
  blitTurned(x, y, scale, color, special, angle) {
    const { ctx } = this;
    const sprite = this.sprite(color, special);
    const size = (sprite.size * scale) / this.dpr;
    ctx.save();
    ctx.translate(x, y);
    ctx.rotate(angle);
    ctx.drawImage(sprite.canvas, -size / 2, -size / 2, size, size);
    ctx.restore();
  }

  drawHint(timeMs) {
    const { ctx, cell, pad } = this;
    const pulse = 0.45 + 0.35 * Math.sin(timeMs / 260);
    ctx.save();
    ctx.strokeStyle = `rgba(255,209,102,${pulse.toFixed(3)})`;
    ctx.lineWidth = Math.max(2, cell * 0.06);
    for (const [r, c] of [
      [this.hint[0], this.hint[1]],
      [this.hint[2], this.hint[3]],
    ]) {
      roundRect(ctx, pad + c * cell + 2, pad + r * cell + 2, cell - 4, cell - 4, cell * 0.2);
      ctx.stroke();
    }
    ctx.restore();
  }
}

/// The six spheres of the Archipelago mark, clockwise from the top.
///
/// The logo's own colors, sampled off it, in its own arrangement: red, green,
/// purple, orange, blue, yellow, which is scattered rather than spectral.
///
/// Muted on purpose, and left that way rather than lifted to the board's own
/// strength. At full saturation this would be six bright circles in a ring
/// beside a rainbow, which is six bright colors in a disc, and at a cell's
/// size those read as the same object. Drained, it reads as what it is: a
/// thing from another world that does not belong to the gem set.
const AP_LOBES = [
  '#99625e',
  '#6e9566',
  '#987f99',
  '#a27357',
  '#5d6890',
  '#aba071',
];

/// How long one revolution of an Archipelago gem takes. Slower than the
/// rainbow's turn, which is a spin; this is a rotation you are meant to watch.
const AP_TURN_MS = 2600;

/// The gem color the three marked specials wear on a tracker icon. One color
/// for all three, because what tells them apart is the marking: five icons in
/// five hues would read as five colors rather than as five items.
const ICON_COLOR = 1;

/**
 * Paints one special at icon size into a canvas of its own, for the tracker.
 *
 * The board's own painter rather than a glyph or a picture, so what is being
 * tracked looks exactly like the thing that turns up in play. `size` is in CSS
 * pixels; the backing store is sized for the display the same way the board's
 * sprites are.
 */
export function paintSpecialIcon(canvas, special, size) {
  const dpr = Math.min(window.devicePixelRatio || 1, MAX_DPR);
  canvas.width = Math.round(size * dpr);
  canvas.height = Math.round(size * dpr);
  canvas.style.width = `${size}px`;
  canvas.style.height = `${size}px`;
  paintGem(
    canvas.getContext('2d'),
    canvas.width / 2,
    canvas.height / 2,
    size * dpr * 0.4,
    ICON_COLOR,
    special,
  );
}

/// The same turn expressed as the shortest way round, which is somewhere in
/// `(-PI, PI]`. Without this a rocket turning past north would swing the whole
/// way round the other side.
export function shortestTurn(radians) {
  return radians - TAU * Math.round(radians / TAU);
}

/// A point along a mote's arc: from where its cell was, bending past a control
/// point thrown off it, to wherever its goal is now.
function bezier(mote, e) {
  const u = 1 - e;
  return {
    x: u * u * mote.x0 + 2 * u * e * mote.cx + e * e * mote.tx,
    y: u * u * mote.y0 + 2 * u * e * mote.cy + e * e * mote.ty,
  };
}

/**
 * Paints what one goal is asking for, at chip size, into a canvas of its own.
 *
 * The board's own painters rather than a swatch or a glyph, for the same
 * reason the tracker uses them: a goal that shows the thing it wants needs no
 * words, and a picture of the thing drawn some other way would be a second
 * answer to what an emerald looks like.
 */
export function paintGoalIcon(canvas, goal, size) {
  const dpr = Math.min(window.devicePixelRatio || 1, MAX_DPR);
  canvas.width = Math.round(size * dpr);
  canvas.height = Math.round(size * dpr);
  canvas.style.width = `${size}px`;
  canvas.style.height = `${size}px`;
  const ctx = canvas.getContext('2d');
  const side = size * dpr;
  const gem = PALETTE[goal.color % PALETTE.length];

  if (goal.kind === ObjectiveKind.SEAL) {
    drawSeal(ctx, side * 0.06, side * 0.06, side * 0.88, gem.fill, gem.edge, false);
  } else if (goal.kind === ObjectiveKind.BRICK) {
    drawBrick(ctx, side * 0.06, side * 0.06, side * 0.88, false);
  } else if (goal.kind === ObjectiveKind.JELLY) {
    // The double layer, which is the solid one. A single layer is a tint you
    // can see against a board and not against a chip this size.
    drawJelly(ctx, side * 0.08, side * 0.08, side * 0.84, true);
  } else if (goal.kind === ObjectiveKind.SCORE) {
    drawController(ctx, side / 2, side / 2, side * 0.46);
  } else {
    paintGem(ctx, side / 2, side / 2, side * 0.42, goal.color, Special.NONE);
  }
}

/**
 * Paints one gem into a sprite: body, highlight, then its special marking.
 *
 * Run once per color and special rather than once per gem per frame, which is
 * what lets the highlight be clipped at all. `clip()` is one of the most
 * expensive things a canvas can be asked to do, and a phone shows it.
 */
function paintGem(ctx, x, y, radius, colorIndex, special) {
  const gem = PALETTE[colorIndex % PALETTE.length];

  if (special === Special.ROCKET) {
    // A rocket is not a gem wearing a hat: it replaces the gem entirely.
    drawRocketBody(ctx, x, y, radius);
    return;
  }
  if (special === Special.RAINBOW) {
    // Nor is a rainbow. It answers to every color, so it wears none of them.
    drawRainbowBody(ctx, x, y, radius);
    return;
  }

  ctx.save();
  shapePath(ctx, gem.shape, x, y, radius);
  ctx.fillStyle = gem.fill;
  ctx.fill();
  ctx.strokeStyle = gem.edge;
  ctx.lineWidth = Math.max(1, radius * 0.12);
  ctx.stroke();

  // Everything from here is inside the gem: the facet highlight, and the
  // marking, which is sized for a circle and would otherwise hang off the side
  // of a triangle or a star. Clipping is only affordable because this is a
  // sprite painted once, not a gem painted every frame.
  ctx.clip();

  // A soft highlight up and to the left reads as a facet.
  ctx.fillStyle = 'rgba(255,255,255,0.28)';
  ctx.beginPath();
  ctx.ellipse(x - radius * 0.28, y - radius * 0.34, radius * 0.5, radius * 0.3, -0.6, 0, TAU);
  ctx.fill();

  drawSpecial(ctx, x, y, radius, special);
  ctx.restore();
}

/// The line and cross markings. Rockets and rainbows are whole gems of their
/// own and never reach here.
function drawSpecial(ctx, x, y, radius, special) {
  if (special === Special.NONE) {
    return;
  }
  ctx.save();
  ctx.lineCap = 'round';
  ctx.strokeStyle = 'rgba(255,255,255,0.92)';
  ctx.lineWidth = Math.max(2, radius * 0.16);

  if (special === Special.LINE_H || special === Special.LINE_V) {
    const horizontal = special === Special.LINE_H;
    for (const offset of [-radius * 0.34, radius * 0.34]) {
      ctx.beginPath();
      if (horizontal) {
        ctx.moveTo(x - radius * 0.8, y + offset);
        ctx.lineTo(x + radius * 0.8, y + offset);
      } else {
        ctx.moveTo(x + offset, y - radius * 0.8);
        ctx.lineTo(x + offset, y + radius * 0.8);
      }
      ctx.stroke();
    }
  } else if (special === Special.CROSS) {
    // Bars both ways, since it takes a row and a column together.
    ctx.beginPath();
    ctx.moveTo(x - radius * 0.8, y);
    ctx.lineTo(x + radius * 0.8, y);
    ctx.moveTo(x, y - radius * 0.8);
    ctx.lineTo(x, y + radius * 0.8);
    ctx.stroke();
  }

  ctx.restore();
}

/**
 * A game controller, which is what a score goal wears.
 *
 * Score is the one goal with nothing on the board to point at, so it gets a
 * mark for playing rather than a picture of a thing to clear. Drawn as a
 * silhouette with the pad and two buttons punched out of it: at chip size a
 * controller with its own d-pad and face buttons drawn on top is mush, and a
 * shape read against the chip behind it is not.
 */
function drawController(ctx, x, y, r) {
  const w = r * 2;
  const h = r * 1.28;
  ctx.save();
  ctx.beginPath();
  // The body: two grips with a waist between them, which is what makes the
  // silhouette a controller rather than a lozenge.
  ctx.moveTo(x - w * 0.5, y + h * 0.18);
  ctx.quadraticCurveTo(x - w * 0.56, y - h * 0.5, x - w * 0.2, y - h * 0.42);
  ctx.lineTo(x + w * 0.2, y - h * 0.42);
  ctx.quadraticCurveTo(x + w * 0.56, y - h * 0.5, x + w * 0.5, y + h * 0.18);
  ctx.quadraticCurveTo(x + w * 0.46, y + h * 0.56, x + w * 0.22, y + h * 0.4);
  ctx.quadraticCurveTo(x, y + h * 0.16, x - w * 0.22, y + h * 0.4);
  ctx.quadraticCurveTo(x - w * 0.46, y + h * 0.56, x - w * 0.5, y + h * 0.18);
  ctx.closePath();
  ctx.fillStyle = '#cfc6e8';
  ctx.fill();
  ctx.strokeStyle = '#3b3460';
  ctx.lineWidth = Math.max(1, r * 0.12);
  ctx.stroke();

  // Punched out rather than drawn on, so they read at any size the chip is.
  ctx.clip();
  ctx.fillStyle = '#2a2448';
  const arm = r * 0.34;
  const bar = r * 0.17;
  const px = x - w * 0.24;
  ctx.fillRect(px - arm / 2, y - bar / 2, arm, bar);
  ctx.fillRect(px - bar / 2, y - arm / 2, bar, arm);

  const bx = x + w * 0.24;
  for (const [dx, dy] of [[-r * 0.16, 0], [r * 0.16, 0], [0, -r * 0.17], [0, r * 0.17]]) {
    ctx.beginPath();
    ctx.arc(bx + dx, y + dy, r * 0.11, 0, TAU);
    ctx.fill();
  }
  ctx.restore();
}

/**
 * Jelly under a cell, and the same thing on a goal chip.
 *
 * There are two states, so they are drawn as two things rather than as two
 * steps of one. A faint tint against a slightly less faint one read as the
 * same cell with a gem sitting on most of it: the double layer is nearly
 * solid, and what shows around the gem is unmistakable at a glance.
 */
function drawJelly(ctx, x, y, size, doubled) {
  ctx.fillStyle = doubled ? 'rgba(232,250,255,0.52)' : 'rgba(160,230,255,0.16)';
  roundRect(ctx, x, y, size, size, size * 0.18);
  ctx.fill();
  ctx.strokeStyle = doubled ? 'rgba(255,255,255,0.72)' : 'rgba(200,245,255,0.4)';
  ctx.lineWidth = Math.max(1, size * (doubled ? 0.04 : 0.03));
  ctx.stroke();
}

/**
 * A seal: a blocker keyed to one gem color, which is the only color that
 * breaks it.
 *
 * Wearing that color is the whole point, so it is a plain rounded square in the
 * gem's own fill with rings drawn inside it, rather than anything shaped. The
 * rings are what separate it from the gem of the same color at a glance: gems
 * are silhouettes, this is a box with something locked in it.
 *
 * Cracked, the rings break open down one side and a fracture runs across, so
 * "one more of this color" reads without having to count rings.
 */
function drawSeal(ctx, x, y, size, fill, edge, cracked) {
  const radius = size * 0.16;
  ctx.save();
  roundRect(ctx, x, y, size, size, radius);
  ctx.clip();
  ctx.fillStyle = fill;
  ctx.fillRect(x, y, size, size);
  ctx.fillStyle = 'rgba(255,255,255,0.12)';
  ctx.fillRect(x, y, size, size * 0.3);
  ctx.fillStyle = 'rgba(0,0,0,0.2)';
  ctx.fillRect(x, y + size * 0.8, size, size * 0.2);

  // Rings, drawn from the outside in and fading as they go, so the middle
  // reads as depth rather than as a target.
  ctx.lineWidth = Math.max(1, size * 0.045);
  for (const [inset, alpha] of [[0.16, 0.5], [0.28, 0.34], [0.4, 0.2]]) {
    ctx.strokeStyle = `rgba(255,255,255,${cracked ? alpha * 0.45 : alpha})`;
    const pad = size * inset;
    roundRect(ctx, x + pad, y + pad, size - pad * 2, size - pad * 2, radius * (1 - inset));
    ctx.stroke();
  }

  if (cracked) {
    ctx.strokeStyle = 'rgba(16,10,26,0.85)';
    ctx.lineWidth = Math.max(1, size * 0.08);
    ctx.lineCap = 'round';
    ctx.beginPath();
    ctx.moveTo(x + size * 0.2, y);
    ctx.lineTo(x + size * 0.5, y + size * 0.42);
    ctx.lineTo(x + size * 0.34, y + size * 0.62);
    ctx.lineTo(x + size * 0.7, y + size);
    ctx.stroke();
  }
  ctx.restore();

  ctx.strokeStyle = edge;
  ctx.lineWidth = Math.max(1, size * 0.06);
  roundRect(ctx, x, y, size, size, radius);
  ctx.stroke();
}

/**
 * A brick: masonry sitting in a cell, in the way of everything.
 *
 * Painted every frame rather than baked into the backdrop with the walls,
 * because unlike a wall it changes: it cracks, and then it goes. Two courses
 * of blocks with the joints staggered, lit from above like the walls so the
 * whole board agrees where the light comes from.
 */
function drawBrick(ctx, x, y, size, cracked) {
  const radius = size * 0.14;
  ctx.save();
  roundRect(ctx, x, y, size, size, radius);
  ctx.clip();

  ctx.fillStyle = cracked ? BRICK_CRACKED_FACE : BRICK_FACE;
  ctx.fillRect(x, y, size, size);
  ctx.fillStyle = 'rgba(255,255,255,0.11)';
  ctx.fillRect(x, y, size, size * 0.3);
  ctx.fillStyle = 'rgba(0,0,0,0.24)';
  ctx.fillRect(x, y + size * 0.78, size, size * 0.22);

  // The joints, staggered course to course the way a wall is actually laid.
  ctx.strokeStyle = BRICK_MORTAR;
  ctx.lineWidth = Math.max(1, size * 0.055);
  ctx.beginPath();
  ctx.moveTo(x, y + size / 2);
  ctx.lineTo(x + size, y + size / 2);
  ctx.moveTo(x + size / 2, y);
  ctx.lineTo(x + size / 2, y + size / 2);
  ctx.moveTo(x + size * 0.25, y + size / 2);
  ctx.lineTo(x + size * 0.25, y + size);
  ctx.moveTo(x + size * 0.75, y + size / 2);
  ctx.lineTo(x + size * 0.75, y + size);
  ctx.stroke();

  if (cracked) {
    // One hit left. The fracture runs right across it rather than chipping a
    // corner, so that "this one goes next" is readable at a glance.
    ctx.strokeStyle = 'rgba(18,9,7,0.92)';
    ctx.lineWidth = Math.max(1, size * 0.075);
    ctx.lineCap = 'round';
    ctx.beginPath();
    ctx.moveTo(x + size * 0.16, y);
    ctx.lineTo(x + size * 0.44, y + size * 0.34);
    ctx.lineTo(x + size * 0.28, y + size * 0.6);
    ctx.lineTo(x + size * 0.64, y + size);
    ctx.stroke();
  }
  ctx.restore();

  ctx.strokeStyle = 'rgba(18,9,7,0.5)';
  ctx.lineWidth = Math.max(1, size * 0.04);
  roundRect(ctx, x, y, size, size, radius);
  ctx.stroke();
}

/// The flame behind a rocket in flight, which cannot be cached because it only
/// burns while the rocket is moving.
function drawExhaust(ctx, x, y, r, angle) {
  ctx.save();
  ctx.translate(x, y);
  ctx.rotate(angle);
  const flame = ctx.createLinearGradient(0, r * 0.6, 0, r * 1.9);
  flame.addColorStop(0, 'rgba(255,209,102,0.85)');
  flame.addColorStop(1, 'rgba(255,120,60,0)');
  ctx.fillStyle = flame;
  ctx.beginPath();
  ctx.moveTo(-r * 0.36, r * 0.6);
  ctx.lineTo(r * 0.36, r * 0.6);
  ctx.lineTo(0, r * 1.9);
  ctx.closePath();
  ctx.fill();
  ctx.restore();
}

/// A rainbow at rest: wedges of every color, spun as it is blitted.
/**
 * An Archipelago gem: six spheres on a ring, the ring turning about the
 * vertical axis.
 *
 * The ring lies in the screen plane, so a sphere's height never moves and its
 * width breathes with the phase: a quarter turn in, the whole thing is edge-on
 * and reads as a vertical stack. Depth decides the drawing order, the size and
 * the shading, which is what makes the far side pass behind the near side
 * rather than through it.
 *
 * Drawn live rather than blitted from a cached sprite, which is the one gem
 * that is. A cache would need an entry per phase, and measured against a
 * frame's budget the saving is nothing: ten of these drawn from scratch every
 * frame cost about 1% of 16ms. They are rare, they carry no clipping, and a
 * live draw turns smoothly instead of stepping through however many phases
 * were cached.
 */
function drawApGemBody(ctx, x, y, r, phase) {
  // The rosette is mostly holes, so it reads smaller than a solid gem of the
  // same extent and wants a little more room than one. Only a little: at the
  // full extent it crowded its cell, so this is that sizing pulled back about
  // a seventh, which leaves it a touch inside the gem's own circle.
  const ring = r * 0.65;
  const lobe = r * 0.36;
  const cos = Math.cos(phase);
  const sin = Math.sin(phase);

  const spheres = AP_LOBES.map((fill, i) => {
    const a = -Math.PI / 2 + (i * TAU) / 6;
    const across = ring * Math.cos(a);
    return { x: x + across * cos, y: y + ring * Math.sin(a), z: -across * sin, fill };
  });
  // Far side first.
  spheres.sort((a, b) => a.z - b.z);

  for (const sphere of spheres) {
    // A touch of perspective. The near side being larger and brighter is most
    // of what sells the turn.
    const depth = sphere.z / ring;
    const size = lobe * (1 + depth * 0.1);
    const shade = ctx.createRadialGradient(
      sphere.x - size * 0.34,
      sphere.y - size * 0.38,
      size * 0.1,
      sphere.x,
      sphere.y,
      size,
    );
    shade.addColorStop(0, shiftLightness(sphere.fill, 0.42 + depth * 0.06));
    shade.addColorStop(0.55, shiftLightness(sphere.fill, depth * 0.08));
    shade.addColorStop(1, shiftLightness(sphere.fill, -0.38 + depth * 0.06));

    ctx.beginPath();
    ctx.arc(sphere.x, sphere.y, size, 0, TAU);
    ctx.fillStyle = shade;
    ctx.fill();
    ctx.strokeStyle = 'rgba(14,11,26,0.85)';
    ctx.lineWidth = Math.max(1, r * 0.055);
    ctx.stroke();

    ctx.beginPath();
    ctx.arc(sphere.x - size * 0.3, sphere.y - size * 0.34, size * 0.19, 0, TAU);
    ctx.fillStyle = `rgba(255,255,255,${0.62 + depth * 0.2})`;
    ctx.fill();
  }
}

/// Lightens a hex color toward white or darkens it toward black.
function shiftLightness(hex, amount) {
  const n = parseInt(hex.slice(1), 16);
  const shift = (channel) =>
    Math.max(0, Math.min(255, Math.round(
      amount > 0 ? channel + (255 - channel) * amount : channel * (1 + amount),
    )));
  return `rgb(${shift((n >> 16) & 255)},${shift((n >> 8) & 255)},${shift(n & 255)})`;
}

function drawRainbowBody(ctx, x, y, r) {
  for (let i = 0; i < PALETTE.length; i += 1) {
    ctx.beginPath();
    ctx.moveTo(x, y);
    ctx.arc(x, y, r, (i * TAU) / PALETTE.length, ((i + 1) * TAU) / PALETTE.length);
    ctx.closePath();
    ctx.fillStyle = PALETTE[i].fill;
    ctx.fill();
  }
  ctx.beginPath();
  ctx.arc(x, y, r, 0, TAU);
  ctx.strokeStyle = 'rgba(28,24,48,0.85)';
  ctx.lineWidth = Math.max(1.5, r * 0.12);
  ctx.stroke();

  ctx.beginPath();
  ctx.arc(x, y, r * 0.26, 0, TAU);
  ctx.fillStyle = 'rgba(255,255,255,0.94)';
  ctx.fill();
  ctx.strokeStyle = 'rgba(28,24,48,0.5)';
  ctx.lineWidth = Math.max(1, r * 0.06);
  ctx.stroke();
}

/// A rocket at rest, nose up. It is turned toward its target as it is blitted.
///
/// Deliberately in no gem's colors. A rocket takes no part in matching, so
/// tinting it like a gem would promise a color it does not have.
function drawRocketBody(ctx, x, y, r) {
  ctx.save();
  ctx.translate(x, y);

  // Fins, then the body over them.
  ctx.fillStyle = ROCKET_FIN;
  ctx.beginPath();
  ctx.moveTo(-r * 0.42, r * 0.15);
  ctx.lineTo(-r * 0.92, r * 0.72);
  ctx.lineTo(-r * 0.42, r * 0.72);
  ctx.moveTo(r * 0.42, r * 0.15);
  ctx.lineTo(r * 0.92, r * 0.72);
  ctx.lineTo(r * 0.42, r * 0.72);
  ctx.fill();

  ctx.beginPath();
  ctx.moveTo(0, -r);
  ctx.quadraticCurveTo(r * 0.52, -r * 0.3, r * 0.46, r * 0.72);
  ctx.lineTo(-r * 0.46, r * 0.72);
  ctx.quadraticCurveTo(-r * 0.52, -r * 0.3, 0, -r);
  ctx.closePath();
  ctx.fillStyle = ROCKET_BODY;
  ctx.fill();
  ctx.strokeStyle = ROCKET_EDGE;
  ctx.lineWidth = Math.max(1.5, r * 0.12);
  ctx.stroke();

  // A nose cone and a porthole, so it reads as a rocket rather than an arrow.
  ctx.beginPath();
  ctx.moveTo(0, -r);
  ctx.quadraticCurveTo(r * 0.3, -r * 0.62, r * 0.24, -r * 0.34);
  ctx.lineTo(-r * 0.24, -r * 0.34);
  ctx.quadraticCurveTo(-r * 0.3, -r * 0.62, 0, -r);
  ctx.closePath();
  ctx.fillStyle = ROCKET_FIN;
  ctx.fill();

  ctx.beginPath();
  ctx.arc(0, r * 0.08, r * 0.19, 0, TAU);
  ctx.fillStyle = ROCKET_PORT;
  ctx.fill();
  ctx.strokeStyle = ROCKET_EDGE;
  ctx.lineWidth = Math.max(1, r * 0.07);
  ctx.stroke();

  ctx.restore();
}

function shapePath(ctx, shape, x, y, r) {
  ctx.beginPath();
  switch (shape) {
    case 'circle':
      // Pulled in a little. A circle fills its radius completely where every
      // other shape here leaves corners of it empty, so drawn to the same
      // radius it sits on the board looking like the largest gem.
      ctx.arc(x, y, r * 0.93, 0, TAU);
      break;
    case 'diamond':
      // A square stood on its corner. Both axes are the same length: stretched
      // taller it stops reading as a square and starts reading as a kite.
      roundedPath(ctx, polygon(x, y, r, 4, -Math.PI / 2), r * 0.26);
      break;
    case 'square':
      roundRect(ctx, x - r * 0.84, y - r * 0.84, r * 1.68, r * 1.68, r * 0.3);
      break;
    case 'triangle':
      // Nudged down, because a triangle's weight sits low and centering it on
      // its bounding box leaves it looking to have slipped. Grown past the
      // others too: inscribed in the same circle it covers far less of it, and
      // reads as a smaller gem rather than a different one.
      roundedPath(ctx, polygon(x, y + r * 0.12, r * 1.16, 3, -Math.PI / 2), r * 0.24);
      break;
    case 'hex':
      roundedPath(ctx, polygon(x, y, r, 6, 0), r * 0.22);
      break;
    case 'pentagon':
      roundedPath(ctx, polygon(x, y, r, 5, -Math.PI / 2), r * 0.22);
      break;
    case 'star':
      // Grown past the others for the same reason the triangle is: five points
      // with gaps between them cover little of the circle they are cut from.
      roundedPath(ctx, star(x, y, r * 1.14, r * 0.56, 5), r * 0.2);
      break;
    case 'tomb': {
      // A headstone: straight sides, a semicircular top, and the two corners it
      // stands on rounded off like everything else here.
      const half = r * 0.7;
      const top = y - r * 0.9;
      const bottom = y + r * 0.88;
      const foot = r * 0.24;
      const springing = top + half;
      ctx.moveTo(x - half, bottom - foot);
      ctx.lineTo(x - half, springing);
      // Over the top, left to right. Increasing angle from PI wraps up and
      // over rather than down and under.
      ctx.arc(x, springing, half, Math.PI, 0);
      ctx.lineTo(x + half, bottom - foot);
      ctx.arcTo(x + half, bottom, x + half - foot, bottom, foot);
      ctx.lineTo(x - half + foot, bottom);
      ctx.arcTo(x - half, bottom, x - half, bottom - foot, foot);
      ctx.closePath();
      break;
    }
    case 'drop': {
      // A teardrop: a circle low in the cell, drawn up to a tip. The straight
      // sides are the tangents from that tip to the circle, so the curve runs
      // into them without a seam, and the tip itself is rounded over.
      //
      // Widening is the bulb's radius; the center rises to keep the bottom of
      // the bulb where it was, so the drop grows sideways rather than downward
      // out of its cell.
      const cy = y + r * 0.2;
      const body = r * 0.8;
      const reach = cy - (y - r);
      const spread = Math.acos(Math.min(1, body / reach));
      const left = -Math.PI / 2 - spread;
      const right = -Math.PI / 2 + spread;
      ctx.moveTo(x + Math.cos(left) * body, cy + Math.sin(left) * body);
      ctx.arcTo(
        x,
        y - r,
        x + Math.cos(right) * body,
        cy + Math.sin(right) * body,
        r * 0.15,
      );
      ctx.lineTo(x + Math.cos(right) * body, cy + Math.sin(right) * body);
      ctx.arc(x, cy, body, right, left);
      ctx.closePath();
      break;
    }
    default:
      ctx.arc(x, y, r, 0, TAU);
  }
}

function polygon(x, y, r, sides, rotation) {
  const points = [];
  for (let i = 0; i < sides; i += 1) {
    const angle = rotation + (i * TAU) / sides;
    points.push([x + Math.cos(angle) * r, y + Math.sin(angle) * r]);
  }
  return points;
}

function star(x, y, outer, inner, points) {
  const corners = [];
  for (let i = 0; i < points * 2; i += 1) {
    const radius = i % 2 === 0 ? outer : inner;
    const angle = -Math.PI / 2 + (i * Math.PI) / points;
    corners.push([x + Math.cos(angle) * radius, y + Math.sin(angle) * radius]);
  }
  return corners;
}

/**
 * Traces a closed run of corners with every one of them rounded off.
 *
 * `arcTo` rounds a corner by cutting back along both of its edges, so a radius
 * an edge cannot afford overshoots and folds the shape through itself. Each
 * corner is clamped to what its own two edges and its own angle allow, which is
 * why a star's needle points round off less than a hexagon's blunt ones: at a
 * sharp angle the same radius would eat most of the edge.
 *
 * The path starts partway along an edge rather than at a corner, since every
 * corner is about to be cut back from both sides.
 */
function roundedPath(ctx, points, radius) {
  const count = points.length;
  const last = points[count - 1];
  ctx.moveTo((last[0] + points[0][0]) / 2, (last[1] + points[0][1]) / 2);

  for (let i = 0; i < count; i += 1) {
    const previous = points[(i + count - 1) % count];
    const corner = points[i];
    const next = points[(i + 1) % count];
    const back = Math.hypot(previous[0] - corner[0], previous[1] - corner[1]);
    const on = Math.hypot(next[0] - corner[0], next[1] - corner[1]);
    if (back === 0 || on === 0) {
      ctx.lineTo(corner[0], corner[1]);
      continue;
    }
    // Half of the shorter edge: the corner at its far end claims the rest.
    const room = Math.min(back, on) / 2;
    const ax = (previous[0] - corner[0]) / back;
    const ay = (previous[1] - corner[1]) / back;
    const bx = (next[0] - corner[0]) / on;
    const by = (next[1] - corner[1]) / on;
    const half = Math.acos(Math.max(-1, Math.min(1, ax * bx + ay * by))) / 2;
    ctx.arcTo(
      corner[0],
      corner[1],
      (corner[0] + next[0]) / 2,
      (corner[1] + next[1]) / 2,
      Math.min(radius, room * Math.tan(half)),
    );
  }
  ctx.closePath();
}

/** `CanvasRenderingContext2D.roundRect` is recent; this keeps older phones in. */
function roundRect(ctx, x, y, w, h, r) {
  const radius = Math.min(r, w / 2, h / 2);
  ctx.beginPath();
  ctx.moveTo(x + radius, y);
  ctx.arcTo(x + w, y, x + w, y + h, radius);
  ctx.arcTo(x + w, y + h, x, y + h, radius);
  ctx.arcTo(x, y + h, x, y, radius);
  ctx.arcTo(x, y, x + w, y, radius);
  ctx.closePath();
}
