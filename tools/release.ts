#!/usr/bin/env bun
// Pocket Tokyo's packages for Pocket Studio: one file per device, built from
// the checked-out commit by the commands a developer runs (tools/tokyo.ts,
// psp.ts, n3ds.ts, ipod.ts).
//
//   bun tools/release.ts [--targets vita,psp,3ds,ipod-touch] [--out dist/release]
//                        [--vita-gxp DIR] [--no-build] [--upload]
//
// For each target it cooks the profile's pack from the exported area
// (.pocket-build/city/shiba/ir) with this commit's compiler, builds the
// program and writes one file to --out:
//
//   vita        pocket-tokyo-<version>.vpk        the pack, the interface and the console's programs inside
//   psp         pocket-tokyo-<version>-psp.zip    PSP/GAME/PocketTokyo/, for the root of a Memory Stick
//   3ds         pocket-tokyo-<version>.3dsx       the pack and the interface in its ROMFS
//   ipod-touch  pocket-tokyo-<version>-ipod.ipa   Payload/PocketTokyo.app
//
// and release.json beside them: the commit, the version (ui/pocket.json),
// each file's size and SHA-256, the inputs and the toolchains. A target that
// fails is reported, the others still build, and the exit status is 1.
//
// --vita-gxp DIR  The programs a console compiled: `manifest.txt` and the
//                 `<hash>.gxp` files a development run leaves in its share's
//                 `tokyo/gxp` (default .pocket-build/vita-usb/share/tokyo/gxp).
//                 SceShaccCg runs on the console only, so the package carries
//                 what it compiled. The set is refused when a program in it
//                 was not compiled from this commit's vita/shaders.
// --no-build      Take the packages release.json lists; their hashes are checked.
// --upload        Send each package to Pocket Studio with `pocket-studio package`,
//                 run here, where `pocket-studio register` wrote .pocket-studio.json.
//                 POCKET_STUDIO_CLI names the command when it is not on PATH.
//
// No package goes to Git or to a GitHub release (AGENTS.md).

import { closeSync, cpSync, existsSync, mkdirSync, openSync, readdirSync, readFileSync, readSync, rmSync, statSync, writeFileSync, writeSync } from "node:fs";
import { basename, join, resolve } from "node:path";
import { deflateRawSync } from "node:zlib";

const ROOT = resolve(import.meta.dir, "..");
const POCKETJS = join(ROOT, "vendor/pocketjs");
const WORK = join(ROOT, ".pocket-build/release");
const AREA = "shiba";
const NAME = "pocket-tokyo";
const TITLE = "Pocket Tokyo";

const argv = process.argv.slice(2);
const option = (key: string, fallback: string) => {
  const at = argv.indexOf(key);
  return at < 0 ? fallback : (argv[at + 1] ?? fallback);
};
const OUT = resolve(option("--out", join(ROOT, "dist/release")));

/** A package as release.json lists it. */
interface Package {
  target: Target;
  filename: string;
  bytes: number;
  sha256: string;
}

/** Pocket Studio's ids for the devices this repository builds for. It also takes `android`; nothing here builds one. */
const TARGETS = ["vita", "psp", "3ds", "ipod-touch"] as const;
type Target = (typeof TARGETS)[number];

/** What each target cooks and what its package is called. */
const BUILDS: Record<Target, { profile: string; filename: (version: string) => string; build: (log: string, output: string) => Promise<void> }> = {
  vita: { profile: "vita60", filename: (v) => `${NAME}-${v}.vpk`, build: vita },
  psp: { profile: "psp30", filename: (v) => `${NAME}-${v}-psp.zip`, build: psp },
  "3ds": { profile: "n3ds30", filename: (v) => `${NAME}-${v}.3dsx`, build: n3ds },
  "ipod-touch": { profile: "ipod60", filename: (v) => `${NAME}-${v}-ipod.ipa`, build: ipod },
};

const sha256 = (bytes: Uint8Array | string) => new Bun.CryptoHasher("sha256").update(bytes).digest("hex");
const fileSha256 = (path: string) => sha256(readFileSync(path));

/** The first line a command prints, or null when it cannot run. */
function line(command: string[], cwd = ROOT): string | null {
  try {
    const done = Bun.spawnSync(command, { cwd, stdout: "pipe", stderr: "pipe" });
    return done.exitCode === 0 ? (done.stdout.toString().trim().split("\n")[0] ?? "") : null;
  } catch {
    return null;
  }
}

/** Runs a build command with its output in `log`; a failure carries the log's last lines. */
async function run(log: string, command: string[], env: Record<string, string> = {}): Promise<void> {
  const fd = openSync(log, "a");
  writeSync(fd, `\n$ ${command.join(" ")}\n`);
  const code = await Bun.spawn(command, { cwd: ROOT, stdin: "ignore", stdout: fd, stderr: fd, env: { ...process.env, ...env } }).exited;
  closeSync(fd);
  if (code !== 0) throw new Error(`\`${command.join(" ")}\` exited ${code}; ${log} ends:\n${readFileSync(log, "utf8").trimEnd().split("\n").slice(-20).join("\n")}`);
}

// ---------------------------------------------------------------- archives

/** Every file and directory under `directory`, named from `prefix`. A directory's name ends in a slash and it has no path. */
function tree(directory: string, prefix: string): { name: string; path?: string }[] {
  return [
    { name: `${prefix}/` },
    ...readdirSync(directory, { withFileTypes: true }).flatMap((entry) =>
      entry.isDirectory() ? tree(join(directory, entry.name), `${prefix}/${entry.name}`) : [{ name: `${prefix}/${entry.name}`, path: join(directory, entry.name) }],
    ),
  ];
}

/**
 * Writes a zip whose bytes follow from its entries alone: the entries named
 * in `first`, then the others in the order of their names, every date
 * 1980-01-01 00:00, modes 0644, or 0755 for a directory and for a file its
 * owner may execute, no extra fields. An entry is deflated at level 6 unless
 * that makes it longer.
 */
function writeZip(output: string, entries: { name: string; path?: string }[], first: string[] = []): void {
  const rank = (name: string) => (first.includes(name) ? first.indexOf(name) : first.length);
  const fd = openSync(output, "w");
  const directory: Buffer[] = [];
  let at = 0;
  for (const entry of [...entries].sort((a, b) => rank(a.name) - rank(b.name) || (a.name < b.name ? -1 : a.name > b.name ? 1 : 0))) {
    const data = entry.path === undefined ? Buffer.alloc(0) : readFileSync(entry.path);
    const deflated = data.length ? deflateRawSync(data, { level: 6 }) : data;
    const body = deflated.length < data.length ? deflated : data;
    const method = body === data ? 0 : 8;
    const mode = entry.path === undefined ? 0o040755 : statSync(entry.path).mode & 0o100 ? 0o100755 : 0o100644;
    const name = Buffer.from(entry.name);
    const crc = Bun.hash.crc32(data);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(method, 8);
    local.writeUInt16LE(0x0021, 12);
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(body.length, 18);
    local.writeUInt32LE(data.length, 22);
    local.writeUInt16LE(name.length, 26);
    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    // Made on Unix, so the upper half of the external attributes is a file mode.
    central.writeUInt16LE((3 << 8) | 20, 4);
    local.copy(central, 6, 4, 30);
    central.writeUInt32LE(((mode << 16) | (entry.path === undefined ? 0x10 : 0)) >>> 0, 38);
    central.writeUInt32LE(at, 42);
    directory.push(central, name);
    for (const part of [local, name, body]) writeSync(fd, part);
    at += local.length + name.length + body.length;
  }
  const list = Buffer.concat(directory);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(directory.length / 2, 8);
  end.writeUInt16LE(directory.length / 2, 10);
  end.writeUInt32LE(list.length, 12);
  end.writeUInt32LE(at, 16);
  writeSync(fd, list);
  writeSync(fd, end);
  closeSync(fd);
}

/**
 * Writes a VPK again from what `vita-pack-vpk` packed. Its archive dates every entry with the time of the
 * build; this one holds the same files, `sce_sys/param.sfo` and `eboot.bin` first as it had them.
 */
function repackVpk(source: string, output: string): void {
  const unpacked = join(WORK, "vpk");
  rmSync(unpacked, { recursive: true, force: true });
  mkdirSync(unpacked, { recursive: true });
  const done = Bun.spawnSync(["unzip", "-q", "-o", source, "-d", unpacked], { stdout: "pipe", stderr: "pipe" });
  if (done.exitCode !== 0) throw new Error(`unzip ${source}: ${done.stderr.toString().trim()}`);
  const files = tree(unpacked, "").filter((entry) => entry.path !== undefined).map((entry) => ({ name: entry.name.slice(1), path: entry.path }));
  writeZip(output, files, ["sce_sys/param.sfo", "eboot.bin"]);
}

/** The id each release build carried, by target (release.json). */
const buildIds: Partial<Record<Target, string>> = {};

/**
 * The id a release build carries where a development build takes a random one: 32 hex digits from the
 * commit and the hashes of what the package is built from. The device tool reads it as POCKET_RELEASE_BUILD.
 */
function releaseBuild(target: Target, inputs: unknown): Record<string, string> {
  buildIds[target] = sha256(JSON.stringify([commit, dirty, target, inputs])).slice(0, 32);
  return { POCKET_RELEASE_BUILD: buildIds[target]! };
}

// ---------------------------------------------------------------- the Vita's programs

const FNV_PRIME = 0x100000001b3n;
const FNV_BASIS = 0xcbf29ce484222325n;
const U64 = (1n << 64n) - 1n;
/** The prime's inverse modulo 2^64: one FNV-1a step can be taken back. */
const FNV_INVERSE = (() => {
  let x = FNV_PRIME;
  for (let i = 0; i < 6; i++) x = (x * ((2n - FNV_PRIME * x) & U64)) & U64;
  return x;
})();

/** The hash before `bytes` were folded into `hash`. */
function unfold(hash: bigint, bytes: Uint8Array): bigint {
  for (let i = bytes.length - 1; i >= 0; i--) hash = ((hash * FNV_INVERSE) & U64) ^ BigInt(bytes[i]!);
  return hash;
}

/** `vita/shaders`, as one hash over each file's name and bytes in the order of the names. */
function shaderSources(): string {
  const hash = new Bun.CryptoHasher("sha256");
  for (const name of readdirSync(join(ROOT, "vita/shaders")).sort()) hash.update(name).update(readFileSync(join(ROOT, "vita/shaders", name)));
  return hash.digest("hex");
}

/**
 * Checks the programs a console compiled against this commit's sources.
 *
 * The console names a program by FNV-1a over its source and its stage
 * (`fnv64` in vita/src/gpu.rs): the source's bytes, 0xff, "v" or "f", 0xff.
 * The source is a Cg file with nothing before it, or a scene program's
 * prefix: the numeric `#define`s of `scene_defines` (vita/src/city.rs),
 * `frame.cg`, then `#define NIGHT` for a night program. FNV-1a steps are
 * invertible, so each name is taken back through a file of `vita/shaders` and
 * what precedes it: a program compiled from this commit's file arrives at the
 * hash's starting value, or at the one state every scene program shares.
 *
 * Refused: a name no file here leads back from (its source changed since the
 * console compiled it), a file no program was compiled from, a missing
 * `.gxp`. Not checked: the numeric defines themselves, which come from
 * constants in vita/src/city.rs and the pack's city record; release.json
 * records the state they hash to.
 */
function vitaPrograms(directory: string): { hashes: string[]; defines: string } {
  const manifest = join(directory, "manifest.txt");
  if (!existsSync(manifest)) {
    throw new Error(
      `no programs for the Vita: ${manifest} is missing. A console compiles them: run the development build there (\`bun tools/tokyo.ts native\`, README "From records to a frame"), then pass its share's tokyo/gxp as --vita-gxp`,
    );
  }
  const hashes = [...new Set(readFileSync(manifest, "utf8").split("\n").filter(Boolean))];
  const text = new TextEncoder();
  const shaders = join(ROOT, "vita/shaders");
  const frame = readFileSync(join(shaders, "frame.cg"));
  const files = readdirSync(shaders).filter((name) => /_[vf]\.cg$/.test(name)).sort();
  /** What a scene program adds after the frame's values (vita/src/city.rs). */
  const variants = ["", "#define NIGHT\n"];
  const plain = new Map<string, string>();
  const scene = new Map<string, { file: string; state: bigint }[]>();
  for (const hash of hashes) {
    if (!/^[0-9a-f]{16}$/.test(hash)) throw new Error(`${manifest}: "${hash}" is not a program's name`);
    if (!existsSync(join(directory, `${hash}.gxp`))) throw new Error(`${join(directory, `${hash}.gxp`)} is missing: the manifest lists a program the directory does not hold`);
    scene.set(hash, []);
    for (const file of files) {
      const stage = text.encode(file.endsWith("_v.cg") ? "v" : "f");
      const before = unfold(unfold(unfold(unfold(BigInt(`0x${hash}`), Uint8Array.of(0xff)), stage), Uint8Array.of(0xff)), readFileSync(join(shaders, file)));
      if (before === FNV_BASIS) plain.set(hash, file);
      for (const variant of variants) scene.get(hash)!.push({ file, state: unfold(unfold(before, text.encode(variant)), frame) });
    }
  }
  // The state after the numeric defines is the one most programs lead back to; a wrong file leads anywhere.
  const votes = new Map<bigint, number>();
  for (const [hash, found] of scene) if (!plain.has(hash)) for (const state of new Set(found.map((f) => f.state))) votes.set(state, (votes.get(state) ?? 0) + 1);
  const [defines, share] = [...votes].sort((a, b) => b[1] - a[1])[0] ?? [0n, 0];
  const from = new Map(plain);
  for (const [hash, found] of scene) {
    const match = share > 1 ? found.find((f) => f.state === defines) : undefined;
    if (!plain.has(hash) && match) from.set(hash, match.file);
  }
  const stale = hashes.filter((hash) => !from.has(hash));
  if (stale.length) {
    throw new Error(
      `the Vita's programs in ${directory} are stale: ${stale.join(", ")} ${stale.length === 1 ? "was" : "were"} not compiled from this commit's vita/shaders (${shaderSources().slice(0, 12)}). Run this commit's development build on a console and pass its programs`,
    );
  }
  const unused = files.filter((file) => ![...from.values()].includes(file));
  if (unused.length) throw new Error(`the Vita's programs in ${directory} are incomplete: none was compiled from ${unused.join(", ")}`);
  return { hashes, defines: defines.toString(16).padStart(16, "0") };
}

// ---------------------------------------------------------------- targets

let programs: { count: number; manifestSha256: string; sourcesSha256: string; sceneDefines: string } | undefined;

/** The standalone VPK (`tools/vita.ts` `vpk`), from a share that holds only the checked programs, written again with fixed dates. */
async function vita(log: string, output: string): Promise<void> {
  const source = resolve(option("--vita-gxp", join(ROOT, ".pocket-build/vita-usb/share/tokyo/gxp")));
  const checked = vitaPrograms(source);
  const share = join(WORK, "vita-share");
  rmSync(share, { recursive: true, force: true });
  mkdirSync(join(share, "tokyo/gxp"), { recursive: true });
  writeFileSync(join(share, "tokyo/gxp/manifest.txt"), checked.hashes.join("\n"));
  for (const hash of checked.hashes) cpSync(join(source, `${hash}.gxp`), join(share, `tokyo/gxp/${hash}.gxp`));
  programs = { count: checked.hashes.length, manifestSha256: sha256(checked.hashes.join("\n")), sourcesSha256: shaderSources(), sceneDefines: checked.defines };
  const pack = JSON.parse(readFileSync(join(ROOT, ".pocket-build/city", AREA, "vita60/receipt.json"), "utf8")).pack.sha256 as string;
  await run(log, ["bun", "tools/tokyo.ts", "vpk", "--share", share, "--area", AREA], releaseBuild("vita", [pack, programs.manifestSha256]));
  repackVpk(join(ROOT, "dist/vita/pocket-tokyo-PKTK00001.vpk"), output);
}

/** The Memory Stick folder (`tools/psp.ts` `package`), zipped from the card's root. */
async function psp(log: string, output: string): Promise<void> {
  rmSync(join(ROOT, "dist/psp/PSP"), { recursive: true, force: true });
  await run(log, ["bun", "tools/psp.ts", "build", "--area", AREA]);
  await run(log, ["bun", "tools/psp.ts", "package", "--area", AREA]);
  writeZip(output, [{ name: "PSP/" }, { name: "PSP/GAME/" }, ...tree(join(ROOT, "dist/psp/PSP/GAME/PocketTokyo"), "PSP/GAME/PocketTokyo")]);
}

/** The `.3dsx` (`tools/n3ds.ts` `build`, which refuses one over the 32 MiB the wire installs). Never a CIA. */
async function n3ds(log: string, output: string): Promise<void> {
  await run(log, ["bun", "tools/n3ds.ts", "build", "--area", AREA]);
  cpSync(join(ROOT, "dist/3ds/pocket-tokyo.3dsx"), output);
}

/** The app bundle (`tools/ipod.ts` `package`), zipped as an `.ipa`. */
async function ipod(log: string, output: string): Promise<void> {
  await run(log, ["bun", "tools/ipod.ts", "package", "--area", AREA]);
  writeZip(output, tree(join(ROOT, ".pocket-build/ipod/Payload"), "Payload"));
}

/** What Pocket Studio accepts as a package: its name, its first bytes and its size. */
function accept(path: string): void {
  const name = basename(path);
  const bytes = statSync(path).size;
  if (!/^[A-Za-z0-9._-]{1,80}$/.test(name)) throw new Error(`${name}: a package's name is 1 to 80 letters, digits, dots, underscores and hyphens`);
  if (bytes > 512 * 1024 * 1024) throw new Error(`${name} is ${bytes} bytes: a package is at most 512 MiB`);
  const head = Buffer.alloc(4);
  const fd = openSync(path, "r");
  readSync(fd, head, 0, 4, 0);
  closeSync(fd);
  const magic = name.endsWith(".3dsx") ? "3DSX" : "PK\x03\x04";
  if (head.toString("latin1") !== magic) throw new Error(`${name} does not start with the bytes of its format`);
}

// ---------------------------------------------------------------- what the build came from

/** The exported area the packs are cooked from: one hash over each file's name and bytes. */
function cityIr() {
  const directory = join(ROOT, ".pocket-build/city", AREA, "ir");
  if (!existsSync(join(directory, "manifest.json"))) {
    throw new Error(`no exported area at ${directory}: run \`bun tools/tokyo.ts fetch\`, \`tiles\` and \`export\` first (README "From records to a frame")`);
  }
  const manifest = JSON.parse(readFileSync(join(directory, "manifest.json"), "utf8"));
  const names = readdirSync(directory).sort();
  const hash = new Bun.CryptoHasher("sha256");
  for (const name of names) hash.update(name).update(readFileSync(join(directory, name)));
  return { area: AREA, format: manifest.format as number, compiled: manifest.compiled as string, files: names.length, sha256: hash.digest("hex") };
}

/** The toolchain a tool names in `rustup run <toolchain> cargo`. */
function named(tool: string): string | null {
  return / run (\S+) cargo /.exec(readFileSync(join(ROOT, tool), "utf8"))?.[1] ?? null;
}

function toolchains() {
  const rustc = (toolchain: string | null) => (toolchain ? `${toolchain}: ${line(["rustup", "run", toolchain, "rustc", "-V"]) ?? "not installed"}` : null);
  const pinned = (file: string) => JSON.parse(readFileSync(join(POCKETJS, "tools/cli", file), "utf8"));
  const vitasdk = process.env.VITASDK || `${process.env.HOME}/vitasdk`;
  const container = /"(devkitpro\/devkitarm@sha256:[0-9a-f]{64})"/.exec(readFileSync(join(POCKETJS, "tools/3ds-toolchain.ts"), "utf8"))?.[1] ?? null;
  return {
    pocketjs: line(["git", "-C", POCKETJS, "rev-parse", "HEAD"]),
    bun: Bun.version,
    rustc: {
      cook: line(["rustc", "-V"]),
      vita: rustc(named("tools/vita.ts")),
      psp: rustc(pinned("psp-toolchain.json").rust.toolchain),
      "3ds": rustc(named("tools/n3ds.ts")),
      "ipod-touch": rustc(pinned("iphone4s-toolchain.json").compiler.rustToolchain),
    },
    vitasdk: {
      gcc: line([`${vitasdk}/bin/arm-vita-eabi-gcc`, "--version"]),
      versionInfoSha256: existsSync(`${vitasdk}/version_info.txt`) ? fileSha256(`${vitasdk}/version_info.txt`) : null,
      cargoVita: line(["cargo", "vita", "--version"]),
    },
    pspSdk: pinned("psp-toolchain.json").sdk.sha256 as string,
    devkitarm: container,
    clang: line(["xcrun", "clang", "--version"]),
  };
}

// ---------------------------------------------------------------- Pocket Studio

/** Sends the packages with the Studio's own CLI, from this directory's project link. */
async function upload(packages: Package[], version: string): Promise<boolean> {
  const link = join(ROOT, ".pocket-studio.json");
  if (!existsSync(link)) {
    console.error(
      `release: ${link} is missing, so this checkout names no Pocket Studio project. Once, in ${ROOT}:\n` +
        `  pocket-studio link <CODE>                       # the link code from the room, when this computer is not linked to an account\n` +
        `  pocket-studio register --title "${TITLE}"   # writes .pocket-studio.json, which Git ignores\n` +
        `then: bun tools/release.ts --no-build --upload`,
    );
    return false;
  }
  const project = JSON.parse(readFileSync(link, "utf8"));
  if (project.kind !== "site") throw new Error(`${link} links a device session, not a registered game: run \`pocket-studio register --title "${TITLE}"\` in a directory without one`);
  const cli = process.env.POCKET_STUDIO_CLI?.trim().split(/\s+/) ?? (Bun.which("pocket-studio") ? ["pocket-studio"] : null);
  if (!cli) throw new Error("no pocket-studio on PATH: install it from the Studio, or set POCKET_STUDIO_CLI to its command");
  console.log(`release: uploading ${packages.length} package(s) to ${project.server} (${project.app})`);
  for (const pkg of packages) {
    const command = [...cli, "package", join(OUT, pkg.filename), "--target", pkg.target, "--version", version];
    console.log(`$ ${command.join(" ")}`);
    const code = await Bun.spawn(command, { cwd: ROOT, stdin: "ignore", stdout: "inherit", stderr: "inherit" }).exited;
    if (code !== 0) throw new Error(`pocket-studio package exited ${code} for ${pkg.filename}`);
  }
  return true;
}

// ---------------------------------------------------------------- main

const asked = option("--targets", TARGETS.join(",")).split(",").filter(Boolean);
const unknown = asked.filter((t) => !(TARGETS as readonly string[]).includes(t));
if (unknown.length || argv.includes("--help")) {
  if (unknown.length) console.error(`release: no build for ${unknown.join(", ")}: the targets are ${TARGETS.join(", ")}`);
  console.log("usage: bun tools/release.ts [--targets vita,psp,3ds,ipod-touch] [--out dist/release] [--vita-gxp DIR] [--no-build] [--upload]");
  process.exit(unknown.length ? 1 : 0);
}
const targets = TARGETS.filter((t) => asked.includes(t));
const version = JSON.parse(readFileSync(join(ROOT, "ui/pocket.json"), "utf8")).version as string;
const commit = line(["git", "rev-parse", "HEAD"]);
const dirty = Bun.spawnSync(["git", "status", "--porcelain"], { cwd: ROOT }).stdout.toString().trim() !== "";
const record = join(OUT, "release.json");
if (argv.includes("--upload") && dirty) throw new Error("the checkout has uncommitted changes: a package names the commit it was built from, so commit before --upload");
let packages: Package[] = [];
const failed: { target: Target; error: string }[] = [];

if (argv.includes("--no-build")) {
  if (!existsSync(record)) throw new Error(`${record} is missing: run without --no-build first`);
  const built = JSON.parse(readFileSync(record, "utf8"));
  if (built.commit !== commit) throw new Error(`${record} was built from ${built.commit}; the checkout is at ${commit}`);
  packages = (built.packages as Package[]).filter((p) => targets.includes(p.target));
  for (const pkg of packages) {
    if (fileSha256(join(OUT, pkg.filename)) !== pkg.sha256) throw new Error(`${pkg.filename} is not the file release.json lists`);
    console.log(`release: ${pkg.target.padEnd(10)} ${pkg.filename}  ${pkg.bytes} bytes  sha256 ${pkg.sha256}`);
  }
} else {
  mkdirSync(OUT, { recursive: true });
  mkdirSync(join(WORK, "logs"), { recursive: true });
  console.log(`release: ${TITLE} ${version} at ${commit}${dirty ? " with uncommitted changes" : ""}`);
  const ir = cityIr();
  const packs: Record<string, { bytes: number; sha256: string }> = {};
  const seconds: Partial<Record<Target, number>> = {};
  // One at a time: each device's interface is compiled into the same generated files under ui/.
  for (const target of targets) {
    const { profile, filename, build } = BUILDS[target];
    const log = join(WORK, "logs", `${target}.log`);
    const output = join(OUT, filename(version));
    const start = performance.now();
    rmSync(log, { force: true });
    rmSync(output, { force: true });
    try {
      console.log(`release: ${target}: cooking ${profile}, building (log: ${log})`);
      await run(log, ["bun", "tools/tokyo.ts", "cook", "--area", AREA, "--profile", profile]);
      packs[profile] = JSON.parse(readFileSync(join(ROOT, ".pocket-build/city", AREA, profile, "receipt.json"), "utf8")).pack;
      await build(log, output);
      accept(output);
      packages.push({ target, filename: filename(version), bytes: statSync(output).size, sha256: fileSha256(output) });
    } catch (error) {
      rmSync(output, { force: true });
      failed.push({ target, error: error instanceof Error ? error.message : String(error) });
      console.error(`release: ${target} failed: ${failed.at(-1)!.error}`);
    }
    seconds[target] = Math.round((performance.now() - start) / 100) / 10;
  }
  writeFileSync(
    record,
    JSON.stringify(
      {
        schema: 1,
        name: NAME,
        title: TITLE,
        version,
        commit,
        dirty,
        packages,
        failed: failed.map(({ target, error }) => ({ target, error: error.split("\n")[0] })),
        inputs: { cityIr: ir, packs, vitaPrograms: programs ?? null, buildIds },
        toolchains: toolchains(),
      },
      null,
      2,
    ) + "\n",
  );
  for (const pkg of packages) console.log(`release: ${pkg.target.padEnd(10)} ${pkg.filename}  ${pkg.bytes} bytes  sha256 ${pkg.sha256}  ${seconds[pkg.target]} s`);
  for (const { target } of failed) console.log(`release: ${target.padEnd(10)} not built  ${seconds[target]} s`);
  console.log(`release: ${record}`);
}

if (argv.includes("--upload")) {
  if (failed.length) throw new Error(`not uploading: ${failed.map((f) => f.target).join(", ")} did not build`);
  if (!(await upload(packages, version))) process.exit(1);
}
process.exit(failed.length ? 1 : 0);
