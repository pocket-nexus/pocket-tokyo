// Pocket Tokyo in a browser tab: the page around the wgpu renderer (../src,
// built to pkg/) and the game's own interface (../../ui, one bundle a device,
// compiled by tools/ui.ts), which a realm of the page runs on PocketJS's UI
// core and the renderer lays over the city.
//
// The page shows one of the handhelds the city runs on: its screens at their
// own size, its presentation of the interface, its buttons. The Pocket3D title
// card plays first; the interface and the head of the pack are read while it
// plays, and the blocks' pictures of the ground after the first frame.
//
// The address chooses the device and the pack:
//
//   ?device=vita|psp|3ds|ipod   the handheld (without it: an iPod touch for a finger, a PS Vita otherwise)
//   ?buttons                    the device's buttons on the page, also where there is a keyboard
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
import { frames, hasWebGPU } from "./pocket3d-shell.js";
import { openInterface, screens } from "./pocket3d-interface.js";
import { createControls, legend } from "./pocket3d-controls.js";
import { choices, createStage } from "./pocket3d-stage.js";
import init, { Tokyo, shapes } from "./pkg/tokyo_wgpu.js";

// The handhelds: each one's screen is the renderer's shape of the same name, its interface the bundle
// under ui/<id>/, and `sticks` says how it is flown (README, "Controls"): two sticks, one stick and two
// buttons to look up and down, or the interface's own stick on a touch panel.
const DEVICES = [
  { id: "vita", label: "PS Vita", sticks: 2 },
  { id: "psp", label: "PSP", sticks: 1 },
  { id: "3ds", label: "Nintendo 3DS", sticks: 1 },
  { id: "ipod", label: "iPod touch", sticks: 0 },
];
// Where the interface's settings are kept between visits (a device keeps them in a file).
const KEPT = "pocket-tokyo.interface";

const query = new URLSearchParams(location.search);
const canvas = document.getElementById("city");
const say = (text) => (document.getElementById("say").textContent = text);
const number = (name) => Math.max(0, Number.parseInt(query.get(name) ?? "0", 10) || 0);
const beside = (name) => new URL(name, import.meta.url).href;
const message = (error) => String(error?.message ?? error);
const coarse = matchMedia("(pointer: coarse)").matches;
const started = performance.now();

function kept() {
  try {
    return localStorage.getItem(KEPT) ?? "";
  } catch {
    return "";
  }
}

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
  const wanted = query.get("device") ?? query.get("shape");
  let device = DEVICES.find((d) => d.id === wanted) ?? DEVICES.find((d) => d.id === (coarse ? "ipod" : "vita"));
  const stage = createStage(document.getElementById("stage"), canvas);
  const controls = createControls();

  // The shell, on the first device's screen. It draws before there is a city: the interface says what is read.
  const [width, height] = (query.get("size") ?? "").split("x").map((n) => Number.parseInt(n, 10) || 0);
  const first = all.find((s) => s.name === device.id);
  stage.show({ width: width || first.width, height: height || first.height });
  const tokyo = await Tokyo.open(canvas, first.name, kept());
  let shape = JSON.parse(tokyo.reshape(first.name, canvas.width, canvas.height, number("samples"), number("budget"), number("hz")));
  // (the flow waits at the title for a guest: one is on its way)
  tokyo.interface_opened();

  // What a frame's parts cost, in milliseconds summed since the start: the guest's turns, and the redraws
  // of its picture (`redrawMs`: the UI core's two drawings, `drawMs` of it, then what they cover and the
  // upload), and the second screen's.
  const timing = { turns: 0, turnMs: 0, redraws: 0, redrawMs: 0, drawMs: 0, lowerMs: 0 };
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
  const picker = choices(document.getElementById("devices"), DEVICES, device.id, (id) => present(DEVICES.find((d) => d.id === id)));
  async function present(next, sized) {
    device = next;
    picker.set(next.id);
    const plan = await (await fetch(beside(`ui/${next.id}/plan.json`))).json();
    if (device !== next) return;
    const of = screens(plan);
    const to = sized ?? all.find((s) => s.name === next.id);
    stage.show({ width: to.width, height: to.height, lower: of.auxiliary });
    shape = JSON.parse(tokyo.reshape(to.name, to.width, to.height, to.samples, to.budget, to.hz));
    controls.device({ sticks: next.sticks, glyphs: of.glyphs });
    if (coarse || query.has("buttons")) controls.buttonsIn(stage.left, stage.right);
    document.getElementById("stage").toggleAttribute("data-stacked", innerHeight > innerWidth);
    stage.fit();
    controls.touch(of.touch === "primary" ? canvas : of.touch === "auxiliary" ? stage.second : null, of.touch === "auxiliary" ? of.auxiliary : of.viewport);
    lower = of.auxiliary ? new ImageData(of.auxiliary[0], of.auxiliary[1]) : null;
    const keys = legend({ sticks: next.sticks, glyphs: of.glyphs }).map(([key, what]) => `${key}: ${what}`);
    const pointer = of.touch === "auxiliary" ? ["the pointer is a stylus on the lower screen"] : of.touch === "primary" ? [next.sticks ? "the screen takes taps" : "The pointer is a finger on the screen"] : [];
    // (a browser whose pointer is a finger has no keys to be told of, and knows what its finger is)
    document.getElementById("keys").textContent = coarse ? "" : [...keys, ...pointer].join(" · ");
    // The guest of the device before goes with its realm; the new one is told the whole state on its first turn.
    ui?.close();
    ui = null;
    tokyo.overlay_hide();
    if (query.get("interface") === "off") return tokyo.control("mode=flight");
    try {
      const opened = await openInterface({ realm: beside("pocket3d-realm.html"), wasm: beside("pocketjs.wasm"), bundle: beside(`ui/${next.id}/tokyo.js`), pak: beside(`ui/${next.id}/tokyo.pak`), plan });
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
  }
  addEventListener("resize", () => {
    document.getElementById("stage").toggleAttribute("data-stacked", innerHeight > innerWidth);
    stage.fit();
  });
  const presented = present(device, { ...shape });

  // One frame: the flight, the guest's turn when it is worth one, its picture when that has changed, the scene.
  const frame = (now) => {
    const held = controls.read();
    const ticks = tokyo.step(now, held.buttons, held.left[0], held.left[1], held.right[0], held.right[1]);
    const turned = ui !== null && tokyo.guest_due(held.buttons, held.touching);
    if (turned) {
      const from = performance.now();
      const line = tokyo.heard();
      if (line) ui.send(line);
      ui.turn(held.buttons, held.contacts, ticks);
      for (const said of ui.drain()) tokyo.say(said);
      const turnedAt = performance.now();
      if (ui.changed()) {
        const picture = ui.pictures();
        tokyo.overlay(picture.black, picture.white, picture.width, picture.height);
        timing.redraws++;
        timing.drawMs += picture.drawMs;
        timing.redrawMs += performance.now() - turnedAt;
      }
      const drawn = performance.now();
      if (lower) {
        // The second screen is the interface's alone: its pixels go to its canvas as they are.
        lower.data.set(ui.lower());
        stage.lower.putImageData(lower, 0, 0);
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
    if (!report.firstCity && tokyo.flies()) report.firstCity = performance.now() - started;
  };

  // While the card plays no frame is drawn; the blocks' pictures are read from the moment the city flies.
  let playing = true;
  const pump = () => {
    if (!playing) return;
    tokyo.pump();
    requestAnimationFrame(pump);
  };
  requestAnimationFrame(pump);
  await title;
  playing = false;
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
