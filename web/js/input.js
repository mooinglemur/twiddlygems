// Pointer input: swipe a gem toward its neighbor, or tap one and then tap the
// neighbor. Both gestures are wired to the same swap, so the game plays the
// same with a thumb or a mouse.

const SWIPE_FRACTION = 0.3;

/**
 * `aimed` is asked about a cell before anything else happens to it, and
 * answers whether it took the tap. It is how something armed out of the
 * inventory is spent: the player picked the item, and this tap is them
 * picking the cell.
 */
export function attachInput(canvas, renderer, engine, onAction, aimed = null) {
  let drag = null;

  const begin = (event) => {
    if (!engine.acceptsInput) {
      return;
    }
    const cell = renderer.cellFromPoint(event.clientX, event.clientY);
    if (!cell) {
      return;
    }
    // Spent where it was pointed, and no drag behind it: something aimed goes
    // at one cell rather than being swiped between two, and a swap started
    // from under it would be a move the player never meant to make.
    if (aimed && aimed(cell)) {
      event.preventDefault();
      return;
    }
    drag = { id: event.pointerId, cell, x: event.clientX, y: event.clientY, swiped: false };
    if (canvas.setPointerCapture) {
      canvas.setPointerCapture(event.pointerId);
    }
    event.preventDefault();
  };

  const move = (event) => {
    if (!drag || drag.id !== event.pointerId || drag.swiped) {
      return;
    }
    const dx = event.clientX - drag.x;
    const dy = event.clientY - drag.y;
    const threshold = renderer.cell * SWIPE_FRACTION;
    if (Math.abs(dx) < threshold && Math.abs(dy) < threshold) {
      return;
    }

    // Whichever axis the thumb committed to first wins; diagonals are not moves.
    const target =
      Math.abs(dx) > Math.abs(dy)
        ? { r: drag.cell.r, c: drag.cell.c + Math.sign(dx) }
        : { r: drag.cell.r + Math.sign(dy), c: drag.cell.c };

    drag.swiped = true;
    engine.clearSelection();
    if (engine.swap(drag.cell.r, drag.cell.c, target.r, target.c)) {
      onAction('swap');
    }
    event.preventDefault();
  };

  const end = (event) => {
    if (!drag || drag.id !== event.pointerId) {
      return;
    }
    if (!drag.swiped) {
      // A press that never traveled is a tap on the cell it started in.
      const outcome = engine.tap(drag.cell.r, drag.cell.c);
      if (outcome !== 0) {
        onAction(outcome === 3 ? 'swap' : 'select');
      }
    }
    drag = null;
  };

  const cancel = () => {
    drag = null;
  };

  canvas.addEventListener('pointerdown', begin);
  canvas.addEventListener('pointermove', move);
  canvas.addEventListener('pointerup', end);
  canvas.addEventListener('pointercancel', cancel);
  // A context menu mid-drag would otherwise leave the gesture half finished.
  canvas.addEventListener('contextmenu', (event) => event.preventDefault());

  return () => {
    canvas.removeEventListener('pointerdown', begin);
    canvas.removeEventListener('pointermove', move);
    canvas.removeEventListener('pointerup', end);
    canvas.removeEventListener('pointercancel', cancel);
  };
}
