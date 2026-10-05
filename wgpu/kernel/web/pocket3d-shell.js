// The page's side of a Pocket3D game drawn with wgpu in a browser tab: what
// every such game's page needs around its own renderer, and nothing of a game.
// Beside this module: the interface's realm (pocket3d-interface.js), a
// handheld's controls (pocket3d-controls.js), its screens on the page
// (pocket3d-stage.js).
//
//   import { playTitle } from "./pocket3d-title.js";
//   import { hasWebGPU, frames } from "./pocket3d-shell.js";
//
//   const title = playTitle();          // the Pocket3D title card, before any other picture
//   if (!hasWebGPU()) { await title; say("…"); return; }
//   const game = await loadTheGame(canvas);
//   await title;
//   frames(() => 60, (now) => game.frame(now));
//
// The title card is PocketJS's (engine/pocket3d/crates/pocket3d-title/web): a
// page plays it first and does not skip, shorten, recolour or redraw it.

/** Whether this browser can give a page a WebGPU device. */
export function hasWebGPU() {
  return typeof navigator !== "undefined" && "gpu" in navigator;
}

/**
 * The frame loop: calls `frame(now)` about `hz()` times a second while the tab
 * is shown, each frame on a refresh of the display and shown for a whole
 * number of them. The display's rate is measured from its refreshes: at 60 a
 * second a game that asks for 60 gets every refresh and one that asks for 30
 * every other; at 120 they get every second and every fourth. A display whose
 * rate is no multiple of the one asked for gives the nearest rate that is
 * (72 a second for 60 on a display of 144), so a game takes the time between
 * two frames from `now`, not from `hz()`. `hz` is asked before each frame:
 * the rate can change while the loop runs. Returns `{ stop() }`.
 */
export function frames(hz, frame) {
  let request = 0;
  let before = 0;
  // Milliseconds from one refresh of the display to the next, and refreshes since the last frame.
  let refresh = 0;
  let since = Infinity;
  const tick = (now) => {
    request = requestAnimationFrame(tick);
    const step = now - before;
    before = now;
    // (a refresh that came late, or the first after the tab was hidden, says nothing of the display's rate)
    if (step > 2 && step < 50) refresh = refresh ? refresh + (step - refresh) * 0.1 : step;
    if (++since < Math.max(1, Math.round(1000 / hz() / (refresh || 1000 / 60)))) return;
    since = 0;
    frame(now);
  };
  request = requestAnimationFrame(tick);
  return { stop: () => cancelAnimationFrame(request) };
}
