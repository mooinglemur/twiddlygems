// Canvas rendering.
//
// The engine hands over a color, a special, an offset and a scale per cell; all
// this file decides is what that looks like. Gems differ by shape as well as
// hue so the board stays readable without relying on color alone.

import { EMPTY_CELL, Flag, Special } from './engine.js';

export const PALETTE = [
  { name: 'Ruby', fill: '#e5484d', edge: '#7e1f25', shape: 'diamond' },
  { name: 'Sapphire', fill: '#3f8cff', edge: '#1b3f8c', shape: 'circle' },
  { name: 'Emerald', fill: '#3fbf6f', edge: '#166534', shape: 'hex' },
  { name: 'Topaz', fill: '#f5c542', edge: '#87660b', shape: 'square' },
  { name: 'Amethyst', fill: '#a56bff', edge: '#4c2a8a', shape: 'triangle' },
  { name: 'Aqua', fill: '#2dd4c8', edge: '#0e6f68', shape: 'star' },
  { name: 'Rose', fill: '#ff8ac0', edge: '#a13a70', shape: 'pentagon' },
  { name: 'Amber', fill: '#f0863c', edge: '#8f430f', shape: 'drop' },
];

const TAU = Math.PI * 2;

export class Renderer {
  constructor(canvas, engine) {
    this.canvas = canvas;
    this.engine = engine;
    this.ctx = canvas.getContext('2d', { alpha: true });
    this.cell = 0;
    this.pad = 0;
    this.hint = null;
    this.layout();
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
    const dpr = Math.min(window.devicePixelRatio || 1, 3);

    this.canvas.style.width = `${width}px`;
    this.canvas.style.height = `${height}px`;
    this.canvas.width = Math.round(width * dpr);
    this.canvas.height = Math.round(height * dpr);
    this.dpr = dpr;
    this.width = width;
    this.height = height;
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
    const { rows, cols } = this.engine;
    const { cells, offsets } = this.engine.snapshot();

    ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
    ctx.clearRect(0, 0, this.width, this.height);

    // The board panel.
    ctx.fillStyle = '#1a1630';
    roundRect(ctx, 2, 2, this.width - 4, this.height - 4, Math.round(cell * 0.28));
    ctx.fill();

    // Sockets and jelly sit still; only gems move.
    for (let i = 0; i < cells.length / 4; i += 1) {
      const r = Math.floor(i / cols);
      const c = i % cols;
      const flags = cells[i * 4 + 3];
      if (flags & Flag.WALL) {
        continue;
      }
      const x = pad + c * cell;
      const y = pad + r * cell;
      const inset = Math.round(cell * 0.04);

      ctx.fillStyle = (r + c) % 2 === 0 ? 'rgba(255,255,255,0.035)' : 'rgba(255,255,255,0.015)';
      roundRect(ctx, x + inset, y + inset, cell - inset * 2, cell - inset * 2, cell * 0.18);
      ctx.fill();

      const jelly = cells[i * 4 + 2];
      if (jelly > 0) {
        ctx.fillStyle = jelly > 1 ? 'rgba(160,230,255,0.28)' : 'rgba(160,230,255,0.14)';
        roundRect(ctx, x + inset, y + inset, cell - inset * 2, cell - inset * 2, cell * 0.18);
        ctx.fill();
        ctx.strokeStyle = 'rgba(200,245,255,0.45)';
        ctx.lineWidth = Math.max(1, cell * 0.03);
        ctx.stroke();
      }
    }

    // Gems are clipped to the panel so the ones falling in from above do not
    // spill over the bezel.
    ctx.save();
    roundRect(ctx, 2, 2, this.width - 4, this.height - 4, Math.round(cell * 0.28));
    ctx.clip();

    for (let i = 0; i < cells.length / 4; i += 1) {
      const color = cells[i * 4];
      if (color === EMPTY_CELL) {
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

      drawGem(
        ctx,
        x,
        y,
        cell * 0.42 * scale,
        color,
        special,
        timeMs,
        offsets[i * 3],
        offsets[i * 3 + 1],
      );
    }

    ctx.restore();

    if (this.hint) {
      this.drawHint(timeMs);
    }

    void rows;
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
 * Draws one gem: body, highlight, then whatever special marking it carries.
 *
 * `dx`/`dy` are the cell's current offset, which for a rocket in flight is the
 * direction it is travelling, so it can be pointed at what it is about to hit.
 */
function drawGem(ctx, x, y, radius, colorIndex, special, timeMs, dx = 0, dy = 0) {
  const gem = PALETTE[colorIndex % PALETTE.length];

  if (special === Special.ROCKET) {
    drawRocket(ctx, x, y, radius, gem, dx, dy);
    return;
  }

  ctx.save();
  shapePath(ctx, gem.shape, x, y, radius);
  ctx.fillStyle = gem.fill;
  ctx.fill();
  ctx.strokeStyle = gem.edge;
  ctx.lineWidth = Math.max(1, radius * 0.12);
  ctx.stroke();

  // A soft highlight up and to the left reads as a facet without costing a
  // gradient object per gem per frame.
  ctx.clip();
  ctx.fillStyle = 'rgba(255,255,255,0.28)';
  ctx.beginPath();
  ctx.ellipse(x - radius * 0.28, y - radius * 0.34, radius * 0.5, radius * 0.3, -0.6, 0, TAU);
  ctx.fill();
  ctx.restore();

  drawSpecial(ctx, x, y, radius, special, timeMs);
}

function drawSpecial(ctx, x, y, radius, special, timeMs) {
  if (special === Special.NONE) {
    return;
  }
  ctx.save();
  ctx.lineCap = 'round';

  if (special === Special.LINE_H || special === Special.LINE_V) {
    const horizontal = special === Special.LINE_H;
    ctx.strokeStyle = 'rgba(255,255,255,0.92)';
    ctx.lineWidth = Math.max(2, radius * 0.16);
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
    ctx.strokeStyle = 'rgba(255,255,255,0.92)';
    ctx.lineWidth = Math.max(2, radius * 0.16);
    ctx.beginPath();
    ctx.moveTo(x - radius * 0.8, y);
    ctx.lineTo(x + radius * 0.8, y);
    ctx.moveTo(x, y - radius * 0.8);
    ctx.lineTo(x, y + radius * 0.8);
    ctx.stroke();
  } else if (special === Special.RAINBOW) {
    // Slowly turning wedges, so it reads as the wildcard at a glance.
    const spin = (timeMs / 1400) % TAU;
    for (let i = 0; i < PALETTE.length; i += 1) {
      ctx.beginPath();
      ctx.moveTo(x, y);
      ctx.arc(x, y, radius * 0.66, spin + (i * TAU) / PALETTE.length, spin + ((i + 1) * TAU) / PALETTE.length);
      ctx.closePath();
      ctx.fillStyle = PALETTE[i].fill;
      ctx.fill();
    }
    ctx.fillStyle = 'rgba(255,255,255,0.92)';
    ctx.beginPath();
    ctx.arc(x, y, radius * 0.2, 0, TAU);
    ctx.fill();
  }

  ctx.restore();
}

/**
 * A rocket, nosed toward wherever it is heading. A freshly made one has no
 * travel yet, so it sits pointing up until it launches.
 */
function drawRocket(ctx, x, y, r, gem, dx, dy) {
  const travelling = Math.abs(dx) > 0.001 || Math.abs(dy) > 0.001;
  const angle = travelling ? Math.atan2(dy, dx) + Math.PI / 2 : 0;

  ctx.save();
  ctx.translate(x, y);
  ctx.rotate(angle);

  if (travelling) {
    // A short exhaust trailing the nose.
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
  }

  // Fins, then the body over them.
  ctx.fillStyle = gem.edge;
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
  ctx.fillStyle = gem.fill;
  ctx.fill();
  ctx.strokeStyle = 'rgba(255,255,255,0.92)';
  ctx.lineWidth = Math.max(1.5, r * 0.12);
  ctx.stroke();

  // A porthole, so it reads as a rocket rather than an arrow.
  ctx.beginPath();
  ctx.arc(0, -r * 0.18, r * 0.2, 0, TAU);
  ctx.fillStyle = 'rgba(255,255,255,0.92)';
  ctx.fill();

  ctx.restore();
}

function shapePath(ctx, shape, x, y, r) {
  ctx.beginPath();
  switch (shape) {
    case 'circle':
      ctx.arc(x, y, r, 0, TAU);
      break;
    case 'diamond':
      ctx.moveTo(x, y - r);
      ctx.lineTo(x + r * 0.82, y);
      ctx.lineTo(x, y + r);
      ctx.lineTo(x - r * 0.82, y);
      ctx.closePath();
      break;
    case 'square':
      roundRect(ctx, x - r * 0.84, y - r * 0.84, r * 1.68, r * 1.68, r * 0.3);
      break;
    case 'triangle':
      polygon(ctx, x, y + r * 0.12, r * 1.06, 3, -Math.PI / 2);
      break;
    case 'hex':
      polygon(ctx, x, y, r, 6, 0);
      break;
    case 'pentagon':
      polygon(ctx, x, y, r, 5, -Math.PI / 2);
      break;
    case 'star':
      star(ctx, x, y, r, r * 0.46, 5);
      break;
    case 'drop':
      ctx.moveTo(x, y - r);
      ctx.quadraticCurveTo(x + r, y - r * 0.2, x + r * 0.62, y + r * 0.45);
      ctx.quadraticCurveTo(x, y + r * 1.1, x - r * 0.62, y + r * 0.45);
      ctx.quadraticCurveTo(x - r, y - r * 0.2, x, y - r);
      ctx.closePath();
      break;
    default:
      ctx.arc(x, y, r, 0, TAU);
  }
}

function polygon(ctx, x, y, r, sides, rotation) {
  for (let i = 0; i < sides; i += 1) {
    const angle = rotation + (i * TAU) / sides;
    const px = x + Math.cos(angle) * r;
    const py = y + Math.sin(angle) * r;
    if (i === 0) {
      ctx.moveTo(px, py);
    } else {
      ctx.lineTo(px, py);
    }
  }
  ctx.closePath();
}

function star(ctx, x, y, outer, inner, points) {
  for (let i = 0; i < points * 2; i += 1) {
    const radius = i % 2 === 0 ? outer : inner;
    const angle = -Math.PI / 2 + (i * Math.PI) / points;
    const px = x + Math.cos(angle) * radius;
    const py = y + Math.sin(angle) * radius;
    if (i === 0) {
      ctx.moveTo(px, py);
    } else {
      ctx.lineTo(px, py);
    }
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
