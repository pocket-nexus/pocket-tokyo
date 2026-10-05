#!/usr/bin/env bun
// Pocket Tokyo on Nintendo 3DS: build the Rust core and the C host into a
// .3dsx (devkitARM in PocketJS's pinned container), install and start it over
// PocketJS's paired LAN wire, steer and measure it.
//
//   bun tools/n3ds.ts build                     # dist/3ds/pocket-tokyo.3dsx, the pack in its ROMFS
//   bun tools/n3ds.ts install [--no-build]      # build, send, start, wait for the program to report
//   bun tools/n3ds.ts status
//   bun tools/n3ds.ts ctl "tour=1 hour=18"      # tokyo_sim::flight::Flight::control words
//   bun tools/n3ds.ts capture [--out f.png] [--surface top|auxiliary]
//   bun tools/n3ds.ts bench [--seconds 60] [--install]   # the tour's frame timings → .pocket-build/validation/3ds/
//   bun tools/n3ds.ts emu [--frames 90] [--ctl "view=..."] [--out f.png]
//                                               # the same .3dsx in Azahar (software PICA): a frame and its status
//
// `--host ADDRESS` (default 192.168.8.159, or POCKET_3DS_HOST). The console
// must run a Pocket Runtime build with the paired wire: this program itself
// once installed, or another Pocket Nexus .3dsx. Installs are .3dsx only.

import { $ } from "bun";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { deflateSync } from "node:zlib";
import { pocketRuntimeDeviceId } from "../vendor/pocketjs/contracts/spec/pocket-runtime-wire.ts";
import { withDeviceLease } from "../vendor/pocketjs/tools/device-lease.ts";
import { THREE_DS_DEV_HOST_ABI, THREE_DS_DEV_TARGET_ID } from "../vendor/pocketjs/tools/3ds-profile.ts";
import { runContainer } from "../vendor/pocketjs/tools/3ds-toolchain.ts";

const ROOT = resolve(import.meta.dir, "..");
const POCKETJS = join(ROOT, "vendor/pocketjs");
const DIR = join(ROOT, ".pocket-build/3ds");
const ARTIFACT = join(ROOT, "dist/3ds/pocket-tokyo.3dsx");
const NAME = "pocket-tokyo.3dsx";
const RECEIPTS = join(ROOT, ".pocket-build/validation/3ds");
/** Pocket3D's app icon for the 3DS (48 and 24 pixels), from the repository's root: `ICONS` in n3ds/Makefile. */
const ICONS = "vendor/pocketjs/engine/pocket3d/icon/3ds";
const argv = process.argv.slice(2);
const cmd = argv[0] ?? "";
const opt = (key: string, fallback: string) => {
  const at = argv.indexOf(key);
  return at < 0 ? fallback : (argv[at + 1] ?? fallback);
};
const PACK = join(ROOT, `.pocket-build/city/${opt("--area", "shiba")}/n3ds30/city.pack`);
const host = opt("--host", process.env.POCKET_3DS_HOST ?? "192.168.8.159");

// Loaded by path at run time: PocketJS's client module is checked by PocketJS's own compiler settings.
const clientModule: string = join(POCKETJS, "tools/3ds-runtime-client.ts");
const { discoverPocketRuntimes, parsePocketRuntimeToken, PocketRuntimeClient } = await import(clientModule);
type Client = any;

const sha = (path: string) => {
  const h = new Bun.CryptoHasher("sha256");
  h.update(readFileSync(path));
  return h.digest("hex");
};

function encodePng(rgba: Uint8Array, w: number, h: number): Uint8Array {
  const table = new Uint32Array(256).map((_, n) => {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    return c >>> 0;
  });
  const crc = (b: Uint8Array) => {
    let c = 0xffffffff;
    for (const x of b) c = table[(c ^ x) & 255]! ^ (c >>> 8);
    return (c ^ 0xffffffff) >>> 0;
  };
  const chunk = (type: string, data: Uint8Array) => {
    const out = new Uint8Array(12 + data.length);
    const view = new DataView(out.buffer);
    view.setUint32(0, data.length);
    out.set(new TextEncoder().encode(type), 4);
    out.set(data, 8);
    view.setUint32(8 + data.length, crc(out.subarray(4, 8 + data.length)));
    return out;
  };
  const head = new Uint8Array(13);
  const hv = new DataView(head.buffer);
  hv.setUint32(0, w);
  hv.setUint32(4, h);
  head.set([8, 6, 0, 0, 0], 8);
  const raw = new Uint8Array((w * 4 + 1) * h);
  for (let y = 0; y < h; y++) raw.set(rgba.subarray(y * w * 4, (y + 1) * w * 4), y * (w * 4 + 1) + 1);
  return Buffer.concat([new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10]), chunk("IHDR", head), chunk("IDAT", deflateSync(raw)), chunk("IEND", new Uint8Array(0))]);
}

async function build() {
  if (!existsSync(PACK)) throw new Error(`no pack at ${PACK}: run \`bun tools/tokyo.ts cook --profile n3ds30\` first`);
  await $`rustup run nightly-2026-07-02 cargo build --release`.cwd(join(ROOT, "n3ds/core"));
  const romfs = join(DIR, "romfs");
  mkdirSync(romfs, { recursive: true });
  mkdirSync(join(DIR, "build"), { recursive: true });
  const packSha = JSON.parse(readFileSync(resolve(PACK, "../receipt.json"), "utf8")).pack.sha256 as string;
  const mark = join(DIR, "romfs.json");
  if (!existsSync(join(romfs, "city.pack")) || !existsSync(mark) || JSON.parse(readFileSync(mark, "utf8")).sha256 !== packSha) {
    cpSync(PACK, join(romfs, "city.pack"));
    writeFileSync(mark, JSON.stringify({ sha256: packSha }));
  }
  // The build's identity: the sources, the core library and the pack.
  const id = new Bun.CryptoHasher("sha256");
  for (const f of readdirSync(join(ROOT, "n3ds/src")).sort()) id.update(readFileSync(join(ROOT, "n3ds/src", f)));
  id.update(readFileSync(join(ROOT, "n3ds/core/target/armv6k-nintendo-3ds/release/libtokyo_n3ds_core.a")));
  id.update(packSha);
  const buildId = id.digest("hex").slice(0, 12);
  const header = `#define POCKETJS_HOST_ABI ${THREE_DS_DEV_HOST_ABI}\n#define POCKETJS_TARGET_ID "${THREE_DS_DEV_TARGET_ID}"\n#define TOKYO_BUILD_ID "${buildId}"\n`;
  const config = join(DIR, "build/config.h");
  if (!existsSync(config) || readFileSync(config, "utf8") !== header) writeFileSync(config, header);
  // Compile a snapshot on the container's own filesystem: the shared mount can show a stale size for a file
  // that was just rewritten, on either side. The snapshot's name is new each build.
  const snapshot = `source-${buildId}-${Date.now()}.tar`;
  for (const f of readdirSync(DIR).filter((f) => f.startsWith("source-"))) rmSync(join(DIR, f));
  // The app icon is Pocket3D's: the snapshot takes its two sizes under the path n3ds/Makefile reads them from.
  await $`tar --no-xattrs -cf ${join(DIR, snapshot)} n3ds/src n3ds/Makefile ${ICONS}/icon.png ${ICONS}/icon-small.png .pocket-build/3ds/build/config.h`.cwd(ROOT);
  await runContainer(
    `mkdir -p /tmp/source /tmp/build && tar -xf /tokyo/.pocket-build/3ds/${snapshot} -C /tmp/source
cp /tmp/source/.pocket-build/3ds/build/config.h /tmp/build/config.h
make -f /tmp/source/n3ds/Makefile -j8 BUILD=/tmp/build SOURCE=/tmp/source/n3ds/src
cp /tmp/build/tokyo.elf /tmp/build/tokyo.map /tokyo/.pocket-build/3ds/build/`,
    [{ hostPath: ROOT, containerPath: "/tokyo" }],
    "/tokyo",
    {},
    "Pocket Tokyo build",
  );
  const bytes = readFileSync(ARTIFACT).length;
  if (bytes > 32 * 1024 * 1024) throw new Error(`the .3dsx is ${bytes} bytes; the wire installs at most 32 MiB`);
  mkdirSync(RECEIPTS, { recursive: true });
  const receipt = { target: "3ds", buildId, bytes, sha256: sha(ARTIFACT), packSha256: packSha };
  writeFileSync(join(RECEIPTS, "build.json"), JSON.stringify(receipt, null, 1) + "\n");
  console.log(`3ds: ${ARTIFACT} ${(bytes / 1e6).toFixed(1)} MB, build ${buildId}`);
  return receipt;
}

async function connect(): Promise<Client> {
  const keys = opt("--keys", join(POCKETJS, ".pocket/3ds/devices"));
  let devices: any[] = await discoverPocketRuntimes({ addresses: [host] });
  for (let attempt = 0; attempt < 3 && !devices.some((d) => d.address === host); attempt++) {
    await Bun.sleep(400);
    devices = await discoverPocketRuntimes({ addresses: [host] });
  }
  const device = devices.find((d) => d.address === host);
  if (!device) throw new Error(`no Pocket Runtime answers at ${host}:8131`);
  let token: Uint8Array | undefined;
  for (const name of existsSync(keys) ? readdirSync(keys).filter((n) => n.endsWith(".key")) : []) {
    const t = parsePocketRuntimeToken(readFileSync(join(keys, name), "utf8"));
    if (pocketRuntimeDeviceId(t) === device.deviceId) token = t;
  }
  if (!token) throw new Error(`no pairing key in ${keys} matches the console; pass --keys DIR`);
  const client = new PocketRuntimeClient({ host, port: device.port, token, timeoutMs: 20000, heartbeatTimeoutMs: 30000 });
  client.on("ctrl", (m: any) => {
    if (m.t === "log" || m.t === "runtime.native") console.log(JSON.stringify(m));
  });
  try {
    await client.connect();
    return client;
  } catch (e) {
    client.close();
    throw e;
  }
}

async function status(c: Client, text = ""): Promise<any> {
  const reply = c.waitForCtrl((m: any) => m.t === "tokyo.status", 8000);
  await c.sendCtrl({ t: "tokyo.control", ...(text ? { text } : {}) });
  return await reply;
}

/** Connects, retrying: right after another client leaves, the first connect can time out. */
async function session<T>(f: (c: Client) => Promise<T>): Promise<T> {
  let last: unknown;
  for (let attempt = 0; attempt < 4; attempt++) {
    let c: Client | undefined;
    try {
      c = await connect();
      return await f(c);
    } catch (e) {
      last = e;
      await Bun.sleep(1200);
    } finally {
      c?.close();
    }
  }
  throw last;
}

/** Runs `f` holding the console's lease. A child process started with `lease.environment` shares it. */
async function device<T>(f: (lease: { environment: NodeJS.ProcessEnv }) => Promise<T>): Promise<T> {
  return await withDeviceLease("3ds:wire", f);
}

/** Sends the .3dsx and waits until the console reports this build, inside one lease. */
async function install(lease: { environment: NodeJS.ProcessEnv }, receipt: { buildId: string }) {
  await $`bun ${join(POCKETJS, "tools/3ds-dev.ts")} install --host ${host} --file ${ARTIFACT} --name ${NAME}`.cwd(POCKETJS).env(lease.environment as Record<string, string>);
  const end = Date.now() + 180_000;
  let last: any;
  while (Date.now() < end) {
    await Bun.sleep(1500);
    try {
      last = await session((c) => status(c));
      if (last.build === receipt.buildId && (last.stage === "running" || last.phase === "load-error")) break;
    } catch (e) {
      last = { error: String(e) };
    }
  }
  if (last?.build !== receipt.buildId) throw new Error(`the console did not come up with this build: ${JSON.stringify(last)}`);
  return last;
}

async function shot(c: Client, surface: string) {
  const pending = c.waitForScreenshot();
  await c.sendCtrl({ t: "screenshot", surface });
  return await pending;
}

switch (cmd) {
  case "build":
    await build();
    break;
  case "install": {
    const receipt = argv.includes("--no-build") ? JSON.parse(readFileSync(join(RECEIPTS, "build.json"), "utf8")) : await build();
    // One lease for the transfer and the wait: another checkout's tool cannot put its own program in between.
    await device(async (lease) => console.log(JSON.stringify(await install(lease, receipt), null, 1)));
    break;
  }
  case "status":
    await device(async () => console.log(JSON.stringify(await session((c) => status(c)), null, 1)));
    break;
  case "ctl":
    await device(async () => console.log(JSON.stringify(await session((c) => status(c, argv[1] ?? "")), null, 1)));
    break;
  case "capture":
    await device(async () => {
      const out = resolve(opt("--out", join(RECEIPTS, `capture-${Date.now()}.png`)));
      mkdirSync(resolve(out, ".."), { recursive: true });
      const text = opt("--ctl", "");
      const picture = await session(async (c) => {
        if (text) {
          await status(c, text);
          await Bun.sleep(Number(opt("--wait", "6")) * 1000);
        }
        return await shot(c, opt("--surface", "top"));
      });
      await Bun.write(out, picture.png);
      console.log(out);
    });
    break;
  case "bench":
    await device(async (lease) => {
      const seconds = Number(opt("--seconds", "60"));
      const extra = opt("--ctl", "");
      const build = JSON.parse(readFileSync(join(RECEIPTS, "build.json"), "utf8"));
      // With --install the transfer and the measurement share the lease.
      if (argv.includes("--install")) await install(lease, build);
      await session(async (c) => {
        const first = await status(c, `tour=1 restart=1 view=off ${extra}`);
        if (first.build !== build.buildId) throw new Error(`the console runs build ${first.build}, not ${build.buildId}`);
        await Bun.sleep(4000);
        const start = Date.now();
        const begin = await status(c);
        const samples: any[] = [];
        while (Date.now() - start < seconds * 1000) {
          // Each sample costs the console a late frame or so: answering takes it a few milliseconds.
          await Bun.sleep(5000);
          const s = await status(c);
          samples.push({ t: (Date.now() - start) / 1000, frameMs: s.frameMs, worstMs: s.worstMs, late: s.late, frames: s.frames, cpuMs: s.cpuMs, gpuMs: s.gpuMs, draws: s.draws, drawn: s.drawn, tris: s.tris, places: s.places, reach: s.reach, hour: s.clock.hour, cells: s.cells, eye: s.eye });
        }
        const last = samples.at(-1)!;
        const mean = (f: (s: any) => number) => samples.reduce((n, s) => n + f(s), 0) / samples.length;
        const frames = last.frames - begin.frames;
        const late = last.late - begin.late;
        const summary = {
          seconds,
          frames,
          lateFrames: late,
          lateShare: late / Math.max(frames, 1),
          averageFrameMs: mean((s) => s.frameMs),
          fps: frames / last.t,
          worstFrameMs: Math.max(...samples.map((s) => s.worstMs)),
          triangles: { least: Math.min(...samples.map((s) => s.drawn)), mean: Math.round(mean((s) => s.drawn)), most: Math.max(...samples.map((s) => s.drawn)) },
          draws: { mean: Math.round(mean((s) => s.draws)), most: Math.max(...samples.map((s) => s.draws)) },
          cpuMs: mean((s) => s.cpuMs),
          gpuMs: { mean: mean((s) => s.gpuMs), most: Math.max(...samples.map((s) => s.gpuMs)) },
          hours: [begin.clock.hour, last.hour],
          control: extra,
        };
        const dir = join(RECEIPTS, `bench-${new Date().toISOString().replace(/[:.]/g, "-")}`);
        mkdirSync(dir, { recursive: true });
        writeFileSync(join(dir, "device.json"), JSON.stringify({ summary, identity: { device: `3ds:${host}`, ...build, new3ds: begin.new3ds }, samples }, null, 1));
        console.log(join(dir, "device.json"));
        console.log(JSON.stringify(summary, null, 1));
      });
    });
    break;
  case "emu": {
    // Azahar with a home of its own: the emulated memory card holds the run's control words, and takes the
    // frame and the status the program writes.
    const app = process.env.AZAHAR ?? "/Applications/Azahar.app";
    const source = `${homedir()}/Library/Application Support/Azahar`;
    if (!existsSync(app) || !existsSync(`${source}/config/qt-config.ini`)) throw new Error(`no Azahar at ${app} with a configuration of its own`);
    mkdirSync(join(DIR, "emu"), { recursive: true });
    const fixture = mkdtempSync(join(DIR, "emu/azahar-"));
    const user = `${fixture}/Library/Application Support/Azahar`;
    mkdirSync(`${user}/config`, { recursive: true });
    for (const dir of ["nand", "sysdata"]) if (existsSync(`${source}/${dir}`)) cpSync(`${source}/${dir}`, `${user}/${dir}`, { recursive: true });
    let config = readFileSync(`${source}/config/qt-config.ini`, "utf8");
    for (const [key, value] of Object.entries({ graphics_api: "0", resolution_factor: "1", frame_limit: "1000", use_vsync: "false", check_for_update_on_start: "false", instant_debug_log: "true" })) {
      config = config.replace(new RegExp(`^${key}=.*$`, "m"), `${key}=${value}`).replace(new RegExp(`^${key}\\\\default=.*$`, "m"), `${key}\\default=false`);
    }
    writeFileSync(`${user}/config/qt-config.ini`, config);
    const card = `${user}/sdmc/pocket-tokyo`;
    mkdirSync(card, { recursive: true });
    const frames = Number(opt("--frames", "90"));
    writeFileSync(`${card}/boot.txt`, `title=0 ${opt("--ctl", "")} shot=${frames} exit=${frames + 3}\n`);
    const rom = `${fixture}/${NAME}`;
    cpSync(ARTIFACT, rom);
    const launch = Bun.spawnSync(["open", "-n", "-g", "-a", app, "--env", `HOME=${fixture}`, "--stdout", `${fixture}/console.log`, "--stderr", `${fixture}/console.log`, "--args", rom]);
    if (launch.exitCode) throw new Error(launch.stderr.toString());
    const owned = () =>
      Bun.spawnSync(["ps", "-axo", "pid=,command="])
        .stdout.toString()
        .split("\n")
        .filter((line) => line.includes(`${app}/Contents/MacOS/azahar`) && line.includes(rom))
        .map((line) => Number(line.trim().split(/\s+/)[0]));
    try {
      const start = Date.now();
      const limit = Number(opt("--timeout", "240")) * 1000;
      let asked = false;
      while (!existsSync(`${card}/done`)) {
        // After an emulator that was killed, macOS asks whether to reopen its windows, and the run waits for the
        // answer.
        if (!asked && Date.now() - start > 4000 && !existsSync(`${card}/boot.log`)) {
          asked = true;
          Bun.spawnSync(["osascript", "-e", 'tell application "System Events" to tell (first process whose name contains "zahar") to click button 2 of window 1']);
        }
        if (Date.now() - start > limit || (Date.now() - start > 15000 && !owned().length)) {
          if (existsSync(`${card}/boot.log`)) console.log(readFileSync(`${card}/boot.log`, "utf8"));
          throw new Error(`the emulator run did not finish: ${fixture}`);
        }
        await Bun.sleep(500);
      }
      if (existsSync(`${card}/status.json`)) console.log(readFileSync(`${card}/status.json`, "utf8"));
      const bytes = readFileSync(`${card}/shot.bgr`);
      if (bytes.length !== 400 * 240 * 3) throw new Error("the frame is not 400 by 240");
      // Columns from the left, each from the bottom up: blue, green, red.
      const rgba = new Uint8Array(400 * 240 * 4);
      for (let y = 0; y < 240; y++)
        for (let x = 0; x < 400; x++) {
          const s = (x * 240 + 239 - y) * 3;
          rgba.set([bytes[s + 2]!, bytes[s + 1]!, bytes[s]!, 255], (y * 400 + x) * 4);
        }
      const out = resolve(opt("--out", join(DIR, "emu/frame.png")));
      writeFileSync(out, encodePng(rgba, 400, 240));
      console.log(out);
    } finally {
      // Asked to quit first: an emulator that is killed leaves macOS a question for the next run.
      for (const signal of ["SIGTERM", "SIGKILL"] as const) {
        for (const pid of owned()) {
          try {
            process.kill(pid, signal);
          } catch {}
        }
        for (let i = 0; i < 30 && owned().length; i++) await Bun.sleep(100);
      }
      if (!argv.includes("--keep")) rmSync(fixture, { recursive: true, force: true });
    }
    break;
  }
  default:
    console.log("usage: bun tools/n3ds.ts <build|install|status|ctl|capture|bench|emu> [--host ADDRESS]");
    process.exit(cmd ? 1 : 0);
}
