// Frame timings on the device over the tour.
//
//   bun tools/tokyo.ts bench [--seconds 150] [--hour 15.5] [--rate 0.03] [--ctl '{"budget":180000}'] [--share DIR]
//
// Starts the tour from its first place at the given hour, samples the running process's status receipt once a
// second and writes the evidence to `.pocket-build/validation/vita/bench-<time>/device.json`. The identity
// (native build, pack hash) comes from what the device reports and must not change during the window.

import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { DeviceEvidence, type DeviceIdentity } from "../vendor/pocketjs/tools/device-evidence.ts";
import { ctl, status } from "./vita.ts";

const ROOT = resolve(import.meta.dir, "..");

interface Sample {
  t: number;
  frameMs: number;
  worstMs: number;
  late: number;
  frames: number;
  cpuMs: number;
  triangles: number;
  draws: number;
  places: number[];
  scale: number;
  hour: number;
}

function identity(s: any): DeviceIdentity {
  if (s?.engine?.stage !== "running") throw new Error(`the device is not running the city (stage ${s?.engine?.stage ?? "unknown"})`);
  return { device: `vita:${s.titleId}`, runtimeBuild: s.nativeBuild, assets: { "city.pack": s.engine.pack.sha256 } };
}

export async function bench(argv: string[]) {
  const arg = (name: string, dflt: string) => {
    const i = argv.indexOf(name);
    return i >= 0 ? argv[i + 1] : dflt;
  };
  const seconds = Number(arg("--seconds", "150"));
  const extra = JSON.parse(arg("--ctl", "{}"));
  ctl(argv, JSON.stringify({ tour: true, restart: true, profile: false, view: null, hour: Number(arg("--hour", "15.5")), rate: Number(arg("--rate", "0.03")), budget: 200000, nonce: Date.now(), ...extra }));
  await Bun.sleep(2500);
  const first = status(argv);
  const evidence = new DeviceEvidence<Sample>(identity(first));
  const start = Date.now();
  let prev: any = first.engine;
  const samples: Sample[] = [];
  while (Date.now() - start < seconds * 1000) {
    await Bun.sleep(1000);
    const s = status(argv);
    const e = s.engine;
    if (e.frames === prev.frames) continue;
    const t = e.city.tris;
    const sample: Sample = { t: (Date.now() - start) / 1000, frameMs: e.frameMs, worstMs: e.worstMs, late: e.late, frames: e.frames, cpuMs: e.cpuMs.draw, triangles: t.top + t.wall + t.solid, draws: e.city.draws, places: e.city.places, scale: e.governor.scale, hour: e.clock.hour };
    evidence.observe(identity(s), sample);
    samples.push(sample);
    prev = e;
  }
  if (samples.length < 2) throw new Error("no samples: is the USB host running and the city on screen?");
  const frames = samples.at(-1)!.frames - first.engine.frames;
  const late = samples.at(-1)!.late - first.engine.late;
  const of = (f: (s: Sample) => number) => samples.map(f);
  const mean = (v: number[]) => v.reduce((a, b) => a + b, 0) / v.length;
  const summary = {
    seconds,
    frames,
    lateFrames: late,
    lateShare: late / Math.max(frames, 1),
    averageFrameMs: mean(of((s) => s.frameMs)),
    worstFrameMs: Math.max(...of((s) => s.worstMs)),
    fps: 1000 / mean(of((s) => s.frameMs)),
    triangles: { least: Math.min(...of((s) => s.triangles)), mean: Math.round(mean(of((s) => s.triangles))), most: Math.max(...of((s) => s.triangles)) },
    draws: { mean: Math.round(mean(of((s) => s.draws))), most: Math.max(...of((s) => s.draws)) },
    cpuDrawMs: mean(of((s) => s.cpuMs)),
    governorScale: { least: Math.min(...of((s) => s.scale)), mean: mean(of((s) => s.scale)) },
    hours: [samples[0]!.hour, samples.at(-1)!.hour],
    msaa: first.engine.msaa,
    control: extra,
  };
  const dir = resolve(ROOT, `.pocket-build/validation/vita/bench-${new Date().toISOString().replace(/[:.]/g, "-")}`);
  mkdirSync(dir, { recursive: true });
  writeFileSync(`${dir}/device.json`, JSON.stringify({ summary, ...evidence.receipt() }, null, 1));
  console.log(`${dir}/device.json`);
  console.log(JSON.stringify(summary, null, 1));
}
