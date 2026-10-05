#!/usr/bin/env bun
// Pocket Tokyo drawn with wgpu (wgpu/): in a browser tab over WebGPU, and on
// this machine, where a frame goes to a file.
//
//   bun tools/wgpu.ts cook                         the city for it: the iPod touch's pack (profiles/ipod60.json)
//   bun tools/wgpu.ts build                        wasm32 + wasm-bindgen + the page + the interface for the four
//                                                  devices (tools/ui.ts) on PocketJS's UI core → .pocket-build/wgpu/site
//   bun tools/wgpu.ts serve [--port 8787]          the site and the pack, with byte ranges
//   bun tools/wgpu.ts dist [--piece 2]             the directory a static host serves → .pocket-build/wgpu/dist:
//                                                  the page, the module under its build's name, and the pack
//                                                  cut into pieces of that many MiB with their manifest
//   bun tools/wgpu.ts serve --dist                 that directory as such a host serves it: no byte ranges
//   bun tools/wgpu.ts shot [--out f.png] [--shape ipod] [--size WxH] [--words "view=… hour=…"] [--against device.png]
//                                                  one frame on this machine's GPU (Metal) → a PNG and the status
//   bun tools/wgpu.ts counts                       the triangles and draws of an eye the iPod touch reported, here
//   bun tools/wgpu.ts check [--headed] [--seconds 5] [--dist]   the page in Chrome, driven by keys, pointer and
//                                                  touch: each device from its title into a flight and its menu,
//                                                  another device picked in a flight, what a frame and a redraw of
//                                                  the interface cost, what the first frame needs on a slow line
//                                                  → .pocket-build/validation/web/
//
// Every command takes [--area shiba] and [--pack PATH] (default: .pocket-build/city/<area>/ipod60/city.pack).
// `build` compiles the interface as tools/ui.ts does: it needs `bun install` in vendor/pocketjs and the exported
// area (.pocket-build/city/<area>/ir).
// The packs, the site and the captures stay under the ignored .pocket-build/.

import { $ } from "bun";
import { cpSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { gzipSync } from "node:zlib";
import { join, resolve } from "node:path";
import { POCKET3D_ICON } from "../vendor/pocketjs/tools/pocket3d-icon.ts";
import { cutPack, stagePocket3dWeb } from "../vendor/pocketjs/tools/pocket3d-web.ts";
import { compileInterface, DEVICES } from "./ui.ts";

const ROOT = resolve(import.meta.dir, "..");
const CRATE = join(ROOT, "wgpu");
const BUILD = join(ROOT, ".pocket-build/wgpu");
const SITE = join(BUILD, "site");
const DIST = join(BUILD, "dist");
// What the host a build is deployed to allows (Pocket Studio's site deployments): the size of a file, the
// files and the bytes of a deployment, and the top-level names it keeps for itself.
const HOST = { file: 32 << 20, files: 4000, bytes: 1 << 30, reserved: ["play", "runtime"] };

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
  rmSync(SITE, { recursive: true, force: true });
  mkdirSync(join(SITE, "pkg"), { recursive: true });
  await $`wasm-bindgen --target web --no-typescript --out-dir ${join(SITE, "pkg")} ${join(CRATE, "target/wasm32-unknown-unknown/release/tokyo_wgpu.wasm")}`;
  for (const file of ["index.html", "main.js", "sweep.js"]) cpSync(join(CRATE, "page", file), join(SITE, file));
  // What the page loads from PocketJS's browser kernel (vendor/pocketjs/devices/web/pocket-web-wgpu), as
  // PocketJS stages it: the page's modules, the Pocket3D title card, the realm of the interface's guest
  // with the UI core, and the host helpers of the framework. Then the icon of the tab.
  await stagePocket3dWeb(SITE);
  cpSync(POCKET3D_ICON.ios2x, join(SITE, "icon.png"));
  // The game's interface for each device, as its own build compiles it, with the plan PocketJS resolved.
  for (const device of DEVICES) {
    const built = await compileInterface(device, area);
    mkdirSync(join(SITE, "ui", device), { recursive: true });
    for (const file of ["tokyo.js", "tokyo.pak", "plan.json"]) cpSync(join(built.directory, file), join(SITE, "ui", device, file));
  }

  const sizes: Record<string, { bytes: number; gzip: number }> = {};
  for (const file of files(SITE).sort()) {
    const bytes = readFileSync(join(SITE, file));
    sizes[file] = { bytes: bytes.length, gzip: gzipSync(bytes, { level: 9 }).length };
  }
  writeFileSync(join(BUILD, "site.json"), JSON.stringify({ wasmBindgen: tool, sizes }, null, 1));
  return sizes;
}

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
  // (everything of the site but the page and its icon: the module, the scripts, the interface)
  const app = files(SITE).filter((file) => !["index.html", "icon.png"].includes(file)).sort().map((file) => [file, readFileSync(join(SITE, file))] as const);
  const id = sha256(Buffer.concat(app.flatMap(([file, bytes]) => [Buffer.from(file), bytes]))).slice(0, 12);
  for (const [file, bytes] of app) {
    mkdirSync(join(DIST, "app", id, file, ".."), { recursive: true });
    writeFileSync(join(DIST, "app", id, file), bytes);
  }
  // The pack in pieces of one size, each named by its hash, and the manifest that lists them
  // (pocket_web_wgpu::source::Manifest), named by the pack's.
  const cut = cutPack(pack, join(DIST, "pack"), pieceBytes);
  const manifest = `pack/${cut.manifest}`;
  const [pieces, hash] = [cut.pieces, cut.sha256];
  // The page names its build and its pack.
  let page = readFileSync(join(SITE, "index.html"), "utf8");
  for (const [from, to] of [[`<meta name="pocket-pack" content="city.pack">`, `<meta name="pocket-pack" content="${manifest}">`], [`src="main.js"`, `src="app/${id}/main.js"`]] as const) {
    if (!page.includes(from)) throw new Error(`wgpu/page/index.html has no ${from}`);
    page = page.replace(from, to);
  }
  writeFileSync(join(DIST, "index.html"), page);
  cpSync(join(SITE, "icon.png"), join(DIST, "icon.png"));

  // What was written is what the host takes.
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

const TYPES: Record<string, string> = { html: "text/html; charset=utf-8", js: "text/javascript; charset=utf-8", wasm: "application/wasm", json: "application/json", png: "image/png", pak: "application/octet-stream" };

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
  const report: Record<string, any> = { chrome: browser.version(), pack, deployed };
  const expect = (what: string, ok: boolean) => {
    if (!ok) throw new Error(`the page: ${what}`);
  };
  type Page = Awaited<ReturnType<typeof browser.newPage>>;
  /** A page on the site, and what a person does to it: keys held for a moment, a pointer down and up at a
   * place of a screen given in that screen's logical pixels, a drag. */
  const visit = async (address: string, context?: Awaited<ReturnType<typeof browser.newContext>>) => {
    const page: Page = await (context ?? browser).newPage({ viewport: { width: 1280, height: 800 } });
    const problems: string[] = [];
    page.on("console", (message) => message.type() === "error" && problems.push(message.text()));
    page.on("pageerror", (error) => problems.push(String(error)));
    await page.goto(`${origin}/${address}`);
    const status = async () => JSON.parse((await page.evaluate("pocketTokyo.tokyo.status()")) as string);
    const at = async (screen: "upper" | "lower", size: number[], x: number, y: number) => {
      const box = (await page.locator(`[data-pocket-screen=${screen}]`).boundingBox())!;
      return [box.x + (x / size[0]!) * box.width, box.y + (y / size[1]!) * box.height] as const;
    };
    return {
      page,
      problems,
      status,
      /** The city flies behind the title, the interface is up, and the card has left. */
      async up() {
        await page.waitForFunction("window.pocketTokyo && ((pocketTokyo.firstCity && pocketTokyo.interface?.()) || pocketTokyo.failure)", undefined, { timeout: 90_000 });
        const failure = (await page.evaluate("pocketTokyo.failure")) as string;
        if (failure) throw new Error(`${address}: ${failure}`);
      },
      async mode(want: string) {
        await page.waitForFunction(`JSON.parse(pocketTokyo.tokyo.status()).mode === ${JSON.stringify(want)}`, undefined, { timeout: 5_000 }).catch(() => {});
        const now = (await status()).mode;
        expect(`${address}: the flow is "${now}", not "${want}"`, now === want);
      },
      async key(code: string, hold = 90) {
        await page.keyboard.down(code);
        await page.waitForTimeout(hold);
        await page.keyboard.up(code);
        await page.waitForTimeout(200);
      },
      async tap(screen: "upper" | "lower", size: number[], x: number, y: number) {
        const [px, py] = await at(screen, size, x, y);
        await page.mouse.move(px, py);
        await page.mouse.down();
        await page.waitForTimeout(100);
        await page.mouse.up();
        await page.waitForTimeout(250);
      },
      async drag(screen: "upper" | "lower", size: number[], from: number[], to: number[], hold = 0) {
        const [ax, ay] = await at(screen, size, from[0]!, from[1]!);
        const [bx, by] = await at(screen, size, to[0]!, to[1]!);
        await page.mouse.move(ax, ay);
        await page.mouse.down();
        await page.mouse.move(bx, by, { steps: 12 });
        await page.waitForTimeout(hold);
        await page.mouse.up();
        await page.waitForTimeout(250);
      },
      /** The screens as their canvases hold them, a pixel to a pixel: `<name>.png`, and `<name>-lower.png`. */
      async save(name: string) {
        const shot = (await page.evaluate("pocketTokyo.capture()")) as { upper: string; lower: string | null };
        const write = (file: string, url: string) => writeFileSync(join(directory, file), Buffer.from(url.slice(url.indexOf(",") + 1), "base64"));
        write(`${name}.png`, shot.upper);
        if (shot.lower) write(`${name}-lower.png`, shot.lower);
        return join(directory, `${name}.png`);
      },
    };
  };
  const heading = (look: number[]) => Math.atan2(look[0]!, -look[2]!);
  const swing = (a: number[], b: number[]) => Math.atan2(Math.sin(heading(b) - heading(a)), Math.cos(heading(b) - heading(a)));
  const compare = async (a: string, b: string) => JSON.parse(await $`${join(CRATE, "target/release/tokyo-shot")} --compare ${a} ${b}`.text());

  try {
    // The GPU the tab is given.
    const probe = await browser.newPage();
    await probe.goto(`${origin}/index.html?interface=off`);
    report.gpu = await probe.evaluate(`(async () => {
      const adapter = await navigator.gpu?.requestAdapter({ powerPreference: "high-performance" });
      if (!adapter) return null;
      const { vendor, architecture, device, description } = adapter.info ?? {};
      return { vendor, architecture, device, description, fallback: adapter.info?.isFallbackAdapter ?? adapter.isFallbackAdapter ?? false, format: navigator.gpu.getPreferredCanvasFormat() };
    })()`);
    await probe.close();

    // ---- the scene alone: the tab's frame of a held eye beside this machine's own, and the title card first
    const words = "hour=13 rate=0 view=230,120,-200,0,150,0";
    {
      const p = await visit(`?device=ipod&interface=off&words=${encodeURIComponent(words)}`);
      await p.page.waitForTimeout(1200);
      const during = (await p.page.evaluate("[document.querySelectorAll('[aria-label=Pocket3D]').length, document.getElementById('city').hidden]")) as [number, boolean];
      await p.page.screenshot({ path: join(directory, "title-card.png") });
      expect(`the Pocket3D title card covers the page before the city is shown (${during})`, during[0] === 1 && during[1] === true);
      await p.page.waitForFunction("window.pocketTokyo?.firstCity > 0 && pocketTokyo.tokyo.settled()", undefined, { timeout: 90_000 });
      const afterwards = (await p.page.evaluate("[document.querySelectorAll('[aria-label=Pocket3D]').length, document.getElementById('city').hidden]")) as [number, boolean];
      expect(`the card has left and the city is shown (${afterwards})`, afterwards[0] === 0 && afterwards[1] === false);
      const tab = await p.save("scene-tab");
      await shot(join(directory, "scene-here.png"), ["--shape", "ipod", "--words", words], deployed ? join(DIST, deployed.pack.manifest) : pack);
      report.sceneTabAgainstHere = await compare(tab, join(directory, "scene-here.png"));
      expect(`the tab's frame is this machine's (${JSON.stringify(report.sceneTabAgainstHere)})`, report.sceneTabAgainstHere.mean < 0.5);
      expect(`no error on the page (${p.problems.join("; ")})`, p.problems.length === 0);
      await p.page.close();
    }

    // ---- each device: from the title into a flight through its interface, a list opened and closed, and
    // what a frame costs with the interface over the scene
    const vita = [480, 272], psp = [480, 272], top3ds = [400, 240], low3ds = [320, 240], ipod = [480, 320];
    const measure = async (p: Awaited<ReturnType<typeof visit>>) => {
      await p.page.waitForFunction("pocketTokyo.tokyo.settled()", undefined, { timeout: 90_000 });
      const read = `({ frames: pocketTokyo.frames, at: performance.now(), timing: { ...pocketTokyo.timing } })`;
      const from = (await p.page.evaluate(read)) as { frames: number; at: number; timing: Record<string, number> };
      await p.page.waitForTimeout(seconds * 1000);
      const to = (await p.page.evaluate(read)) as typeof from;
      const span = (to.at - from.at) / 1000;
      const d = (key: string) => to.timing[key]! - from.timing[key]!;
      const status = await p.status();
      const frameMs = (await p.page.evaluate("pocketTokyo.burst(300)")) as number;
      const round = (n: number) => +n.toFixed(3);
      return {
        fps: round((to.frames - from.frames) / span),
        // (300 frames made without waiting for the display: the flight, the guest's turns and redraws, the scene)
        frameMs: round(frameMs),
        turnsPerSecond: round(d("turns") / span),
        turnMs: round(d("turnMs") / Math.max(1, d("turns"))),
        redrawsPerSecond: round(d("redraws") / span),
        // (a redraw: the UI core draws the interface once with its alpha, `drawMs`; the rest is the upload)
        redrawMs: round(d("redrawMs") / Math.max(1, d("redraws"))),
        drawMs: round(d("drawMs") / Math.max(1, d("redraws"))),
        // (the second screen is drawn when its own draw hash changes)
        lowersPerSecond: round(d("lowers") / span),
        lowerMs: round(d("lowerMs") / Math.max(1, d("lowers"))),
        drawn: status.drawn,
        draws: status.draws,
        late: status.late,
        shape: status.shape,
      };
    };
    report.devices = {};
    {
      // PS Vita: the d-pad and the face buttons from the keys; the left stick flies, the right one looks.
      const p = await visit("?device=vita");
      await p.up();
      await p.mode("title");
      await p.save("vita-title");
      await p.key("ArrowDown");
      await p.key("KeyZ");
      await p.mode("flight");
      const before = await p.status();
      expect("vita: Fly yourself hands the eye over", before.tour.on === false);
      await p.page.keyboard.down("KeyW");
      await p.page.keyboard.down("KeyL");
      await p.page.waitForTimeout(1200);
      await p.page.keyboard.up("KeyW");
      await p.page.keyboard.up("KeyL");
      const flown = await p.status();
      const moved = Math.hypot(flown.eye[0] - before.eye[0], flown.eye[2] - before.eye[2]);
      expect(`vita: the left stick flies (${moved.toFixed(0)} m) and the right one turns the view (${swing(before.look, flown.look).toFixed(2)} rad)`, moved > 20 && swing(before.look, flown.look) > 0.3);
      await p.save("vita-flight");
      await p.key("Space");
      await p.mode("menu");
      await p.save("vita-menu");
      await p.key("KeyX");
      await p.mode("flight");
      // The panel takes taps: the menu again, and its first row (Resume) under a finger.
      await p.key("Space");
      await p.mode("menu");
      await p.tap("upper", vita, 330, 71);
      await p.mode("flight");
      await p.key("ShiftLeft");
      expect("vita: SELECT hands the eye to the tour", (await p.status()).tour.on === true);
      report.devices.vita = await measure(p);
      expect(`vita: no error on the page (${p.problems.join("; ")})`, p.problems.length === 0);
      await p.page.close();
    }
    {
      // PSP: one stick flies ahead and turns; two face buttons look up and down.
      const p = await visit("?device=psp");
      await p.up();
      await p.mode("title");
      await p.key("Enter");
      await p.mode("flight");
      expect("psp: Take the tour starts on the tour", (await p.status()).tour.on === true);
      await p.key("ShiftLeft");
      const before = await p.status();
      expect("psp: SELECT takes the eye off the tour", before.tour.on === false);
      await p.page.keyboard.down("KeyW");
      await p.page.keyboard.down("KeyD");
      await p.page.keyboard.down("KeyK");
      await p.page.waitForTimeout(1200);
      for (const code of ["KeyW", "KeyD", "KeyK"]) await p.page.keyboard.up(code);
      const flown = await p.status();
      const moved = Math.hypot(flown.eye[0] - before.eye[0], flown.eye[2] - before.eye[2]);
      expect(`psp: the stick flies (${moved.toFixed(0)} m) and turns (${swing(before.look, flown.look).toFixed(2)} rad), the bottom face button looks down (${(flown.look[1] - before.look[1]).toFixed(2)})`, moved > 20 && swing(before.look, flown.look) > 0.3 && flown.look[1] < before.look[1] - 0.1);
      await p.save("psp-flight");
      await p.key("Space");
      await p.mode("menu");
      await p.key("ArrowDown");
      await p.save("psp-menu");
      await p.key("Space");
      await p.mode("flight");
      await p.key("ShiftLeft");
      report.devices.psp = await measure(p);
      // A setting is kept between visits: the menu, Settings, the statistics switch; then the page again.
      await p.key("Space");
      await p.mode("menu");
      for (let i = 0; i < 3; i++) await p.key("ArrowDown");
      await p.key("Enter");
      await p.key("ArrowDown");
      await p.key("Enter");
      await p.page.waitForTimeout(300);
      await p.save("psp-settings");
      const kept = (await p.page.evaluate(`localStorage.getItem("pocket-tokyo.interface")`)) as string | null;
      expect(`psp: the statistics switch is on and kept (${kept})`, (await p.status()).statistics === true && JSON.parse(kept ?? "{}").options?.stats === 1);
      await p.page.reload();
      await p.up();
      await p.key("Enter");
      await p.mode("flight");
      await p.page.waitForTimeout(500);
      expect("psp: the setting is there again on the next visit", (await p.status()).statistics === true);
      await p.save("psp-statistics");
      report.kept = kept;
      expect(`psp: no error on the page (${p.problems.join("; ")})`, p.problems.length === 0);
      await p.page.close();
    }
    {
      // Nintendo 3DS: the lists and the map are on the lower screen, under the pointer as under a stylus.
      const p = await visit("?device=3ds");
      await p.up();
      await p.mode("title");
      await p.save("3ds-title");
      await p.tap("lower", low3ds, 100, 75);
      await p.mode("flight");
      const before = await p.status();
      expect("3ds: Fly yourself, under the stylus, hands the eye over", before.tour.on === false);
      await p.page.keyboard.down("KeyW");
      await p.page.keyboard.down("KeyI");
      await p.page.waitForTimeout(1000);
      await p.page.keyboard.up("KeyW");
      await p.page.keyboard.up("KeyI");
      const flown = await p.status();
      expect("3ds: the Circle Pad flies and X looks up", Math.hypot(flown.eye[0] - before.eye[0], flown.eye[2] - before.eye[2]) > 20 && flown.look[1] > before.look[1] + 0.1);
      await p.save("3ds-flight");
      // The Menu key of the lower screen, then B to leave the list; the Tour key hands the eye over.
      await p.tap("lower", low3ds, 288, 26);
      await p.mode("menu");
      await p.save("3ds-menu");
      await p.key("KeyK");
      await p.mode("flight");
      await p.tap("lower", low3ds, 288, 72);
      expect("3ds: the Tour key of the lower screen hands the eye to the tour", (await p.status()).tour.on === true);
      // The day as a bar: a drag along it turns the clock.
      const hour = (await p.status()).clock.hour;
      await p.drag("lower", low3ds, [200, 217], [60, 217], 600);
      const turned = (await p.status()).clock.hour;
      expect(`3ds: a drag along the day's bar turns the clock (${hour.toFixed(1)} to ${turned.toFixed(1)})`, Math.abs(turned - hour) > 2);
      await p.save("3ds-clock");
      report.devices["3ds"] = await measure(p);
      expect(`3ds: no error on the page (${p.problems.join("; ")})`, p.problems.length === 0);
      await p.page.close();
    }
    {
      // iPod touch: no button at all. The pointer is a finger: the lists, the stick, the keys, the view.
      const held = "hour=21 rate=0 view=230,120,-200,0,150,0";
      const p = await visit(`?device=ipod&words=${encodeURIComponent(held)}`);
      await p.up();
      await p.mode("title");
      await p.save("ipod-title");
      await p.tap("upper", ipod, 100, 164);
      await p.mode("flight");
      await p.page.waitForFunction("pocketTokyo.tokyo.settled()", undefined, { timeout: 90_000 });
      await p.page.waitForTimeout(700);
      // The eye of a capture of the device itself, at the same hour and with the same things on the screen.
      report.ipodFlightCapture = await p.save("ipod-flight");
      // The eye in hand: a finger on the city turns the view, the stick flies, the menu's key opens the list.
      await p.page.evaluate(`pocketTokyo.tokyo.control("view=off rate=0.03")`);
      const before = await p.status();
      await p.drag("upper", ipod, [240, 120], [340, 120]);
      await p.page.waitForTimeout(400);
      const looked = await p.status();
      expect(`ipod: a finger dragged to the right turns the view to the left (${swing(before.look, looked.look).toFixed(2)} rad)`, swing(before.look, looked.look) < -0.2);
      await p.drag("upper", ipod, [86, 236], [86, 206], 1200);
      const flown = await p.status();
      expect("ipod: the stick pushed up flies ahead", Math.hypot(flown.eye[0] - looked.eye[0], flown.eye[2] - looked.eye[2]) > 20);
      await p.tap("upper", ipod, 32, 32);
      await p.mode("menu");
      await p.save("ipod-menu");
      await p.tap("upper", ipod, 270, 30);
      await p.mode("flight");
      await p.tap("upper", ipod, 332, 34);
      expect("ipod: the TOUR key hands the eye to the tour", (await p.status()).tour.on === true);
      report.devices.ipod = await measure(p);
      expect(`ipod: no error on the page (${p.problems.join("; ")})`, p.problems.length === 0);
      await p.page.close();
    }

    // ---- another device in the middle of a flight, picked from the page's own text
    {
      const p = await visit("?device=vita");
      await p.up();
      await p.key("Enter");
      await p.mode("flight");
      await p.page.waitForTimeout(1500);
      const before = await p.status();
      await p.page.getByRole("button", { name: "Nintendo 3DS" }).click();
      await p.page.waitForFunction("pocketTokyo.device() === '3ds' && pocketTokyo.interface()", undefined, { timeout: 20_000 });
      await p.page.waitForTimeout(1500);
      const after = await p.status();
      const sizes = (await p.page.evaluate("[...document.querySelectorAll('[data-pocket-screen]')].map((c) => [c.width, c.height, c.hidden])")) as [number, number, boolean][];
      expect(`a device picked in a flight keeps the flight (${after.mode}, tour ${after.tour.on}, at ${after.tour.at} from ${before.tour.at}) on its own screens (${JSON.stringify(sizes)})`,
        after.mode === "flight" && after.tour.on === true && after.tour.at > before.tour.at && after.shape.name === "3ds" && after.governor.budget === 60000 && JSON.stringify(sizes) === "[[400,240,false],[320,240,false]]");
      await p.save("switched-3ds");
      await p.page.screenshot({ path: join(directory, "page-3ds.png") });
      await p.page.getByRole("button", { name: "iPod touch" }).click();
      await p.page.waitForFunction("pocketTokyo.device() === 'ipod' && pocketTokyo.interface()", undefined, { timeout: 20_000 });
      await p.page.waitForTimeout(1200);
      const last = await p.status();
      expect("the next device too", last.mode === "flight" && last.shape.name === "ipod" && Math.abs(last.fps - 60) < 6);
      await p.save("switched-ipod");
      await p.page.screenshot({ path: join(directory, "page-ipod.png") });
      report.switched = { from: before.shape.name, to: [after.shape.name, last.shape.name], tourAt: [before.tour.at, after.tour.at, last.tour.at] };
      expect(`no error on the page (${p.problems.join("; ")})`, p.problems.length === 0);
      await p.page.close();
    }

    // ---- a browser whose pointer is a finger: the iPod touch first, and a device's buttons on the page
    {
      const context = await browser.newContext({ hasTouch: true, isMobile: true, viewport: { width: 844, height: 390 }, deviceScaleFactor: 2 });
      const p = await visit("", context);
      await p.up();
      expect("a finger gets the iPod touch first", (await p.page.evaluate("pocketTokyo.device()")) === "ipod");
      await p.page.screenshot({ path: join(directory, "finger-ipod.png") });
      await p.page.close();
      const q = await visit("?device=psp", context);
      await q.up();
      const named = (await q.page.evaluate("[...document.querySelectorAll('[data-pocket-button]')].map((b) => b.textContent).join(' ')")) as string;
      expect(`a device with buttons has them on the page (${named})`, ["L", "R", "△", "○", "✕", "□", "START", "SELECT", "▲"].every((name) => named.split(" ").includes(name)));
      const touch = async (label: string, hold = 120) => {
        const box = (await q.page.locator(`[data-pocket-button="${label}"]`).boundingBox())!;
        const cdp = await context.newCDPSession(q.page);
        const point = { x: box.x + box.width / 2, y: box.y + box.height / 2, id: 1 };
        await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [point] });
        await q.page.waitForTimeout(hold);
        await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
        await q.page.waitForTimeout(250);
        await cdp.detach();
      };
      await touch("○");
      await q.mode("flight");
      await touch("START");
      await q.mode("menu");
      await q.page.screenshot({ path: join(directory, "finger-psp.png") });
      await touch("✕");
      await q.mode("flight");
      // The stick on the page, under a thumb: the eye leaves the tour and flies.
      const before = await q.status();
      const stick = (await q.page.locator("[data-pocket-stick]").boundingBox())!;
      const cdp = await context.newCDPSession(q.page);
      const centre = { x: stick.x + stick.width / 2, y: stick.y + stick.height / 2 };
      await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [{ ...centre, id: 1 }] });
      await cdp.send("Input.dispatchTouchEvent", { type: "touchMove", touchPoints: [{ x: centre.x, y: centre.y - stick.height * 0.4, id: 1 }] });
      await q.page.waitForTimeout(1300);
      await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
      await q.page.waitForTimeout(200);
      const flown = await q.status();
      expect("the stick on the page flies", flown.tour.on === false && Math.hypot(flown.eye[0] - before.eye[0], flown.eye[2] - before.eye[2]) > 15);
      expect(`no error on the page (${q.problems.join("; ")})`, q.problems.length === 0);
      await context.close();
      report.finger = { first: "ipod", buttons: named };
    }

    // ---- what the first frame of the city needs: the page on a line of 16 Mbit/s
    {
      const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
      const cdp = await page.context().newCDPSession(page);
      await cdp.send("Network.enable");
      await cdp.send("Network.emulateNetworkConditions", { offline: false, latency: 20, downloadThroughput: 2_000_000, uploadThroughput: 1_000_000 });
      let received = 0;
      let counting = true;
      cdp.on("Network.dataReceived", (event: { encodedDataLength: number; dataLength: number }) => {
        if (counting) received += event.encodedDataLength || event.dataLength;
      });
      await page.goto(`${origin}/?device=vita`);
      await page.waitForFunction("window.pocketTokyo && (pocketTokyo.firstCity || pocketTokyo.failure)", undefined, { timeout: 180_000 });
      counting = false;
      const first = (await page.evaluate("({ ready: pocketTokyo.ready, interfaceReady: pocketTokyo.interfaceReady, firstFrame: pocketTokyo.firstFrame, firstCity: pocketTokyo.firstCity, failure: pocketTokyo.failure, read: JSON.parse(pocketTokyo.tokyo.status()).read })")) as Record<string, any>;
      await page.screenshot({ path: join(directory, "first-city-frame.png") });
      await page.waitForFunction("pocketTokyo.tokyo.settled()", undefined, { timeout: 240_000 });
      const settledAt = (await page.evaluate("performance.now()")) as number;
      report.firstFrame = { bytesPerSecond: 2_000_000, bytesReceived: received, interfaceMs: Math.round(first.interfaceReady), firstFrameMs: Math.round(first.firstFrame), firstCityFrameMs: Math.round(first.firstCity), picturesThen: first.read.pictures, everythingMs: Math.round(settledAt), failure: first.failure };
      await page.close();
    }

    // ---- a browser without WebGPU is told so in one sentence, after the card
    {
      const without = await browser.newPage();
      await without.addInitScript("Object.defineProperty(Navigator.prototype, 'gpu', { get: undefined, configurable: true }); delete Navigator.prototype.gpu;");
      await without.goto(`${origin}/`);
      await without.waitForFunction("document.getElementById('say').textContent !== ''", undefined, { timeout: 20_000 });
      report.withoutWebGPU = await without.locator("#say").textContent();
      const cover = (await without.evaluate("document.querySelectorAll('[aria-label=Pocket3D]').length")) as number;
      expect(`a browser without WebGPU is told so, once the card has left ("${report.withoutWebGPU}", ${cover})`, report.withoutWebGPU === "This browser has no WebGPU, which Pocket Tokyo draws with." && cover === 0);
      await without.close();
    }
  } finally {
    await browser.close();
    server.stop(true);
    report.sizes = sizes;
    writeFileSync(join(directory, "report.json"), JSON.stringify(report, null, 1));
  }
  const { sizes: _, ...brief } = report;
  console.log(JSON.stringify({ ...brief, module: sizes["pkg/tokyo_wgpu_bg.wasm"], uiCore: sizes["pocketjs.wasm"] }, null, 1));
  console.log(directory);
} else throw new Error("usage: cook | build | serve [--port N] [--dist] | dist [--piece MiB] | shot [--out PNG] [--shape NAME] [--words WORDS] [--against PNG] | counts | check [--headed] [--seconds N] [--dist]");
