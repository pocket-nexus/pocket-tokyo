// Pocket Tokyo in a browser tab: the page around the wgpu renderer (../src,
// built to pkg/). The Pocket3D title card plays first; the pack is read while
// it plays. The keys are the handhelds' pad:
//
//   W A S D   fly (ahead, back, left, right)      E, Q       climb, descend
//   arrows    look; a drag turns the view         Shift      faster
//   [ ]       the clock, earlier and later        T          the tour takes the eye, or hands it back
//
// The address chooses the screen and the pack:
//
//   ?shape=psp|vita|3ds|ipod   a handheld's screen, frames a second and triangle budget
//   ?size=960x544 &samples=4 &budget=200000 &hz=60   one of them changed
//   ?pack=URL                  the pack (an iPod touch pack; the server answers byte ranges)
//   ?words=hour=19+rate=0      what a development host would send the flight
//
// `window.pocketTokyo` is the running city, for a console and for tools/wgpu.ts.
import { playTitle } from "./pocket3d-title.js";
import { drags, fit, frames, hasWebGPU, keys } from "./pocket3d-shell.js";
import init, { Tokyo, shapes } from "./pkg/tokyo_wgpu.js";

// tokyo_sim::camera::btn, tokyo_sim::flight::key and tokyo_interface::pad::MENU.
const BTN = { FAST: 1, UP: 2, DOWN: 4 };
const KEY = { TOUR: 1 << 8, LATER: 1 << 10, EARLIER: 1 << 11, MENU: 1 << 16 };
const OWN = ["KeyW", "KeyA", "KeyS", "KeyD", "KeyE", "KeyQ", "KeyT", "Space", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "BracketLeft", "BracketRight", "Enter"];

const query = new URLSearchParams(location.search);
const canvas = document.getElementById("city");
const room = document.getElementById("room");
const say = (text) => (document.getElementById("say").textContent = text);
const number = (name) => Math.max(0, Number.parseInt(query.get(name) ?? "0", 10) || 0);
const started = performance.now();

async function start() {
  // The card is the first picture of every launch, and covers the page while the city is read.
  const title = playTitle();
  if (!hasWebGPU()) {
    await title;
    say("This browser has no WebGPU, which Pocket Tokyo draws with.");
    return;
  }
  await init();
  const all = JSON.parse(shapes());
  const named = all.find((s) => s.name === query.get("shape")) ?? all.find((s) => s.name === "vita");
  const [width, height] = (query.get("size") ?? "").split("x").map((n) => Number.parseInt(n, 10) || 0);
  let shape = { ...named, width: width || named.width, height: height || named.height };
  canvas.width = shape.width;
  canvas.height = shape.height;
  const tokyo = await Tokyo.start(canvas, new URL(query.get("pack") ?? "city.pack", location.href).href, shape.name);
  shape = JSON.parse(tokyo.reshape(shape.name, shape.width, shape.height, number("samples"), number("budget"), number("hz")));
  if (query.get("words")) tokyo.control(query.get("words"));
  const place = () => fit(canvas, shape.width, shape.height, room);
  addEventListener("resize", place);

  const pad = keys(window, { own: OWN });
  const drag = drags(canvas, () => 480);
  const report = { tokyo, shape: () => shape, firstFrame: 0, frames: 0, failure: "" };
  window.pocketTokyo = report;
  await title;
  canvas.hidden = false;
  place();
  const loop = frames(() => shape.hz, (now) => {
    const [dx, dy] = drag.take();
    if (dx || dy) tokyo.look(dx, dy);
    const buttons = pad.held("ShiftLeft", "ShiftRight") * BTN.FAST | pad.held("KeyE", "Space") * BTN.UP | pad.held("KeyQ") * BTN.DOWN;
    const held = pad.held("KeyT") * KEY.TOUR | pad.held("BracketRight") * KEY.LATER | pad.held("BracketLeft") * KEY.EARLIER | pad.held("Enter") * KEY.MENU;
    try {
      tokyo.frame(now, buttons, held, pad.axis("KeyA", "KeyD"), pad.axis("KeyS", "KeyW"), pad.axis("ArrowLeft", "ArrowRight"), 0.7 * pad.axis("ArrowDown", "ArrowUp"));
    } catch (error) {
      // A frame the canvas had no texture for is skipped; anything else stops the city and says why.
      report.failure = String(error?.message ?? error);
      if (!/Outdated|Lost|Timeout/.test(report.failure)) {
        loop.stop();
        say(report.failure);
      }
      return;
    }
    report.frames++;
    report.firstFrame ||= performance.now() - started;
  });
  // Another screen while the city flies: `pocketTokyo.reshape("psp")`, or with numbers of one's own.
  report.reshape = (name, { width = 0, height = 0, samples = 0, budget = 0, hz = 0 } = {}) => {
    const next = all.find((s) => s.name === name);
    if (!next) throw new Error(`no shape "${name}"`);
    canvas.width = width || next.width;
    canvas.height = height || next.height;
    shape = JSON.parse(tokyo.reshape(name, canvas.width, canvas.height, samples, budget, hz));
    place();
    return shape;
  };
}

start().catch((error) => {
  window.pocketTokyo = { failure: String(error?.message ?? error) };
  say(window.pocketTokyo.failure);
});
