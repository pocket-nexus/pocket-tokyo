// Pocket Tokyo in a browser tab: the wgpu renderer (../src, built to pkg/)
// and the game's own interface (../../ui, one bundle a device, compiled by
// tools/ui.ts), which a realm of the page runs on PocketJS's UI core and the
// renderer lays over the city. The page itself is the Pocket3D player of
// PocketJS's browser kernel (vendor/pocketjs/devices/web/pocket-web-wgpu,
// staged beside this file): the bar, the device's shell with its keys, the
// way to Pocket Studio. This file says what the game is and draws into the
// player's canvas.
//
// The page shows one of the handhelds the city runs on: its shell, its
// screens, its presentation of the interface, its buttons. The Pocket3D title
// card plays first; the interface and the head of the pack are read while it
// plays, and the blocks' pictures of the ground after the first frame.
//
// The address chooses the device and the pack:
//
//   ?device=vita|psp|3ds|ipod   the handheld (without it: an iPod touch for a finger, a PS Vita otherwise)
//   ?size=960x544 &samples=4 &budget=200000 &hz=60   the first device's screen, changed
//   ?pack=URL                   the pack (an iPod touch pack): its file, on a server that answers byte
//                               ranges, or the manifest (.json) of one cut into pieces. Without it, the
//                               page's own (<meta name="pocket-pack">)
//   ?words=hour=19+rate=0       what a development host would send the flight
//   ?sweep=here                 the shadows swept in the frames, not in a worker
//   ?interface=off              the city alone, flown by the device's pad: no guest is started
//
// `window.pocketTokyo` is the running city, for a console and for tools/wgpu.ts.
import { playTitle } from "./pocket3d-title.js";
import { frames, hasWebGPU, titleCard } from "./pocket3d-shell.js";
import { openInterface, screens } from "./pocket3d-interface.js";
import { createPlayer } from "./pocket3d-player.js";
import init, { Tokyo, shapes } from "./pkg/tokyo_wgpu.js";

// The handhelds: each one's screen is the renderer's shape of the same name, its interface the bundle
// under ui/<id>/, and `sticks` says how it is flown (README, "Controls"): two sticks, one stick and two
// buttons to look up and down, or the interface's own stick on a touch panel. `note` is what the player
// says beside the device's name: how this picture differs from the one that device's own build draws
// (README, the table of devices).
const DEVICES = [
  { id: "vita", label: "PS Vita", sticks: 2, note: "On a PS Vita the city has about three times the triangles, traffic on its streets and a glow at night. This page draws the iPod touch build's city." },
  { id: "psp", label: "PSP", sticks: 1, note: "On a PSP the edges are hard and the light is plainer. This page draws the iPod touch build's city, with smoothed edges." },
  { id: "3ds", label: "Nintendo 3DS", sticks: 1, note: "On a 3DS the edges are hard and the light is plainer. This page draws the iPod touch build's city, with smoothed edges." },
  { id: "ipod", label: "iPod touch", sticks: 0, note: "This page draws the iPod touch build's own pack and passes. On an iPod touch 4 the colours are less precise, and a few frames in a thousand come late." },
];
// Where the interface's settings are kept between visits (a device keeps them in a file).
const KEPT = "pocket-tokyo.interface";

const query = new URLSearchParams(location.search);
const number = (name) => Math.max(0, Number.parseInt(query.get(name) ?? "0", 10) || 0);
const beside = (name) => new URL(name, import.meta.url).href;
const message = (error) => String(error?.message ?? error);
const coarse = matchMedia("(pointer: coarse)").matches;
const started = performance.now();

// The page: PocketJS's player, with the device the address asks for, or an iPod touch under a finger.
const wanted = query.get("device") ?? query.get("shape");
let device = DEVICES.find((d) => d.id === wanted) ?? DEVICES.find((d) => d.id === (coarse ? "ipod" : "vita"));
let present = () => {};
const player = createPlayer({
  title: "Pocket Tokyo",
  tagline: "A flight over Shiba, around Tokyo Tower.",
  devices: DEVICES,
  device: device.id,
  // (the targets `bun tools/release.ts` builds a package for)
  runsOn: ["psp", "vita", "3ds", "ipod-touch", "android"],
  pick: (id) => present(DEVICES.find((d) => d.id === id)),
});
const { canvas, stage, controls } = player;
const say = (text) => player.say(text);

function kept() {
  try {
    return localStorage.getItem(KEPT) ?? "";
  } catch {
    return "";
  }
}

async function start() {
  // The card is the first picture of every launch, and covers the page while the city is read. No frame is
  // drawn while it plays; the blocks' pictures are read from the moment the city flies.
  let pump = () => {};
  const title = titleCard(playTitle, () => pump());
  if (!hasWebGPU()) {
    await title;
    say("This browser has no WebGPU, which Pocket Tokyo draws with.");
    return;
  }
  await init();
  const all = JSON.parse(shapes());

  // The shell, on the first device's screen. It draws before there is a city: the interface says what is read.
  const [width, height] = (query.get("size") ?? "").split("x").map((n) => Number.parseInt(n, 10) || 0);
  const first = all.find((s) => s.name === device.id);
  stage.show({ device: device.id, width: width || first.width, height: height || first.height });
  const tokyo = await Tokyo.open(canvas, first.name, kept());
  let shape = JSON.parse(tokyo.reshape(first.name, canvas.width, canvas.height, number("samples"), number("budget"), number("hz")));
  // (the flow waits at the title for a guest: one is on its way)
  tokyo.interface_opened();
  pump = () => tokyo.pump();

  // What a frame's parts cost, in milliseconds summed since the start: the guest's turns, the redraws of
  // its picture (`redrawMs`: the UI core's drawing, `drawMs` of it, then the upload), and the second
  // screen's redraws.
  const timing = { turns: 0, turnMs: 0, redraws: 0, redrawMs: 0, drawMs: 0, lowers: 0, lowerMs: 0 };
  // (milliseconds from the page's start: the city read, the interface up, the first frame, the first of the city)
  const report = { tokyo, device: () => device.id, shape: () => shape, ready: 0, interfaceReady: 0, firstFrame: 0, firstCity: 0, frames: 0, failure: "", timing };
  window.pocketTokyo = report;

  // The city, read beside everything else. The shadows are swept in a worker; a browser that starts none
  // sweeps them in the frames.
  const pack = query.get("pack") ?? document.querySelector('meta[name="pocket-pack"]').content;
  tokyo.reader().read(new URL(pack, location.href).href).then((city) => {
    let sweeper;
    try {
      sweeper = query.get("sweep") === "here" ? undefined : new Worker(beside("sweep.js"), { type: "module" });
    } catch {
      sweeper = undefined;
    }
    tokyo.fly(city, sweeper);
    if (query.get("words")) tokyo.control(query.get("words"));
    report.ready = performance.now() - started;
  }).catch((error) => {
    report.failure = message(error);
    tokyo.fail(report.failure);
    if (!ui) say(report.failure);
  });

  // A device on the page: its screens, its controls, and its presentation of the interface in a new realm.
  let ui = null;
  let lower = null;
  present = async (next, sized) => {
    device = next;
    const plan = await (await fetch(beside(`ui/${next.id}/plan.json`))).json();
    if (device !== next) return;
    const of = screens(plan);
    const to = sized ?? all.find((s) => s.name === next.id);
    // The device's shell with its screens in it, its keys as the controls, and the screen that takes touch.
    player.show(next.id, { width: to.width, height: to.height, lower: of.auxiliary, sticks: next.sticks, glyphs: of.glyphs, touch: of.touch, viewport: of.viewport });
    shape = JSON.parse(tokyo.reshape(to.name, to.width, to.height, to.samples, to.budget, to.hz));
    // (the turns a second the device's own host gives its interface)
    const simHz = shape.turns;
    lower = of.auxiliary ? new ImageData(of.auxiliary[0], of.auxiliary[1]) : null;
    // The guest of the device before goes with its realm; the new one is told the whole state on its first turn.
    ui?.close();
    ui = null;
    tokyo.overlay_hide();
    if (query.get("interface") === "off") return tokyo.control("mode=flight");
    try {
      const opened = await openInterface({ realm: beside("app-instance.html"), wasm: beside("pocketjs.wasm"), bundle: beside(`ui/${next.id}/tokyo.js`), pak: beside(`ui/${next.id}/tokyo.pak`), plan, simHz });
      // (another device was chosen while this one's interface was read)
      if (device !== next) return opened.close();
      tokyo.interface_opened();
      ui = opened;
      report.interfaceReady ||= performance.now() - started;
    } catch (error) {
      // Without its interface the city is flown by the pad alone.
      report.failure = message(error);
      tokyo.control("mode=flight");
    }
  };
  const presented = present(device, { ...shape });

  // One frame: the flight, the guest's turn when it is worth one, its picture when that has changed, the scene.
  const frame = (now) => {
    const held = controls.read();
    tokyo.step(now, held.buttons, held.left[0], held.left[1], held.right[0], held.right[1]);
    // (a turn is offered as often as the device offers one, and is that many sixtieths of a second)
    const ticks = ui !== null ? tokyo.guest_due(held.touching) : 0;
    const turned = ticks > 0;
    if (turned) {
      const from = performance.now();
      const line = tokyo.heard();
      if (line) ui.send(line);
      ui.turn(tokyo.guest_buttons(), held.contacts, ticks);
      for (const said of ui.drain()) tokyo.say(said);
      const turnedAt = performance.now();
      if (ui.changed()) {
        // One drawing by the UI core, with its alpha, and the upload.
        const picture = ui.picture();
        const drew = performance.now();
        tokyo.overlay(picture.pixels, picture.width, picture.height);
        timing.redraws++;
        timing.drawMs += drew - turnedAt;
        timing.redrawMs += performance.now() - turnedAt;
      }
      const drawn = performance.now();
      if (lower && ui.lowerChanged()) {
        // The second screen is the interface's alone: its pixels go to its canvas as they are.
        lower.data.set(ui.lower());
        stage.lower.putImageData(lower, 0, 0);
        timing.lowers++;
        timing.lowerMs += performance.now() - drawn;
      }
      timing.turns++;
      timing.turnMs += turnedAt - from;
    }
    controls.next(turned);
    const settings = tokyo.prefs_take();
    if (settings) {
      try {
        localStorage.setItem(KEPT, settings);
      } catch {
        // A browser that keeps nothing starts from the defaults next time.
      }
    }
    tokyo.draw();
    report.frames++;
    report.firstFrame ||= performance.now() - started;
    if (!report.firstCity && tokyo.flies()) {
      report.firstCity = performance.now() - started;
      // (the city is on the screen: the player may read what it kept back)
      player.ready();
    }
  };

  await title;
  pump = () => {};
  canvas.hidden = false;
  stage.fit();
  const loop = frames(() => shape.hz, (now) => {
    try {
      frame(now);
    } catch (error) {
      // A frame the canvas had no texture for is skipped; anything else stops the city and says why.
      report.failure = message(error);
      if (!/Outdated|Lost|Timeout/.test(report.failure)) {
        loop.stop();
        say(report.failure);
      }
    }
  });

  // For a console and for tools/wgpu.ts.
  report.presented = () => presented;
  // Another device while the city flies: `pocketTokyo.present("psp")`.
  report.present = (id) => present(DEVICES.find((d) => d.id === id));
  report.interface = () => ui;
  // The frame as the canvases hold it, one pixel of the city to a pixel: PNGs as data URLs.
  report.capture = () => {
    frame(performance.now());
    return { upper: canvas.toDataURL("image/png"), lower: stage.second.hidden ? null : stage.second.toDataURL("image/png") };
  };
  // Milliseconds a frame costs the processor and the GPU together, over `count` frames made without
  // waiting for the display: the flight, the guest's turns and redraws, the scene.
  report.burst = async (count = 300) => {
    const gpu = canvas.getContext("webgpu").getConfiguration().device;
    const from = performance.now();
    for (let i = 0; i < count; i++) frame(from + ((i + 1) * 1000) / shape.hz);
    await gpu.queue.onSubmittedWorkDone();
    return (performance.now() - from) / count;
  };
}

start().catch((error) => {
  window.pocketTokyo = { failure: message(error) };
  say(window.pocketTokyo.failure);
});
