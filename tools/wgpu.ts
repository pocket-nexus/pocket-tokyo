#!/usr/bin/env bun
// Pocket Tokyo drawn with wgpu (wgpu/): in a browser tab over WebGPU, and on
// this machine, where a frame goes to a file.
//
//   bun tools/wgpu.ts cook                         the city for it: the iPod touch's pack (profiles/ipod60.json)
//   bun tools/wgpu.ts build                        wasm32 + wasm-bindgen + the page → .pocket-build/wgpu/site
//   bun tools/wgpu.ts serve [--port 8787]          the site and the pack, with byte ranges
//   bun tools/wgpu.ts dist [--piece 2]             the directory a static host serves → .pocket-build/wgpu/dist:
//                                                  the page, the module under its build's name, and the pack
//                                                  cut into pieces of that many MiB with their manifest
//   bun tools/wgpu.ts serve --dist                 that directory as such a host serves it: no byte ranges
//   bun tools/wgpu.ts shot [--out f.png] [--shape ipod] [--size WxH] [--words "view=… hour=…"] [--against device.png]
//                                                  one frame on this machine's GPU (Metal) → a PNG and the status
//   bun tools/wgpu.ts counts                       the triangles and draws of an eye the iPod touch reported, here
//   bun tools/wgpu.ts check [--headed] [--seconds 5] [--dist]   the tab in Chrome: a frame, frames a second at
//                                                  two sizes, bytes read before the first frame, the keys
//                                                  → .pocket-build/validation/web/
//
// Every command takes [--area shiba] and [--pack PATH] (default: .pocket-build/city/<area>/ipod60/city.pack).
// The packs, the site and the captures stay under the ignored .pocket-build/.

import { $ } from "bun";
import { cpSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { gzipSync } from "node:zlib";
import { join, resolve } from "node:path";
import { POCKET3D_ICON } from "../vendor/pocketjs/tools/pocket3d-icon.ts";

const ROOT = resolve(import.meta.dir, "..");
const CRATE = join(ROOT, "wgpu");
const BUILD = join(ROOT, ".pocket-build/wgpu");
const SITE = join(BUILD, "site");
const DIST = join(BUILD, "dist");
// What the host a build is deployed to allows (Pocket Studio's site deployments): the size of a file, the
// files and the bytes of a deployment, and the top-level names it keeps for itself.
const HOST = { file: 32 << 20, files: 4000, bytes: 1 << 30, reserved: ["play", "runtime"] };
const TITLE = join(ROOT, "vendor/pocketjs/engine/pocket3d/crates/pocket3d-title/web");

const [command, ...rest] = process.argv.slice(2);
const option = (flag: string, fallback = "") => {
  const i = rest.indexOf(flag);
  return i >= 0 && rest[i + 1] ? rest[i + 1]! : fallback;
};
const area = option("--area", "shiba");
const pack = resolve(option("--pack", join(ROOT, `.pocket-build/city/${area}/ipod60/city.pack`)));

function needPack() {
  if (!existsSync(pack)) throw new Error(`${pack} is missing: bun tools/wgpu.ts cook`);
}

/** The wasm module, its JavaScript side and the page, as one directory a static server can serve. */
async function build() {
  // The crate and the command line tool write two halves of one interface: their versions must be the same.
  const lock = readFileSync(join(CRATE, "Cargo.lock"), "utf8").match(/name = "wasm-bindgen"\nversion = "([^"]+)"/)?.[1];
  const tool = (await $`wasm-bindgen --version`.text()).trim().split(" ")[1];
  if (lock !== tool) throw new Error(`wasm-bindgen ${tool} is installed and wgpu/Cargo.lock has ${lock}: cargo install wasm-bindgen-cli --version ${lock}`);
  await $`cargo build --release --lib --target wasm32-unknown-unknown`.cwd(CRATE);
  mkdirSync(join(SITE, "pkg"), { recursive: true });
  await $`wasm-bindgen --target web --no-typescript --out-dir ${join(SITE, "pkg")} ${join(CRATE, "target/wasm32-unknown-unknown/release/tokyo_wgpu.wasm")}`;
  for (const file of ["index.html", "main.js", "sweep.js"]) cpSync(join(CRATE, "page", file), join(SITE, file));
  cpSync(join(CRATE, "kernel/web/pocket3d-shell.js"), join(SITE, "pocket3d-shell.js"));
  // The Pocket3D title card and the icon of the tab, as PocketJS ships them.
  for (const file of ["pocket3d-title.js", "art.js"]) cpSync(join(TITLE, file), join(SITE, file));
  cpSync(POCKET3D_ICON.ios2x, join(SITE, "icon.png"));
  const sizes: Record<string, { bytes: number; gzip: number }> = {};
  for (const file of ["pkg/tokyo_wgpu_bg.wasm", "pkg/tokyo_wgpu.js", "main.js", "sweep.js", "pocket3d-shell.js", "pocket3d-title.js", "art.js", "index.html"]) {
    const bytes = readFileSync(join(SITE, file));
    sizes[file] = { bytes: bytes.length, gzip: gzipSync(bytes, { level: 9 }).length };
  }
  writeFileSync(join(BUILD, "site.json"), JSON.stringify({ wasmBindgen: tool, sizes }, null, 1));
  return sizes;
}

const APP = ["main.js", "sweep.js", "pocket3d-shell.js", "pocket3d-title.js", "art.js", "pkg/tokyo_wgpu.js", "pkg/tokyo_wgpu_bg.wasm"];
const sha256 = (bytes: Uint8Array | string) => new Bun.CryptoHasher("sha256").update(bytes).digest("hex");

/** Every file under a directory, as paths from it. */
function files(directory: string, under = ""): string[] {
  return readdirSync(join(directory, under), { withFileTypes: true }).flatMap((entry) => (entry.isDirectory() ? files(directory, join(under, entry.name)) : [join(under, entry.name)]));
}

/**
 * The directory a static host serves, for a host that limits a file's size and keeps a file for ten minutes
 * in a browser's cache. Only the page is asked for again at every visit, so everything it names has a name of
 * its own contents: the module and its scripts under `app/<build>/`, the pack's manifest by the pack's hash,
 * a piece by its own. A deployment of new code leaves the pack's files as they are.
 */
async function dist(pieceBytes: number) {
  needPack();
  await build();
  rmSync(DIST, { recursive: true, force: true });
  const app = APP.map((file) => [file, readFileSync(join(SITE, file))] as const);
  const id = sha256(Buffer.concat(app.flatMap(([file, bytes]) => [Buffer.from(file), bytes]))).slice(0, 12);
  for (const [file, bytes] of app) {
    mkdirSync(join(DIST, "app", id, file, ".."), { recursive: true });
    writeFileSync(join(DIST, "app", id, file), bytes);
  }
  // The pack in pieces of one size, and the manifest that lists them (pocket_web_wgpu::source::Manifest).
  const whole = readFileSync(pack);
  const hash = sha256(whole);
  mkdirSync(join(DIST, "pack"));
  const pieces: string[] = [];
  for (let at = 0; at < whole.length; at += pieceBytes) {
    const piece = whole.subarray(at, at + pieceBytes);
    pieces.push(`${sha256(piece).slice(0, 20)}.bin`);
    writeFileSync(join(DIST, "pack", pieces.at(-1)!), piece);
  }
  const manifest = `pack/${hash.slice(0, 16)}.json`;
  writeFileSync(join(DIST, manifest), JSON.stringify({ pack: "pocket-pack-pieces/1", bytes: whole.length, piece: pieceBytes, sha256: hash, pieces }, null, 1));
  // The page names its build and its pack.
  let page = readFileSync(join(SITE, "index.html"), "utf8");
  for (const [from, to] of [[`<meta name="pocket-pack" content="city.pack">`, `<meta name="pocket-pack" content="${manifest}">`], [`src="main.js"`, `src="app/${id}/main.js"`]] as const) {
    if (!page.includes(from)) throw new Error(`wgpu/page/index.html has no ${from}`);
    page = page.replace(from, to);
  }
  writeFileSync(join(DIST, "index.html"), page);
  cpSync(join(SITE, "icon.png"), join(DIST, "icon.png"));

  // What was written is the pack, and is what the host takes.
  const again = new Bun.CryptoHasher("sha256");
  for (const name of pieces) again.update(readFileSync(join(DIST, "pack", name)));
  if (again.digest("hex") !== hash) throw new Error("the pieces do not make up the pack");
  const all = files(DIST).map((file) => ({ file, bytes: statSync(join(DIST, file)).size }));
  const total = all.reduce((sum, f) => sum + f.bytes, 0);
  const largest = all.reduce((a, b) => (b.bytes > a.bytes ? b : a));
  const part = (prefix: string) => all.filter((f) => f.file.startsWith(prefix)).reduce((sum, f) => ({ files: sum.files + 1, bytes: sum.bytes + f.bytes }), { files: 0, bytes: 0 });
  const refused = [
    ...all.filter((f) => f.bytes > HOST.file).map((f) => `${f.file} is ${f.bytes} bytes (a file is at most ${HOST.file})`),
    ...(all.length > HOST.files ? [`${all.length} files (at most ${HOST.files})`] : []),
    ...(total > HOST.bytes ? [`${total} bytes (at most ${HOST.bytes})`] : []),
    ...readdirSync(DIST).filter((name) => HOST.reserved.includes(name)).map((name) => `${name}/ is the host's own`),
  ];
  if (refused.length) throw new Error(`the host would refuse the directory: ${refused.join("; ")}`);
  const report = { directory: DIST, files: all.length, bytes: total, largest, build: id, page: part("index.html"), app: part("app/"), pack: { ...part("pack/"), manifest, pieces: pieces.length, piece: pieceBytes, sha256: hash } };
  writeFileSync(join(BUILD, "dist.json"), JSON.stringify(report, null, 1));
  return report;
}

/** The deployable directory as its host serves it: whole files, the page asked for again at every visit. */
function serveDist(port: number) {
  return Bun.serve({
    port,
    hostname: "127.0.0.1",
    fetch(request) {
      const path = decodeURIComponent(new URL(request.url).pathname);
      const file = join(DIST, path === "/" ? "index.html" : path);
      if (!file.startsWith(DIST) || !existsSync(file) || !statSync(file).isFile()) return new Response("not found", { status: 404 });
      const type = file.split(".").pop()!;
      return new Response(Bun.file(file), { headers: { "Content-Type": TYPES[type] ?? "application/octet-stream", "Cache-Control": type === "html" ? "no-cache" : "public, max-age=600" } });
    },
  });
}

const TYPES: Record<string, string> = { html: "text/html; charset=utf-8", js: "text/javascript; charset=utf-8", wasm: "application/wasm", json: "application/json", png: "image/png" };

/** The site and the pack. The pack is answered a range at a time, as a tab asks for it. */
function serve(port: number) {
  const size = statSync(pack).size;
  return Bun.serve({
    port,
    hostname: "127.0.0.1",
    fetch(request) {
      const path = decodeURIComponent(new URL(request.url).pathname);
      if (path === "/city.pack") {
        const head = { "Accept-Ranges": "bytes", "Content-Type": "application/octet-stream", "Cache-Control": "no-store" };
        const range = request.headers.get("range")?.match(/^bytes=(\d+)-(\d*)$/);
        if (!range) return new Response(Bun.file(pack), { headers: head });
        const from = Number(range[1]);
        const to = Math.min(range[2] ? Number(range[2]) : size - 1, size - 1);
        if (from > to) return new Response(null, { status: 416, headers: { "Content-Range": `bytes */${size}` } });
        return new Response(Bun.file(pack).slice(from, to + 1), { status: 206, headers: { ...head, "Content-Range": `bytes ${from}-${to}/${size}`, "Content-Length": String(to - from + 1) } });
      }
      const file = join(SITE, path === "/" ? "index.html" : path);
      if (!file.startsWith(SITE) || !existsSync(file) || !statSync(file).isFile()) return new Response("not found", { status: 404 });
      return new Response(Bun.file(file), { headers: { "Content-Type": TYPES[file.split(".").pop()!] ?? "application/octet-stream", "Cache-Control": "no-store" } });
    },
  });
}

/** One frame on this machine's GPU, from the pack's file or from a manifest of its pieces. Returns the status
 * the run printed. */
async function shot(out: string, extra: string[], from = pack) {
  await $`cargo build --release --bin tokyo-shot`.cwd(CRATE).quiet();
  mkdirSync(resolve(out, ".."), { recursive: true });
  const text = await $`${join(CRATE, "target/release/tokyo-shot")} --pack ${from} --out ${out} ${extra}`.text();
  return JSON.parse(text);
}

const stamp = () => new Date().toISOString().replace(/[:.]/g, "-");
const validation = (run: string) => {
  const directory = join(ROOT, ".pocket-build/validation/web", run);
  mkdirSync(directory, { recursive: true });
  return directory;
};

if (command === "cook") {
  await $`bun tools/tokyo.ts cook --area ${area} --profile ipod60`.cwd(ROOT);
} else if (command === "build") {
  console.log(JSON.stringify(await build(), null, 1));
} else if (command === "dist") {
  console.log(JSON.stringify(await dist(Math.round(Number(option("--piece", "2")) * (1 << 20))), null, 1));
} else if (command === "serve" && rest.includes("--dist")) {
  if (!existsSync(join(DIST, "index.html"))) await dist(2 << 20);
  const server = serveDist(Number(option("--port", "8787")));
  console.log(`http://127.0.0.1:${server.port}/   (${DIST})`);
} else if (command === "serve") {
  needPack();
  if (!existsSync(join(SITE, "pkg/tokyo_wgpu_bg.wasm"))) await build();
  const server = serve(Number(option("--port", "8787")));
  console.log(`http://127.0.0.1:${server.port}/   (${pack})`);
} else if (command === "shot") {
  needPack();
  const out = resolve(option("--out", join(validation(`shot-${stamp()}`), "frame.png")));
  const passed = ["--shape", "--size", "--samples", "--budget", "--frames", "--words", "--against", "--status"].flatMap((flag) => (option(flag) ? [flag, option(flag)] : []));
  console.log(JSON.stringify(await shot(out, passed), null, 1));
  console.log(out);
} else if (command === "counts") {
  // An eye the iPod touch reported in a status (`bun tools/ipod.ts bench`, 2026-10-05, build 96adc0b4e314),
  // with what its frame drew. The distances of the levels of detail are held where its governor had them.
  needPack();
  const device = { eye: [1315.1, 468.7, -96], look: [-0.957, -0.28, 0.07], hour: 20.276, reach: [391.8, 1044.8], tris: [10362, 12363, 0, 1272], draws: 231, places: [0, 11, 25], turned: 8647 };
  const target = device.eye.map((v, i) => +(v + device.look[i]! * 100).toFixed(1));
  const words = `hour=${device.hour} rate=0 near=${device.reach[0]} mid=${device.reach[1]} view=${[...device.eye, ...target].join(",")}`;
  const here = await shot(join(validation(`counts-${stamp()}`), "frame.png"), ["--shape", "ipod", "--words", words]);
  const rows = (["tris", "draws", "places", "turned"] as const).map((key) => ({ what: key, ipod: JSON.stringify(device[key]), wgpu: JSON.stringify(here[key]) }));
  console.table(rows);
  if (rows.some((row) => row.ipod !== row.wgpu)) throw new Error("the wgpu frame does not draw what the iPod touch's did");
} else if (command === "check") {
  needPack();
  // (--dist: the deployable directory, served whole files only, with the pack in pieces)
  const deployed = rest.includes("--dist") ? await dist(Math.round(Number(option("--piece", "2")) * (1 << 20))) : null;
  const sizes = deployed ? JSON.parse(readFileSync(join(BUILD, "site.json"), "utf8")).sizes : await build();
  const { chromium } = await import("playwright-core");
  const server = deployed ? serveDist(0) : serve(0);
  const directory = validation(`check-${deployed ? "dist-" : ""}${stamp()}`);
  const seconds = Number(option("--seconds", "5"));
  const origin = `http://127.0.0.1:${server.port}`;
  // WebGPU needs the real GPU: headless Chrome is given Metal through ANGLE; --headed opens a window instead.
  const browser = await chromium.launch({ channel: "chrome", headless: !rest.includes("--headed"), args: ["--use-angle=metal", "--enable-gpu", "--ignore-gpu-blocklist", "--enable-unsafe-webgpu", "--no-proxy-server"] });
  const report: Record<string, unknown> = { chrome: browser.version(), pack, sizes, deployed };
  type Run = { address: string; readyMs: number; firstFrameMs: number; bytesBeforeFirstFrame: number; fps: number; frameMs: number; status: Record<string, unknown>; problems: string[] };
  try {
    /** Opens the page, waits for the city, and measures: frames a second in step with the display, and what a
     * frame costs when nothing waits for the display. `picture`: the canvas's own pixels go to this file. */
    const run = async (address: string, picture?: string): Promise<Run> => {
      const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
      const problems: string[] = [];
      page.on("console", (message) => message.type() === "error" && problems.push(message.text()));
      page.on("pageerror", (error) => problems.push(String(error)));
      // What the tab was sent before its first frame: the page, the module, the head of the pack.
      let before = 0;
      let counting = true;
      page.on("response", (response) => {
        if (counting) before += Number(response.headers()["content-length"] ?? 0);
      });
      await page.goto(`${origin}/${address}`);
      await page.waitForFunction("window.pocketTokyo && (window.pocketTokyo.frames > 0 || window.pocketTokyo.failure)", undefined, { timeout: 60_000 });
      counting = false;
      const first = (await page.evaluate("({ failure: pocketTokyo.failure, ready: pocketTokyo.ready, firstFrame: pocketTokyo.firstFrame })")) as { failure: string; ready: number; firstFrame: number };
      if (first.failure) throw new Error(`${address}: ${first.failure}`);
      // The city as the tab shows it once every cell near the eye has arrived.
      await page.waitForFunction("pocketTokyo.tokyo.settled()", undefined, { timeout: 60_000 });
      const from = (await page.evaluate("({ frames: pocketTokyo.frames, at: performance.now() })")) as { frames: number; at: number };
      await page.waitForTimeout(seconds * 1000);
      const to = (await page.evaluate("({ frames: pocketTokyo.frames, at: performance.now() })")) as { frames: number; at: number };
      const status = JSON.parse((await page.evaluate("pocketTokyo.tokyo.status()")) as string);
      if (picture) {
        const url = (await page.evaluate("pocketTokyo.capture()")) as string;
        writeFileSync(picture, Buffer.from(url.slice(url.indexOf(",") + 1), "base64"));
      }
      const frameMs = (await page.evaluate("pocketTokyo.burst(300)")) as number;
      await page.close();
      return { address, readyMs: Math.round(first.ready), firstFrameMs: Math.round(first.firstFrame), bytesBeforeFirstFrame: before, fps: +(((to.frames - from.frames) * 1000) / (to.at - from.at)).toFixed(2), frameMs: +frameMs.toFixed(3), status, problems };
    };
    // The GPU the tab is given.
    const probe = await browser.newPage();
    await probe.goto(`${origin}/index.html`);
    report.gpu = await probe.evaluate(`(async () => {
      const adapter = await navigator.gpu?.requestAdapter({ powerPreference: "high-performance" });
      if (!adapter) return null;
      const { vendor, architecture, device, description } = adapter.info ?? {};
      return { vendor, architecture, device, description, fallback: adapter.info?.isFallbackAdapter ?? adapter.isFallbackAdapter ?? false, format: navigator.gpu.getPreferredCanvasFormat() };
    })()`);
    await probe.close();

    // A held eye and a stopped clock: the tab's frame beside this machine's own of the same view.
    const words = "hour=13 rate=0 view=230,120,-200,0,150,0";
    const picture = join(directory, "tab.png");
    report.picture = await run(`?shape=ipod&words=${encodeURIComponent(words)}`, picture);
    await shot(join(directory, "here.png"), ["--shape", "ipod", "--words", words], deployed ? join(DIST, deployed.pack.manifest) : pack);
    report.tabAgainstHere = JSON.parse(await $`${join(CRATE, "target/release/tokyo-shot")} --compare ${picture} ${join(directory, "here.png")}`.text());
    // The tour, for the frames a second: the iPod touch's screen and the PS Vita's.
    report.ipod = await run("?shape=ipod", join(directory, "ipod-tour.png"));
    report.vita = await run("?shape=vita", join(directory, "vita-tour.png"));
    // The shadows swept in the frames, for what a sweep costs a frame there.
    report.sweepHere = (await run("?shape=ipod&sweep=here")).status.shadows;

    // The page as a person meets it: the title card first, then the keys, a drag and another screen.
    const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
    const problems: string[] = [];
    page.on("console", (message) => message.type() === "error" && problems.push(message.text()));
    page.on("pageerror", (error) => problems.push(String(error)));
    const expect = (what: string, ok: boolean) => {
      if (!ok) throw new Error(`the page: ${what}`);
    };
    const status = async () => JSON.parse((await page.evaluate("pocketTokyo.tokyo.status()")) as string);
    // (the card's cover and whether the city's canvas is shown)
    const shown = async () => (await page.evaluate("[document.querySelectorAll('[aria-label=Pocket3D]').length, document.getElementById('city').hidden]")) as [number, boolean];
    await page.goto(`${origin}/?shape=vita`);
    await page.waitForTimeout(1200);
    const during = await shown();
    await page.screenshot({ path: join(directory, "title.png") });
    expect(`the Pocket3D title card covers the page before the city is shown (${during})`, during[0] === 1 && during[1] === true);
    await page.waitForFunction("window.pocketTokyo?.frames > 60", undefined, { timeout: 60_000 });
    const afterwards = await shown();
    expect(`the card has left and the city is shown (${afterwards})`, afterwards[0] === 0 && afterwards[1] === false);
    const touring = await status();
    await page.keyboard.down("KeyW");
    await page.waitForTimeout(1500);
    await page.keyboard.up("KeyW");
    const flown = await status();
    expect("W takes the eye off the tour and flies it", touring.tour.on === true && flown.tour.on === false);
    const box = (await page.locator("canvas#city").boundingBox())!;
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width / 2 + 200, box.y + box.height / 2, { steps: 10 });
    await page.mouse.up();
    await page.waitForTimeout(500);
    const turned = await status();
    const heading = (look: number[]) => Math.atan2(look[0]!, -look[2]!);
    // (the picture follows the pointer: dragged right, the eye turns left)
    const swung = Math.atan2(Math.sin(heading(turned.look) - heading(flown.look)), Math.cos(heading(turned.look) - heading(flown.look)));
    expect(`a drag to the right turns the eye to the left (it turned ${swung.toFixed(3)} rad)`, swung < -0.2);
    await page.keyboard.press("KeyT");
    await page.waitForTimeout(300);
    expect("T hands the eye back to the tour", (await status()).tour.on === true);
    const shaped = (await page.evaluate(`pocketTokyo.reshape("psp")`)) as { width: number; height: number; hz: number };
    await page.waitForTimeout(1500);
    const after = await status();
    const size = (await page.evaluate("[document.getElementById('city').width, document.getElementById('city').height]")) as number[];
    expect("another screen while the city flies", shaped.width === 480 && shaped.height === 272 && size.join() === "480,272" && after.shape.name === "psp" && after.governor.budget === 42000 && Math.abs(after.fps - 30) < 2);
    await page.screenshot({ path: join(directory, "page-psp.png") });
    expect(`no error on the page (${problems.join("; ")})`, problems.length === 0);
    await page.close();
    report.page = { tourThenKeys: [touring.tour.on, flown.tour.on], dragTurnedRad: +swung.toFixed(3), reshaped: after.shape, fpsAfterReshape: after.fps };

    // A browser without WebGPU is told so in one sentence, after the card.
    const without = await browser.newPage();
    await without.addInitScript("Object.defineProperty(Navigator.prototype, 'gpu', { get: undefined, configurable: true }); delete Navigator.prototype.gpu;");
    await without.goto(`${origin}/`);
    await without.waitForFunction("document.getElementById('say').textContent !== ''", undefined, { timeout: 20_000 });
    report.withoutWebGPU = await without.locator("#say").textContent();
    const cover = (await without.evaluate("document.querySelectorAll('[aria-label=Pocket3D]').length")) as number;
    expect(`a browser without WebGPU is told so, once the card has left ("${report.withoutWebGPU}", ${cover})`, report.withoutWebGPU === "This browser has no WebGPU, which Pocket Tokyo draws with." && cover === 0);
    await without.close();
  } finally {
    await browser.close();
    server.stop(true);
    writeFileSync(join(directory, "report.json"), JSON.stringify(report, null, 1));
  }
  const brief = (r: Run) => ({ readyMs: r.readyMs, firstFrameMs: r.firstFrameMs, bytesBeforeFirstFrame: r.bytesBeforeFirstFrame, fps: r.fps, frameMs: r.frameMs, drawn: r.status.drawn, draws: r.status.draws, read: r.status.read, shadows: r.status.shadows, problems: r.problems });
  console.log(JSON.stringify({ deployed: deployed && { files: deployed.files, bytes: deployed.bytes, largest: deployed.largest, pack: deployed.pack }, chrome: report.chrome, gpu: report.gpu, wasm: sizes["pkg/tokyo_wgpu_bg.wasm"], tabAgainstHere: report.tabAgainstHere, picture: brief(report.picture as Run), ipod: brief(report.ipod as Run), vita: brief(report.vita as Run), sweepHere: report.sweepHere, page: report.page, withoutWebGPU: report.withoutWebGPU }, null, 1));
  console.log(directory);
} else throw new Error("usage: cook | build | serve [--port N] [--dist] | dist [--piece MiB] | shot [--out PNG] [--shape NAME] [--words WORDS] [--against PNG] | counts | check [--headed] [--seconds N] [--dist]");
