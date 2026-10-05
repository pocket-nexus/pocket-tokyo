#!/usr/bin/env bun
/**
 * Compiles the interface (`ui/`, one PocketJS app) for a device. PocketJS
 * resolves `ui/pocket.json` against the device's profile, picks the
 * presentation its modality asks for and writes the bundle and its pak.
 *
 *   bun tools/ui.ts <psp|vita|3ds|ipod> [--area shiba]   → .pocket-build/ui/<device>/tokyo.{js,pak}, plan.json
 *   bun tools/ui.ts prepare [--area shiba]               → what the interface compiles from the area (ui/app/generated/)
 *   bun tools/ui.ts preview [device…]                    → pictures of every screen, .pocket-build/ui/preview/
 *   bun tools/ui.ts test                                 → the interface driven through a mock renderer
 */
import { existsSync, mkdirSync, statSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { POCKET_CAPABILITIES, definePlatformContractRegistry, defineTargetRegistry } from "../vendor/pocketjs/contracts/spec/platforms.ts";
import { extractHostBuildInputs, type HostBuildInputs } from "../vendor/pocketjs/framework/src/manifest/index.ts";
import { validateAndResolveBuildPlan } from "../vendor/pocketjs/framework/src/manifest/resolve.ts";
import { createCanvas, loadImage } from "../vendor/pocketjs/node_modules/@napi-rs/canvas";
import { resolve3dsBuildPlan } from "../vendor/pocketjs/tools/3ds-profile.ts";
import { IPODTOUCH4_DEV_HOST_ABI } from "../vendor/pocketjs/tools/ipodtouch4-profile.ts";

export const DEVICES = ["psp", "vita", "3ds", "ipod"] as const;
export type Device = (typeof DEVICES)[number];

const root = resolve(import.meta.dir, "..");
const pocket = join(root, "vendor/pocketjs");
const project = join(root, "ui");
const manifest = join(project, "pocket.json");
const generated = join(project, "app/generated");

/**
 * The iPod touch 4 as Pocket Tokyo presents it. PocketJS's own profile for
 * the device describes its host, which draws at the panel's 640×960; here the
 * interface shares the scene's drawable, 480×320 on its side at one sample
 * per logical pixel.
 */
export const IPOD_TARGET = "ipodtouch4-tokyo";
const IPOD_CONTRACTS = definePlatformContractRegistry(POCKET_CAPABILITIES, defineTargetRegistry({
  [IPOD_TARGET]: {
    hostAbi: IPODTOUCH4_DEV_HOST_ABI,
    platform: "ios",
    form: "takeover",
    display: { physicalViewport: [480, 320], logicalViewports: [[480, 320]], presentations: ["native"], rasterDensity: 1 },
    capabilities: ["input.touch", "text.glyphs.baked"],
  },
}));
function resolveIPodBuildPlan(manifest: unknown): unknown {
  const resolution = validateAndResolveBuildPlan(manifest, { target: IPOD_TARGET }, IPOD_CONTRACTS);
  if (!resolution.ok) throw new Error(`ui: ${resolution.diagnostics.map((d) => `${d.path || "/"}: ${d.message}`).join("; ")}`);
  return resolution.plan;
}
/** Devices outside PocketJS's public registry resolve through a profile kept with their tool. */
const PRIVATE: Partial<Record<Device, (manifest: unknown) => unknown>> = { "3ds": resolve3dsBuildPlan, ipod: resolveIPodBuildPlan };

export interface Interface {
  /** Directory holding `tokyo.js`, `tokyo.pak` and `plan.json`. */
  directory: string;
  inputs: HostBuildInputs;
  plan: { features: Record<string, boolean> };
}

function run(args: string[], cwd = pocket) {
  const done = Bun.spawnSync(args, { cwd, stdout: "inherit", stderr: "inherit" });
  if (done.exitCode !== 0) throw new Error(`ui: ${args.slice(0, 3).join(" ")} failed`);
}

/** The reference's projection (`web/src/shared/geo.js`): metres east and south of the origin. */
function projection(lon0: number, lat0: number) {
  const phi = (lat0 * Math.PI) / 180;
  const mLat = 111132.954 - 559.822 * Math.cos(2 * phi) + 1.175 * Math.cos(4 * phi);
  const mLon = 111412.84 * Math.cos(phi) - 93.5 * Math.cos(3 * phi);
  return (lon: number, lat: number): [number, number] => [(lon - lon0) * mLon, -(lat - lat0) * mLat];
}

/** Pixels a tile of the area takes on the map at one sample per logical pixel, and the side of the
 *  picture the map is stored in. */
const TILE_PX = 17;
const TEXTURE = 256;

/**
 * Writes what the interface compiles from the area (`ui/app/generated/`,
 * ignored): the area from above, the square of city that picture shows, and
 * the places the area file names, in the city's metres. The picture is the
 * export's own (`far_*.png`, each tile as the reference draws it from above),
 * so the map and the scene cannot disagree.
 */
export async function prepareInterface(device?: Device, area = "shiba"): Promise<void> {
  const ir = join(root, ".pocket-build/city", area, "ir");
  const manifestPath = join(ir, "manifest.json");
  if (!existsSync(manifestPath)) throw new Error(`ui: no exported area: run \`bun tools/tokyo.ts export --area ${area}\` first`);
  const city = await Bun.file(manifestPath).json();
  const areaPath = join(root, "areas", `${area}.json`);
  const source = await Bun.file(areaPath).json();
  mkdirSync(generated, { recursive: true });

  const glob = new Bun.Glob("far_*.png");
  const tiles = [...glob.scanSync(ir)].map((name) => {
    const [, x, z] = /^far_(-?\d+)_(-?\d+)\.png$/.exec(name)!;
    return { name, x: Number(x), z: Number(z) };
  });
  const x0 = Math.min(...tiles.map((t) => t.x)), z0 = Math.min(...tiles.map((t) => t.z));
  const across = Math.max(...tiles.map((t) => t.x)) - x0 + 1, down = Math.max(...tiles.map((t) => t.z)) - z0 + 1;
  // The Vita rasters two samples per logical pixel.
  const density = device === "vita" ? 2 : 1;
  const mapPath = join(generated, "map.png");
  if (across * TILE_PX > TEXTURE || down * TILE_PX > TEXTURE) throw new Error(`ui: the area is ${across} × ${down} tiles: more than the map's picture holds`);
  const stamp = join(generated, `.map-${area}-${density}`);
  if (!existsSync(mapPath) || !existsSync(stamp) || statSync(stamp).mtimeMs < statSync(manifestPath).mtimeMs) {
    const px = TILE_PX * density;
    // A pak image is a square of a power of two: the map stands at its top left.
    const canvas = createCanvas(TEXTURE * density, TEXTURE * density);
    const ctx = canvas.getContext("2d");
    ctx.fillStyle = "#3d4248";
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    ctx.imageSmoothingEnabled = true;
    ctx.imageSmoothingQuality = "high";
    for (const tile of tiles) ctx.drawImage(await loadImage(join(ir, tile.name)), (tile.x - x0) * px, (tile.z - z0) * px, px, px);
    // A map is read under marks and type: the picture steps back a little.
    ctx.fillStyle = "#0a0e1647";
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    for (const old of new Bun.Glob(".map-*").scanSync({ cwd: generated, dot: true })) await Bun.file(join(generated, old)).delete();
    writeFileSync(mapPath, canvas.toBuffer("image/png"));
    writeFileSync(stamp, "");
  }

  const toMetres = projection(city.origin.lon, city.origin.lat);
  const tile = city.tileSize as number;
  const places = ((source.places ?? []) as { name: string; lat: number; lon: number; reach?: number }[]).map((place) => {
    const [x, z] = toMetres(place.lon, place.lat);
    return { name: place.name, x: Math.round(x), z: Math.round(z), reach: place.reach ?? 320 };
  });
  writeFileSync(join(generated, "area.ts"), `// Generated by tools/ui.ts from areas/${area}.json and the exported area.

/** What the area is called, and what it lies in. */
export const AREA = ${JSON.stringify({ name: source.name, district: source.district ?? "" })};

/** The area from above: the picture (a square of \`texture\` logical pixels with the map at its top
 *  left), the map's size in it, and the city's metres at its left and top edges and per pixel.
 *  North is up. */
export const MAP = ${JSON.stringify({ image: "generated/map.png", texture: TEXTURE, width: across * TILE_PX, height: down * TILE_PX, x0: x0 * tile, z0: z0 * tile, metres: tile / TILE_PX })};

/** Named places, metres east and south of the area's origin; \`reach\` is how far from one the eye still is there. */
export const PLACES: { name: string; x: number; z: number; reach: number }[] = ${JSON.stringify(places, null, 2)};
`);
  // The PSP keeps the map as 16-bit texels; a map drawn smaller than it is stored is sampled between texels.
  writeFileSync(join(project, "app/images.json"), JSON.stringify({ "generated/map.png": device === "psp" ? { psm: 0, linear: true } : { linear: true } }, null, 2) + "\n");
}

export async function compileInterface(device: Device, area = "shiba"): Promise<Interface> {
  await prepareInterface(device, area);
  const directory = join(root, ".pocket-build/ui", device);
  mkdirSync(directory, { recursive: true });
  const planPath = join(directory, "plan.json");
  const resolver = PRIVATE[device];
  if (resolver) {
    writeFileSync(planPath, JSON.stringify(resolver(await Bun.file(manifest).json()), null, 2) + "\n");
    run(["bun", "tools/build.ts", `--plan=${planPath}`, `--project-root=${project}`, `--outdir=${directory}`]);
  } else {
    run(["bun", "tools/pocket.ts", "compile", "--target", device, "--manifest", manifest, "--project-root", project, "--outdir", directory]);
    await Bun.write(planPath, Bun.file(join(project, ".pocket", device, "plan.json")));
  }
  const plan = await Bun.file(planPath).json();
  return { directory, plan, inputs: extractHostBuildInputs(plan) };
}

if (import.meta.main) {
  const args = process.argv.slice(2);
  const at = args.indexOf("--area");
  const area = at >= 0 ? args.splice(at, 2)[1] : "shiba";
  const [command, ...rest] = args;
  if (command === "prepare") await prepareInterface(undefined, area);
  else if (command === "preview" || command === "test") {
    // Both run the bundle on PocketJS's UI core built for wasm.
    if (!existsSync(join(pocket, "hosts/web/pocketjs.wasm"))) run(["bun", "tools/wasm.ts"]);
    // One process per device: a bundle owns `globalThis.frame`.
    for (const device of rest.length ? (rest as Device[]) : DEVICES) {
      await compileInterface(device, area);
      run(["bun", `ui/test/${command === "test" ? "flow" : "preview"}.ts`, device], root);
    }
  } else if (DEVICES.includes(command as Device)) {
    const built = await compileInterface(command as Device, area);
    console.log(`${command}: ${built.inputs.target}, ${built.inputs.viewport.logical.join("×")} @${built.inputs.viewport.rasterDensity}x → ${built.directory}`);
  } else throw new Error(`usage: bun tools/ui.ts <${DEVICES.join("|")}|prepare|preview [device…]|test> [--area shiba]`);
}
