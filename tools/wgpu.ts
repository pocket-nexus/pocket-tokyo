#!/usr/bin/env bun
// Pocket Tokyo drawn with wgpu (wgpu/): in a browser tab over WebGPU, and on
// this machine, where a frame goes to a file.
//
//   bun tools/wgpu.ts cook                         the city for it: the iPod touch's pack (profiles/ipod60.json)
//   bun tools/wgpu.ts build                        wasm32 + wasm-bindgen + the page → .pocket-build/wgpu/site
//   bun tools/wgpu.ts serve [--port 8787]          the site and the pack, with byte ranges
//   bun tools/wgpu.ts shot [--out f.png] [--shape ipod] [--size WxH] [--words "view=… hour=…"] [--against device.png]
//                                                  one frame on this machine's GPU (Metal) → a PNG and the status
//   bun tools/wgpu.ts counts                       the triangles and draws of an eye the iPod touch reported, here
//   bun tools/wgpu.ts check [--headed] [--seconds 5]   the tab in Chrome: a frame, frames a second at two sizes,
//                                                  bytes read before the first frame → .pocket-build/validation/web/
//
// Every command takes [--area shiba] and [--pack PATH] (default: .pocket-build/city/<area>/ipod60/city.pack).
// The packs, the site and the captures stay under the ignored .pocket-build/.

import { $ } from "bun";
import { cpSync, existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { gzipSync } from "node:zlib";
import { join, resolve } from "node:path";
import { POCKET3D_ICON } from "../vendor/pocketjs/tools/pocket3d-icon.ts";

const ROOT = resolve(import.meta.dir, "..");
const CRATE = join(ROOT, "wgpu");
const BUILD = join(ROOT, ".pocket-build/wgpu");
const SITE = join(BUILD, "site");
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
  for (const file of ["index.html", "main.js"]) cpSync(join(CRATE, "page", file), join(SITE, file));
  cpSync(join(CRATE, "kernel/web/pocket3d-shell.js"), join(SITE, "pocket3d-shell.js"));
  // The Pocket3D title card and the icon of the tab, as PocketJS ships them.
  for (const file of ["pocket3d-title.js", "art.js"]) cpSync(join(TITLE, file), join(SITE, file));
  cpSync(POCKET3D_ICON.ios2x, join(SITE, "icon.png"));
  const sizes: Record<string, { bytes: number; gzip: number }> = {};
  for (const file of ["pkg/tokyo_wgpu_bg.wasm", "pkg/tokyo_wgpu.js", "main.js", "pocket3d-shell.js", "pocket3d-title.js", "art.js", "index.html"]) {
    const bytes = readFileSync(join(SITE, file));
    sizes[file] = { bytes: bytes.length, gzip: gzipSync(bytes, { level: 9 }).length };
  }
  writeFileSync(join(BUILD, "site.json"), JSON.stringify({ wasmBindgen: tool, sizes }, null, 1));
  return sizes;
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

/** One frame on this machine's GPU. Returns the status the run printed. */
async function shot(out: string, extra: string[]) {
  await $`cargo build --release --bin tokyo-shot`.cwd(CRATE).quiet();
  mkdirSync(resolve(out, ".."), { recursive: true });
  const text = await $`${join(CRATE, "target/release/tokyo-shot")} --pack ${pack} --out ${out} ${extra}`.text();
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
  const sizes = await build();
  const { chromium } = await import("playwright-core");
  const server = serve(0);
  const directory = validation(`check-${stamp()}`);
  const seconds = Number(option("--seconds", "5"));
  const origin = `http://127.0.0.1:${server.port}`;
  // WebGPU needs the real GPU: headless Chrome is given Metal through ANGLE; --headed opens a window instead.
  const browser = await chromium.launch({ channel: "chrome", headless: !rest.includes("--headed"), args: ["--use-angle=metal", "--enable-gpu", "--ignore-gpu-blocklist", "--enable-unsafe-webgpu", "--no-proxy-server"] });
  const report: Record<string, unknown> = { chrome: browser.version(), pack, sizes };
  try {
    const measure = async (name: string, address: string) => {
      const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
      const problems: string[] = [];
      page.on("console", (message) => message.type() === "error" && problems.push(message.text()));
      page.on("pageerror", (error) => problems.push(String(error)));
      let before = 0;
      let counting = true;
      page.on("response", async (response) => {
        // What the tab was sent before its first frame: the page, the module, the head of the pack.
        const length = Number(response.headers()["content-length"] ?? 0) || (await response.body().catch(() => Buffer.alloc(0))).length;
        if (counting) before += length;
      });
      await page.goto(`${origin}/${address}`);
      await page.waitForFunction("window.pocketTokyo && (window.pocketTokyo.frames > 0 || window.pocketTokyo.failure)", undefined, { timeout: 60_000 });
      counting = false;
      const first = (await page.evaluate("({ failure: pocketTokyo.failure, firstFrame: pocketTokyo.firstFrame })")) as { failure: string; firstFrame: number };
      if (first.failure) throw new Error(`${name}: ${first.failure}`);
      // The city as the tab shows it once every cell near the eye has arrived.
      await page.waitForFunction("pocketTokyo.tokyo.settled()", undefined, { timeout: 60_000 });
      const from = (await page.evaluate("({ frames: pocketTokyo.frames, at: performance.now() })")) as { frames: number; at: number };
      await page.waitForTimeout(seconds * 1000);
      const to = (await page.evaluate("({ frames: pocketTokyo.frames, at: performance.now(), status: JSON.parse(pocketTokyo.tokyo.status()), adapter: navigator.gpu.getPreferredCanvasFormat() })")) as { frames: number; at: number; status: Record<string, unknown>; adapter: string };
      await page.locator("canvas").screenshot({ path: join(directory, `${name}.png`) });
      await page.close();
      return { address, firstFrameMs: Math.round(first.firstFrame), bytesBeforeFirstFrame: before, fps: +(((to.frames - from.frames) * 1000) / (to.at - from.at)).toFixed(2), canvasFormat: to.adapter, status: to.status, problems };
    };
    // A held eye and a stopped clock for the picture; the tour for the frames a second.
    const held = "words=" + encodeURIComponent("hour=13 rate=0 view=230,120,-200,0,150,0");
    report.picture = await measure("ipod-view", `?shape=ipod&${held}`);
    report.ipod = await measure("ipod-tour", "?shape=ipod");
    report.vita = await measure("vita-tour", "?shape=vita");
  } finally {
    await browser.close();
    server.stop(true);
    writeFileSync(join(directory, "report.json"), JSON.stringify(report, null, 1));
  }
  console.log(JSON.stringify(report, null, 1));
  console.log(directory);
} else throw new Error("usage: cook | build | serve [--port N] | shot [--out PNG] [--shape NAME] [--words WORDS] [--against PNG] | counts | check [--headed] [--seconds N]");
