#!/usr/bin/env bun
// Pocket Tokyo on PSP: compile the interface (ui/) for the PSP, build the
// PRX with PocketJS's pinned rust-psp toolchain, stage both with the pack on a
// PSPLINK share, start it, steer and measure it.
//
//   bun tools/psp.ts build                    # ui/ + psp/ → dist/psp/{pocket-tokyo.prx,EBOOT.PBP,tokyo.js,tokyo.pak}
//   bun tools/psp.ts serve                    # start usbhostfs_pc (detached, logged) when none is running
//   bun tools/psp.ts run [--no-build]         # build, stage, reset PSPLINK, wait for it to reconnect, start the PRX
//   bun tools/psp.ts status
//   bun tools/psp.ts ctl "tour=1 hour=18"     # host0:/tokyo/control.txt (see tokyo_sim::flight::Flight::control;
//                                             # `mode=title|flight|menu` sets the flow, `ui=tour|fly|menu|resume|title`
//                                             # asks what the interface's lists ask, `press=<mask>` and `rest=<turns>`
//                                             # press PocketJS buttons on it)
//   bun tools/psp.ts capture [--out f.png]    # PSPLINK screenshot
//   bun tools/psp.ts bench [--seconds 60]     # the tour's frame timings, the interface up → .pocket-build/validation/psp/
//   bun tools/psp.ts package                  # dist/psp/PSP/GAME/PocketTokyo for a Memory Stick
//   bun tools/psp.ts emu [--frames 240] [--ctl "view=..."] [--out f.png] [--standalone] [--small] [--keep] [--no-interface]
//                                             # the same PRX in PPSSPPHeadless (software GE): a frame and its status;
//                                             # --small runs it with the 24 MB of a PSP-1000, where the city leaves
//                                             # no room for the interface; --keep leaves interface.json from the last run
//
// One usbhostfs_pc owns the PSP's cable. If one is running (in any checkout),
// these commands use its directory; `--share DIR` names another. Device
// commands take PocketJS's `psp:usb` lease; `--take` ends another holder first.

import { $ } from "bun";
import { closeSync, cpSync, existsSync, mkdirSync, openSync, readFileSync, readSync, rmSync, statSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { deflateSync } from "node:zlib";
import { extractHostBuildInputs, hostBuildEnvironment } from "../vendor/pocketjs/framework/src/manifest/index.ts";
import { withDeviceLease } from "../vendor/pocketjs/tools/device-lease.ts";
import { POCKET3D_ICON } from "../vendor/pocketjs/tools/pocket3d-icon.ts";
import { compileInterface, type Interface } from "./ui.ts";

const ROOT = resolve(import.meta.dir, "..");
const OUT = resolve(ROOT, "dist/psp");
const argv = process.argv.slice(2);
const cmd = argv[0] ?? "";
const opt = (key: string, fallback: string) => {
  const at = argv.indexOf(key);
  return at < 0 ? fallback : (argv[at + 1] ?? fallback);
};
const PACK = resolve(ROOT, `.pocket-build/city/${opt("--area", "shiba")}/psp30/city.pack`);
const port = opt("--port", "10000");

/** The directory the running usbhostfs_pc serves, if there is one. */
async function runningShare(): Promise<string | undefined> {
  const pid = (await $`pgrep -x usbhostfs_pc`.nothrow().quiet().text()).trim().split("\n")[0];
  if (!pid) return undefined;
  const args = (await $`ps -o args= -p ${pid}`.nothrow().quiet().text()).trim().split(/\s+/);
  return args.at(-1);
}
const share = resolve(opt("--share", process.env.TOKYO_PSP_SHARE ?? (await runningShare()) ?? `${ROOT}/.pocket-build/psp/host0`));
const app = `${share}/tokyo`;

const HOST_LOG = `${ROOT}/.pocket-build/psp/usbhostfs.log`;

/** Where the usbhostfs_pc log ends now; undefined without the log (a host started elsewhere). */
function logEnd(): number | undefined {
  return existsSync(HOST_LOG) ? statSync(HOST_LOG).size : undefined;
}

/** Whether the host has logged a new connection to the PSP since `from`. */
function reconnected(from: number): boolean {
  const size = statSync(HOST_LOG).size;
  if (size <= from) return false;
  const fd = openSync(HOST_LOG, "r");
  const buf = Buffer.alloc(Math.min(size - from, 1 << 20));
  readSync(fd, buf, 0, buf.length, size - buf.length);
  closeSync(fd);
  return buf.includes("Connected to device");
}

function encodePng(rgba: Uint8Array, w: number, h: number): Uint8Array {
  const crcTable = new Uint32Array(256).map((_, n) => {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    return c >>> 0;
  });
  const crc = (b: Uint8Array) => {
    let c = 0xffffffff;
    for (const x of b) c = crcTable[(c ^ x) & 255]! ^ (c >>> 8);
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

/** The interface's bundle and its pak: the program reads them beside the pack. */
const UI_FILES = ["tokyo.js", "tokyo.pak"];

/** The interface compiled for the PSP; the last compiled one when `ui/` does not compile just now. */
async function interfaceBundle(): Promise<Interface> {
  try {
    return await compileInterface("psp", opt("--area", "shiba"));
  } catch (e) {
    const directory = `${ROOT}/.pocket-build/ui/psp`;
    if (!UI_FILES.every((f) => existsSync(`${directory}/${f}`)) || !existsSync(`${directory}/plan.json`)) throw e;
    console.warn(`psp: ui/ did not compile (${String(e).split("\n")[0]}); using the bundle already in ${directory}`);
    const plan = JSON.parse(readFileSync(`${directory}/plan.json`, "utf8"));
    return { directory, plan, inputs: extractHostBuildInputs(plan) };
  }
}

/** A PARAM.SFO: keys in order, 32-bit integers and NUL-terminated strings padded to four bytes. */
function paramSfo(values: Record<string, number | string>): Buffer {
  const keys = Object.keys(values).sort();
  const data = keys.map((key) => {
    const value = values[key]!;
    if (typeof value === "number") {
      const bytes = Buffer.alloc(4);
      bytes.writeUInt32LE(value);
      return { format: 0x0404, used: 4, bytes };
    }
    const text = Buffer.from(value + "\0");
    return { format: 0x0204, used: text.length, bytes: Buffer.concat([text, Buffer.alloc((4 - (text.length % 4)) % 4)]) };
  });
  const names = Buffer.from(keys.map((key) => key + "\0").join(""));
  const keyTable = 20 + keys.length * 16;
  const dataTable = keyTable + Math.ceil(names.length / 4) * 4;
  const out = Buffer.alloc(dataTable + data.reduce((sum, d) => sum + d.bytes.length, 0));
  out.write("\0PSF");
  out.writeUInt32LE(0x101, 4);
  out.writeUInt32LE(keyTable, 8);
  out.writeUInt32LE(dataTable, 12);
  out.writeUInt32LE(keys.length, 16);
  let nameAt = 0;
  let dataAt = 0;
  keys.forEach((key, i) => {
    const at = 20 + i * 16;
    const d = data[i]!;
    out.writeUInt16LE(nameAt, at);
    out.writeUInt16LE(d.format, at + 2);
    out.writeUInt32LE(d.used, at + 4);
    out.writeUInt32LE(d.bytes.length, at + 8);
    out.writeUInt32LE(dataAt, at + 12);
    d.bytes.copy(out, dataTable + dataAt);
    nameAt += key.length + 1;
    dataAt += d.bytes.length;
  });
  names.copy(out, keyTable);
  return out;
}

/**
 * Packs the PRX as an EBOOT. `large` asks for the 52 MB of a PSP-2000 or later (cargo-psp has no
 * setting for it): the city and the interface together need more than a PSP-1000's 24 MB, where
 * the program runs without the interface. ICON0.PNG is the Pocket3D icon from PocketJS; PIC1.PNG is
 * a capture of this game.
 */
async function pbp(out: string, prx: string, large: boolean) {
  const sfo = `${out}.SFO`;
  writeFileSync(sfo, paramSfo({ BOOTABLE: 1, CATEGORY: "MG", DISC_VERSION: "1.00", ...(large ? { MEMSIZE: 1 } : {}), PARENTAL_LEVEL: 1, PSP_SYSTEM_VER: "1.00", REGION: 0x8000, TITLE: "Pocket Tokyo" }));
  await $`pack-pbp ${out} ${sfo} ${POCKET3D_ICON.psp} NULL NULL ${ROOT}/psp/assets/pic1.png NULL ${prx} NULL`.quiet();
  rmSync(sfo, { force: true });
}

async function build() {
  const ui = await interfaceBundle();
  // Loaded by path at run time: PocketJS's toolchain module resolves its manifest through its own tsconfig.
  const toolchain: string = `${ROOT}/vendor/pocketjs/tools/psp-toolchain.ts`;
  const tc = (await import(toolchain)).resolvePspBuildToolchain();
  // The interface's runtime (PocketJS's PSP host library) builds QuickJS from C for the same target, with
  // PocketJS's own flags for it, and checks the target it was compiled for against the interface's plan.
  await $`${tc.rustup} run ${tc.manifest.rust.toolchain} cargo psp --release`.cwd(`${ROOT}/psp`).env({
    ...tc.environment,
    RUSTFLAGS: "-A linker-messages -A unexpected-cfgs -A unstable-name-collisions",
    CRATE_CC_NO_DEFAULTS: "1",
    TARGET_CC: "clang",
    TARGET_AR: `${tc.llvmBin}/llvm-ar`,
    TARGET_CFLAGS:
      `-target mipsel-sony-psp -mcpu=mips2 -msingle-float -mlittle-endian -mno-abicalls -fno-pic -G0 -mno-check-zero-division ` +
      `-fno-stack-protector -O2 -I${tc.sdk.path}/psp/include -I${tc.sdk.path}/psp/sdk/include`,
    AR_mipsel_sony_psp: `${tc.llvmBin}/llvm-ar`,
    RANLIB_mipsel_sony_psp: `${tc.llvmBin}/llvm-ranlib`,
    ...hostBuildEnvironment(ui.inputs, { outputDirectory: ui.directory, embedApp: false }),
    POCKETJS_OFFLOAD_SLOT: "",
    RUST_PSP_ABORT_ONLY: "1",
    RUST_PSP_TARGET: `${ROOT}/vendor/pocketjs/hosts/psp/targets/mipsel-sony-psp.json`,
  });
  const from = `${ROOT}/psp/target/mipsel-sony-psp/release`;
  mkdirSync(OUT, { recursive: true });
  cpSync(`${from}/pocket-tokyo-psp.prx`, `${OUT}/pocket-tokyo.prx`);
  await pbp(`${OUT}/EBOOT.PBP`, `${OUT}/pocket-tokyo.prx`, true);
  for (const f of UI_FILES) cpSync(`${ui.directory}/${f}`, `${OUT}/${f}`);
  console.log(`psp: ${OUT}/pocket-tokyo.prx ${(readFileSync(`${OUT}/pocket-tokyo.prx`).length / 1024).toFixed(0)} KiB, interface ${UI_FILES.map((f) => `${f} ${(readFileSync(`${OUT}/${f}`).length / 1024).toFixed(0)} KiB`).join(", ")}`);
}

/** Puts the interface's files in `dir` when they differ from the built ones. */
function stageInterface(dir: string) {
  for (const f of UI_FILES) {
    if (!existsSync(`${OUT}/${f}`)) throw new Error(`no ${OUT}/${f}: run \`bun tools/psp.ts build\` first`);
    if (!existsSync(`${dir}/${f}`) || sha(`${dir}/${f}`) !== sha(`${OUT}/${f}`)) cpSync(`${OUT}/${f}`, `${dir}/${f}`);
  }
}

function sha(path: string): string {
  const h = new Bun.CryptoHasher("sha256");
  h.update(readFileSync(path));
  return h.digest("hex");
}

/** The pack's identity as the cook wrote it, so an unchanged pack is not copied again. */
function packId(): string {
  const receipt = resolve(PACK, "../receipt.json");
  return existsSync(receipt) ? JSON.parse(readFileSync(receipt, "utf8")).pack.sha256 : sha(PACK);
}

function stage(dir: string) {
  if (!existsSync(PACK)) throw new Error(`no pack at ${PACK}: run \`bun tools/tokyo.ts cook --profile psp30\` first`);
  mkdirSync(dir, { recursive: true });
  const id = packId();
  const mark = `${dir}/city.json`;
  if (!existsSync(`${dir}/city.pack`) || !existsSync(mark) || JSON.parse(readFileSync(mark, "utf8")).sha256 !== id) {
    cpSync(PACK, `${dir}/city.pack`);
    writeFileSync(mark, JSON.stringify({ sha256: id, bytes: statSync(PACK).size }));
  }
  return id;
}

async function pspsh(text: string): Promise<string> {
  const p = Bun.spawn(["pspsh", "-p", port, "-e", text], { stdout: "pipe", stderr: "pipe" });
  const timer = setTimeout(() => p.kill(), 15000);
  const [out, err] = await Promise.all([new Response(p.stdout).text(), new Response(p.stderr).text(), p.exited]);
  clearTimeout(timer);
  if (/Error|Could not|failed|connect:/i.test(out + err)) throw new Error(`pspsh ${text}: ${(out + err).trim()}`);
  return out + err;
}

function readStatus(): any {
  // The device rewrites the file in place; a read can land in the middle.
  for (let i = 0; ; i++) {
    try {
      return JSON.parse(readFileSync(`${app}/status.json`, "utf8"));
    } catch (e) {
      if (i >= 40) throw new Error(`no status at ${app}/status.json: is the program running?`);
      Bun.sleepSync(25);
    }
  }
}

async function waitFor(test: (s: any) => boolean, seconds: number): Promise<any> {
  const end = Date.now() + seconds * 1000;
  let last: any;
  while (Date.now() < end) {
    try {
      last = readStatus();
      if (last.stage === "failed") throw new Error(`the device reports: ${last.error} (${last.code})`);
      if (test(last)) return last;
    } catch (e) {
      if (String(e).includes("the device reports")) throw e;
    }
    await Bun.sleep(300);
  }
  throw new Error(`the PSP did not get there in ${seconds} s (last status: ${JSON.stringify(last)})`);
}

function ctl(text: string) {
  mkdirSync(app, { recursive: true });
  // The nonce makes two equal commands differ.
  writeFileSync(`${app}/control.txt`, `${text} nonce=${Date.now()}\n`);
}

async function capture(out: string) {
  const name = `capture-${Date.now()}.bmp`;
  await pspsh(`scrshot host0:/${name}`);
  const path = `${share}/${name}`;
  for (let i = 0; i < 40 && !existsSync(path); i++) await Bun.sleep(100);
  const bmp = readFileSync(path);
  rmSync(path, { force: true });
  // BITMAPINFOHEADER, 24 or 32 bits, rows bottom-up unless the height is negative.
  const view = new DataView(bmp.buffer, bmp.byteOffset, bmp.byteLength);
  const at = view.getUint32(10, true);
  const w = view.getInt32(18, true);
  const hRaw = view.getInt32(22, true);
  const h = Math.abs(hRaw);
  const bpp = view.getUint16(28, true) / 8;
  const stride = (w * bpp + 3) & ~3;
  const rgba = new Uint8Array(w * h * 4);
  for (let y = 0; y < h; y++) {
    const row = at + (hRaw > 0 ? h - 1 - y : y) * stride;
    for (let x = 0; x < w; x++) {
      const s = row + x * bpp;
      rgba.set([bmp[s + 2]!, bmp[s + 1]!, bmp[s]!, 255], (y * w + x) * 4);
    }
  }
  mkdirSync(resolve(out, ".."), { recursive: true });
  writeFileSync(out, encodePng(rgba, w, h));
  console.log(out);
}

async function bench(seconds: number, extra: string) {
  // The tour as a person sees it: the flight, with the interface's instruments over it.
  ctl(`mode=flight tour=1 restart=1 view=off ${extra}`);
  await Bun.sleep(4000);
  const first = readStatus();
  const build = JSON.parse(readFileSync(`${app}/build.json`, "utf8"));
  const samples: any[] = [];
  const start = Date.now();
  let prev = first;
  while (Date.now() - start < seconds * 1000) {
    await Bun.sleep(1000);
    const s = readStatus();
    if (s.frames === prev.frames) continue;
    samples.push({ t: (Date.now() - start) / 1000, frameMs: s.frameMs, worstMs: s.worstMs, late: s.late, frames: s.frames, cpuMs: s.cpuMs, gpuMs: s.gpuMs, draws: s.draws, drawn: s.drawn, tris: s.tris, places: s.places, reach: s.reach, hour: s.clock.hour, cells: s.cells, eye: s.eye, interface: s.interface });
    prev = s;
  }
  if (samples.length < 2) throw new Error("no samples: is the program on screen?");
  const last = samples.at(-1)!;
  const mean = (f: (s: any) => number) => samples.reduce((n, s) => n + f(s), 0) / samples.length;
  const frames = last.frames - first.frames;
  const late = last.late - first.late;
  const summary = {
    seconds,
    frames,
    lateFrames: late,
    lateShare: late / Math.max(frames, 1),
    averageFrameMs: mean((s) => s.frameMs),
    fps: 1000 / mean((s) => s.frameMs),
    worstFrameMs: Math.max(...samples.map((s) => s.worstMs)),
    triangles: { least: Math.min(...samples.map((s) => s.drawn)), mean: Math.round(mean((s) => s.drawn)), most: Math.max(...samples.map((s) => s.drawn)) },
    draws: { mean: Math.round(mean((s) => s.draws)), most: Math.max(...samples.map((s) => s.draws)) },
    cpuMs: { select: mean((s) => s.cpuMs.select), list: mean((s) => s.cpuMs.list) },
    gpuWaitMs: mean((s) => s.gpuMs),
    interface: last.interface && {
      up: last.interface.up,
      turnsPerSecond: (last.interface.turns - first.interface.turns) / seconds,
      turnMs: { script: mean((s) => s.interface.turnMs.script), layout: mean((s) => s.interface.turnMs.layout), worst: last.interface.turnMs.worst },
      cpuMsPerFrame: mean((s) => s.interface.cpuMs),
      listWords: Math.max(...samples.map((s) => s.interface.words)),
      bytes: last.interface.bytes,
    },
    hours: [first.clock.hour, last.hour],
    control: extra,
  };
  const dir = resolve(ROOT, `.pocket-build/validation/psp/bench-${new Date().toISOString().replace(/[:.]/g, "-")}`);
  mkdirSync(dir, { recursive: true });
  writeFileSync(`${dir}/device.json`, JSON.stringify({ summary, identity: { device: "psp:usb", ...build }, memory: readStatus().memory, samples }, null, 1));
  console.log(`${dir}/device.json`);
  console.log(JSON.stringify(summary, null, 1));
}

/** Runs `f` holding the PSP's lease; `--take` ends another holder first. */
async function device<T>(f: () => Promise<T>): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    try {
      return await withDeviceLease("psp:usb", f);
    } catch (e) {
      const m = /owner pid=(\d+)/.exec(String(e));
      if (!m || !argv.includes("--take") || attempt > 0) throw e;
      console.log(`psp: ending the holder of psp:usb (pid ${m[1]})`);
      process.kill(Number(m[1]), "SIGTERM");
      await Bun.sleep(800);
    }
  }
}

switch (cmd) {
  case "build":
    await build();
    break;
  case "run":
    if (!argv.includes("--no-build")) await build();
    await device(async () => {
      const id = stage(app);
      cpSync(`${OUT}/pocket-tokyo.prx`, `${share}/pocket-tokyo.prx`);
      stageInterface(app);
      writeFileSync(`${app}/build.json`, JSON.stringify({ prxSha256: sha(`${OUT}/pocket-tokyo.prx`), packSha256: id, interfaceSha256: Object.fromEntries(UI_FILES.map((f) => [f, sha(`${OUT}/${f}`)])) }, null, 1));
      writeFileSync(`${app}/boot.txt`, `${opt("--boot", "title=0")}\n`);
      rmSync(`${app}/status.json`, { force: true });
      // A reset restarts PSPLINK from the Memory Stick and drops the cable for several seconds. A command sent
      // before the cable is back leaves PSPLINK half reset (the shell answers, storage does not) until someone
      // restarts it on the console. So: reset, then nothing until usbhostfs_pc logs a new connection.
      // PSPLINK lists its own modules last: with nothing after USBHostFS, no program is loaded and no reset is needed.
      const names = (await pspsh("modlist").catch(() => "")).split("\n").filter((l) => l.includes("Name:")).map((l) => l.split("Name:")[1]!.trim());
      const idle = names.length > 0 && (names.at(-1) === "USBHostFS" || names.at(-1) === "PSPLINK");
      const before = logEnd();
      if (idle) {
        console.log("psp: PSPLINK is idle; loading without a reset");
      } else {
        console.log(`psp: resetting PSPLINK (${names.at(-1) ?? "module list unreadable"})`);
        await pspsh("reset").catch(() => "");
        if (before === undefined) {
          console.log("psp: no usbhostfs_pc log here (the host was started elsewhere); waiting 15 s for PSPLINK");
          await Bun.sleep(15000);
        } else {
          const end = Date.now() + 40000;
          while (!reconnected(before)) {
            if (Date.now() > end) throw new Error("PSPLINK did not reconnect after the reset: restart PSPLINK on the console");
            await Bun.sleep(250);
          }
          await Bun.sleep(1000);
        }
      }
      if (!/city\.pack/.test(await pspsh("ls host0:/tokyo"))) throw new Error("PSPLINK does not serve host0: restart PSPLINK on the console");
      await pspsh("ldstart host0:/pocket-tokyo.prx");
      const s = await waitFor((s) => s.stage === "running", 180);
      console.log(JSON.stringify(s, null, 1));
    });
    break;
  case "serve": {
    // One usbhostfs_pc owns the cable. This one outlives the command and logs where `run` can count its connections.
    if ((await $`pgrep -x usbhostfs_pc`.nothrow().quiet()).exitCode === 0) throw new Error("a usbhostfs_pc is already running; stop it first, or pass its directory with --share");
    mkdirSync(share, { recursive: true });
    mkdirSync(resolve(HOST_LOG, ".."), { recursive: true });
    const { spawn } = await import("node:child_process");
    const log = openSync(HOST_LOG, "w");
    spawn("usbhostfs_pc", ["-b", port, share], { cwd: share, detached: true, stdio: ["ignore", log, log] }).unref();
    console.log(`psp: usbhostfs_pc serves ${share} (log ${HOST_LOG})`);
    break;
  }
  case "status":
    console.log(JSON.stringify(readStatus(), null, 1));
    break;
  case "ctl":
    ctl(argv[1] ?? "");
    break;
  case "capture":
    await device(() => capture(resolve(opt("--out", `${ROOT}/.pocket-build/validation/psp/capture-${Date.now()}.png`))));
    break;
  case "bench":
    await device(() => bench(Number(opt("--seconds", "60")), opt("--ctl", "")));
    break;
  case "emu": {
    // PPSSPP mounts `--root` as host0:. Its software renderer follows the GE's clipping rules.
    const headless = process.env.PPSSPP_HEADLESS ?? `${process.env.HOME}/ppsspp-src/build/PPSSPPHeadless`;
    if (!existsSync(headless)) throw new Error(`no PPSSPPHeadless at ${headless} (set PPSSPP_HEADLESS)`);
    const root = `${ROOT}/.pocket-build/psp/emu`;
    // `--standalone` puts the pack beside the EBOOT, as on a Memory Stick, instead of on the share.
    const standalone = argv.includes("--standalone");
    mkdirSync(`${root}/tokyo`, { recursive: true });
    stage(standalone ? root : `${root}/tokyo`);
    rmSync(standalone ? `${root}/tokyo/city.pack` : `${root}/city.pack`, { force: true });
    // The interface's files and what it kept go where the pack is.
    // `--keep` leaves what the interface kept in the last run (`interface.json`) for this one to read.
    for (const f of [...UI_FILES, ...(argv.includes("--keep") ? [] : ["interface.json"])]) for (const dir of [root, `${root}/tokyo`]) rmSync(`${dir}/${f}`, { force: true });
    if (!argv.includes("--no-interface")) stageInterface(standalone ? root : `${root}/tokyo`);
    // `--small`: without the request for large memory the emulator gives the program a PSP-1000's 24 MB.
    if (argv.includes("--small")) await pbp(`${root}/EBOOT.PBP`, `${OUT}/pocket-tokyo.prx`, false);
    else cpSync(`${OUT}/EBOOT.PBP`, `${root}/EBOOT.PBP`);
    const frames = Number(opt("--frames", "240"));
    for (const f of ["status.json", "shot.raw"]) rmSync(`${root}/tokyo/${f}`, { force: true });
    writeFileSync(`${root}/tokyo/boot.txt`, `title=0 ${opt("--ctl", "")} shot=${frames} exit=${frames + 3}\n`);
    const run = await $`${headless} --root ${root} --graphics=${opt("--graphics", "software")} --timeout=${opt("--timeout", "240")} ${root}/EBOOT.PBP`.nothrow().quiet();
    const log = (run.stdout.toString() + run.stderr.toString()).trim();
    if (log) console.log(log.split("\n").slice(-12).join("\n"));
    if (existsSync(`${root}/tokyo/status.json`)) console.log(readFileSync(`${root}/tokyo/status.json`, "utf8"));
    if (!existsSync(`${root}/tokyo/shot.raw`)) throw new Error("the emulator run wrote no frame");
    const raw = readFileSync(`${root}/tokyo/shot.raw`);
    const rgba = new Uint8Array(480 * 272 * 4);
    // The frame buffer is 16-bit: red in the low five bits, then six of green, five of blue.
    for (let i = 0; i < 480 * 272; i++) {
      const p = raw[i * 2]! | (raw[i * 2 + 1]! << 8);
      rgba.set([((p & 31) * 255) / 31, (((p >> 5) & 63) * 255) / 63, ((p >> 11) * 255) / 31, 255], i * 4);
    }
    const out = resolve(opt("--out", `${ROOT}/.pocket-build/psp/emu/frame.png`));
    writeFileSync(out, encodePng(rgba, 480, 272));
    console.log(out);
    break;
  }
  case "package": {
    const dir = `${OUT}/PSP/GAME/PocketTokyo`;
    mkdirSync(dir, { recursive: true });
    cpSync(`${OUT}/EBOOT.PBP`, `${dir}/EBOOT.PBP`);
    stage(dir);
    stageInterface(dir);
    rmSync(`${dir}/city.json`, { force: true });
    console.log(`psp: copy ${OUT}/PSP to the root of a Memory Stick`);
    break;
  }
  default:
    console.log("usage: bun tools/psp.ts <build|serve|run|status|ctl|capture|bench|emu|package> [--share DIR] [--take]");
    process.exit(cmd ? 1 : 0);
}
