// The page's side of a Pocket3D game drawn with wgpu in a browser tab: what
// every such game's page needs around its own renderer, and nothing of a game.
//
//   import { playTitle } from "./pocket3d-title.js";
//   import { hasWebGPU, fit, frames, keys, drags } from "./pocket3d-shell.js";
//
//   const title = playTitle();          // the Pocket3D title card, before any other picture
//   if (!hasWebGPU()) { await title; say("…"); return; }
//   const game = await loadTheGame(canvas);
//   await title;
//   frames(() => 60, (now) => game.frame(now, …));
//
// The title card is PocketJS's (engine/pocket3d/crates/pocket3d-title/web): a
// page plays it first and does not skip, shorten, recolour or redraw it.

/** Whether this browser can give a page a WebGPU device. */
export function hasWebGPU() {
  return typeof navigator !== "undefined" && "gpu" in navigator;
}

/**
 * Sizes a canvas of `width` by `height` pixels on the page: the largest whole
 * multiple of its pixels that fits `room` (an element), or the fraction that
 * fits when one does not. Returns the scale.
 */
export function fit(canvas, width, height, room = document.documentElement) {
  const most = Math.min(room.clientWidth / width, room.clientHeight / height);
  const scale = most >= 1 ? Math.floor(most) : most;
  canvas.style.width = `${Math.round(width * scale)}px`;
  canvas.style.height = `${Math.round(height * scale)}px`;
  return scale;
}

/**
 * The frame loop: calls `frame(now)` at most `hz()` times a second, in step
 * with the display, while the tab is shown. `hz` is asked before each frame,
 * so the rate can change while the loop runs. Returns `{ stop() }`.
 */
export function frames(hz, frame) {
  let last = -Infinity;
  let request = 0;
  const tick = (now) => {
    request = requestAnimationFrame(tick);
    const interval = 1000 / hz();
    // (a display refresh comes a little early as often as a little late)
    if (now - last < interval - 2) return;
    // Frames keep to the display's refreshes: a late one does not move the ones after it.
    last = now - last > interval * 4 ? now : last + Math.round((now - last) / interval) * interval;
    frame(now);
  };
  request = requestAnimationFrame(tick);
  return { stop: () => cancelAnimationFrame(request) };
}

/**
 * The keys held on `target`, by `KeyboardEvent.code`. `held(code)` is 1 or 0;
 * `axis(less, more)` is -1, 0 or 1. Every key is let go when the page loses
 * the keyboard.
 */
export function keys(target = window, { own = [] } = {}) {
  const down = new Set();
  const mine = new Set(own);
  target.addEventListener("keydown", (event) => {
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    down.add(event.code);
    // (the page does not scroll under a key the game reads)
    if (mine.has(event.code)) event.preventDefault();
  });
  target.addEventListener("keyup", (event) => down.delete(event.code));
  window.addEventListener("blur", () => down.clear());
  const held = (...codes) => (codes.some((code) => down.has(code)) ? 1 : 0);
  return { held, axis: (less, more) => held(...[more].flat()) - held(...[less].flat()), any: () => down.size > 0 };
}

/**
 * A pointer dragging on `element`: `take()` returns how far it has moved
 * since the last call, in the element's own units of `width` across.
 */
export function drags(element, width) {
  let dx = 0;
  let dy = 0;
  let held = null;
  element.addEventListener("pointerdown", (event) => {
    held = event.pointerId;
    element.setPointerCapture(held);
  });
  element.addEventListener("pointermove", (event) => {
    if (event.pointerId !== held) return;
    const scale = width() / element.clientWidth;
    dx += event.movementX * scale;
    dy += event.movementY * scale;
  });
  const lift = (event) => {
    if (event.pointerId === held) held = null;
  };
  element.addEventListener("pointerup", lift);
  element.addEventListener("pointercancel", lift);
  return {
    take() {
      const moved = [dx, dy];
      dx = dy = 0;
      return moved;
    },
  };
}
