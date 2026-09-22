// Canvas rendering.
//
// The engine hands over a color, a special, an offset and a scale per cell; all
// this file decides is what that looks like. Gems differ by shape as well as
// hue so the board stays readable without relying on color alone.

import { EMPTY_CELL, EventKind, Flag, Phase, Special } from './engine.js';

/// The first `rules.colors` of these are what gets dealt, so the order is the
/// game's gem set and not just a list. Six are in play today.
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

/// A rocket wears no gem's colors, because it belongs to no color.
const ROCKET_BODY = '#eceaf6';
const ROCKET_EDGE = '#39325c';
const ROCKET_FIN = '#e5484d';
const ROCKET_PORT = '#8fd0ff';
const SHARDS_PER_GEM = 9;
const PUFFS_PER_GEM = 4;
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
/// Said when the goal is met and the leftover moves are being spent. Held in
/// one place because the pop-over checks against it to avoid re-raising itself
/// on every round of the flourish.
const FINALE_TOAST = 'Goal! Cashing in';
const TOAST_MS = 1800;
const TOAST_IN = 0.18;
const TOAST_OUT = 0.4;

export class Renderer {
  constructor(canvas, engine) {
    this.canvas = canvas;
    this.engine = engine;
    this.ctx = canvas.getContext('2d', { alpha: true });
    this.cell = 0;
    this.pad = 0;
    this.hint = null;
    /// Bursts waiting for their moment: a blast spreads outward, so each cell's
    /// debris is held back until the clear actually reaches it.
    this.pendingBursts = [];
    this.particles = [];
    /// A line of text swelling and fading over the board, or null.
    this.toast = null;
    this.lastFrame = null;
    this.layout();
  }

  /// Queues debris for everything the engine just cleared. Each event carries
  /// the delay the engine assigned it, which is what makes a row clear ripple.
  addEvents(events, now) {
    for (const event of events) {
      if (event.kind === EventKind.CLEAR) {
        this.pendingBursts.push({ at: now + event.value, r: event.r, c: event.c, color: event.color });
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
        this.pendingBursts.push({
          at: now,
          r: event.r,
          c: event.c,
          color: sealed ? event.color : 0,
          impact: event.value === 0,
          tint: sealed ? null : BRICK_DEBRIS,
        });
      } else if (event.kind === EventKind.SHUFFLE) {
        // The board is about to rearrange itself. Without a word about it the
        // player looks away and looks back at a different board.
        this.toast = { text: 'No moves, shuffling', at: now };
      } else if (event.kind === EventKind.LOW_MOVES) {
        const left = event.value;
        this.toast = { text: `${left} move${left === 1 ? '' : 's'} left`, at: now };
      } else if (event.kind === EventKind.FINALE) {
        // The goal is met and the board is about to take itself apart. Said
        // once, on the first round: the later rounds are the same event again
        // and would keep re-raising the pop-over over its own fade.
        if (!this.toast || this.toast.text !== FINALE_TOAST) {
          this.toast = { text: FINALE_TOAST, at: now };
        }
      }
    }
  }

  /// Drops everything in flight, for a restart or a level change.
  reset() {
    this.pendingBursts.length = 0;
    this.particles.length = 0;
    this.toast = null;
    this.lastFrame = null;
    this.backdrop = null;
    this.dirty = true;
  }

  updateParticles(now) {
    const dt = this.lastFrame === null ? 16 : Math.min(now - this.lastFrame, 50);
    this.lastFrame = now;

    if (this.pendingBursts.length > 0) {
      const due = [];
      const waiting = [];
      for (const burst of this.pendingBursts) {
        (burst.at <= now ? due : waiting).push(burst);
      }
      this.pendingBursts = waiting;
      for (const burst of due) {
        this.burst(burst);
      }
    }

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

  /// One cell's worth of debris: shards of the gem, and a puff of smoke. A
  /// rocket strike throws the same thing much harder, with a blast ring.
  burst({ r, c, color, impact = false, tint = null }) {
    if (this.particles.length > MAX_PARTICLES) {
      return;
    }
    const { cell, pad } = this;
    const x = pad + (c + 0.5) * cell;
    const y = pad + (r + 0.5) * cell;
    // A tint for debris that is not a gem and so has no palette entry of its
    // own, which so far means brick.
    const gem = { fill: tint ?? PALETTE[color % PALETTE.length].fill };
    const shards = impact ? SHARDS_PER_IMPACT : SHARDS_PER_GEM;
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

  /** Sizes the canvas to the largest whole-cell board its container allows. */
  layout() {
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

    this.updateParticles(timeMs);

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
      if (cells[i * 4 + 1] === Special.RAINBOW) {
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
      ctx.fillStyle = jelly > 1 ? 'rgba(160,230,255,0.28)' : 'rgba(160,230,255,0.14)';
      roundRect(ctx, pad + c * cell + inset, pad + r * cell + inset, cell - inset * 2, cell - inset * 2, cell * 0.18);
      ctx.fill();
      ctx.strokeStyle = 'rgba(200,245,255,0.45)';
      ctx.lineWidth = Math.max(1, cell * 0.03);
      ctx.stroke();
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
      // A blocker holds no gem, and a seal puts the color it answers to in
      // this byte, so the flag is what says whether there is a gem here.
      if (color === EMPTY_CELL || cells[i * 4 + 3] & Flag.BRICK) {
        continue;
      }
      const r = Math.floor(i / cols);
      const c = i % cols;
      const special = cells[i * 4 + 1];
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
        airborne.push([x, y, scale, color, offsets[i * 3], offsets[i * 3 + 1]]);
      } else if (special === Special.RAINBOW) {
        // The one cached gem that turns: it is drawn spinning.
        this.blitTurned(x, y, scale, color, special, (timeMs / 1400) % TAU);
      } else {
        this.blit(x, y, scale, color, special);
      }
    }

    if (this.particles.length > 0) {
      this.drawParticles(ctx);
    }

    for (const [x, y, scale, color, dx, dy] of airborne) {
      const travelling = Math.abs(dx) > 0.001 || Math.abs(dy) > 0.001;
      const angle = travelling ? Math.atan2(dy, dx) + Math.PI / 2 : 0;
      if (travelling) {
        drawExhaust(ctx, x, y, cell * 0.42 * scale, angle);
      }
      this.blitTurned(x, y, scale, color, Special.ROCKET, angle);
    }

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
