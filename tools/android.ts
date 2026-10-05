#!/usr/bin/env bun
/**
 * Pocket Tokyo on the Redmi 1S (Android 4.3, Adreno 305): cook, build, package,
 * install, drive and measure. The app is one NativeActivity: the C in
 * android/src, the Rust core in android/core and the interface in ui/, with
 * PocketJS's UI core, QuickJS and guest driver linked in. The NDK compiles
 * and links it and the SDK's build tools package it; there is no Java and no
 * Gradle.
 *
 *   bun tools/android.ts doctor                  the tools and the phone this needs
 *   bun tools/android.ts cook [--area shiba]     the city for profiles/redmi1s60.json → .pocket-build/city/<area>/redmi1s60/city.pack
 *   bun tools/android.ts build                   the two libraries and the interface → .pocket-build/android/
 *   bun tools/android.ts apk                     build, then the signed package with the pack inside → dist/android/PocketTokyo.apk
 *   bun tools/android.ts install                 apk, then `adb install`, answering MIUI's two questions on the phone
 *   bun tools/android.ts apk --release           the package members install → dist/android/PocketTokyo-release.apk
 *   bun tools/android.ts install --release       that package on the phone, started
 *   bun tools/android.ts native [--pack]         build, then replace the engine and the interface (and the pack) in the app's data: no install
 *   bun tools/android.ts boot "samples=2"        words a development launch reads (samples, width, height, title=0); `boot ""` clears them
 *   bun tools/android.ts launch                  start it and wait for its first frames of the city
 *   bun tools/android.ts stop | status
 *   bun tools/android.ts ctl "ui=fly hour=19"    words for the shell, the flow and the flight (android/src/main.c, android/core/src/lib.rs)
 *   bun tools/android.ts capture [--out PNG]     the frame as the app drew it, interface and all
 *   bun tools/android.ts screen [--out PNG]      the panel as the system composed it (`screencap`)
 *   bun tools/android.ts title [--out PNG]       launches, and brings back the Pocket3D title card's held frame
 *   bun tools/android.ts bench [--seconds 150] [--ctl "budget=90000"]   the tour's frame timings, the instruments over it → .pocket-build/validation/android/
 *   bun tools/android.ts reset [--dev]           stops the app and removes what it kept (the settings; with --dev the development copies too)
 *
 * A development package is `android:debuggable` and signed with the phone's debug key: `native`, `boot`,
 * `launch`, `status`, `ctl`, `capture`, `title`, `bench` and `reset` reach the app's data through run-as.
 * A release (`--release`) is not debuggable, reads no development copy from `files/dev` and is signed with the
 * Pocket Nexus Android release key: the PKCS #12 keystore POCKET_NEXUS_ANDROID_KEY names (default
 * ~/.config/pocket-nexus/signing/pocket-nexus-android-release.p12, alias `pocket-nexus`), its password the
 * first line of the `.password` file beside it. A phone installs a package over an installed one only under
 * the same certificate: a development package is uninstalled before a release goes on, and the other way round.
 *
 * The phone is the one device `adb` lists (or ANDROID_SERIAL).
 */
import { createHash, randomBytes } from "node:crypto";
import { cpSync, existsSync, mkdirSync, readFileSync, rmSync, statSync, utimesSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { ensureQuickJsCheckout, quickJsCheckout } from "../vendor/pocketjs/tools/native-source.ts";
import { POCKET3D_ICON_ANDROID } from "../vendor/pocketjs/tools/pocket3d-icon.ts";
import { compileInterface } from "./ui.ts";

const root = resolve(import.meta.dir, "..");
const args = Bun.argv.slice(2);
const command = args[0] ?? "doctor";
const option = (key: string, fallback = "") => (args.includes(key) ? (args[args.indexOf(key) + 1] ?? fallback) : fallback);
const area = option("--area", "shiba");
const out = join(root, ".pocket-build/android");
const validation = join(root, ".pocket-build/validation/android");
const pack = join(root, `.pocket-build/city/${area}/redmi1s60/city.pack`);
/** A release: not debuggable, no development copies, signed with the release key. */
const release = args.includes("--release");
const apk = join(root, "dist/android", release ? "PocketTokyo-release.apk" : "PocketTokyo.apk");
const pocket = join(root, "vendor/pocketjs");
const id = "dev.pocketnexus.tokyo";
const version = { code: 1, name: "0.1.0" };

/** What builds for Android 4.3 on ARMv7: the NDK that still has API 18, and the compiler PocketJS's UI core is built with. */
const TOOLCHAIN = {
  sdk: process.env.ANDROID_HOME ?? "/opt/homebrew/share/android-commandlinetools",
  ndk: "21.4.7075529",
  buildTools: "34.0.0",
  platform: "android-34",
  api: 18,
  rust: "nightly-2026-07-02",
  rustTarget: "armv7-linux-androideabi",
  quickjs: { version: "2026-06-04", repository: "https://github.com/pocket-nexus/quickjs-rs.git", revision: "ba5bdd0dc013518768e76cd9e05cd30ed53dd35b" },
};
const ndk = join(TOOLCHAIN.sdk, "ndk", TOOLCHAIN.ndk);
const llvm = join(ndk, "toolchains/llvm/prebuilt", process.platform === "darwin" ? "darwin-x86_64" : "linux-x86_64", "bin");
const clang = join(llvm, `armv7a-linux-androideabi${TOOLCHAIN.api}-clang`);
const glue = join(ndk, "sources/android/native_app_glue");
const buildTools = join(TOOLCHAIN.sdk, "build-tools", TOOLCHAIN.buildTools);
const androidJar = join(TOOLCHAIN.sdk, "platforms", TOOLCHAIN.platform, "android.jar");
const cache = join(homedir(), ".cache/pocket-nexus/android");
const quickJsRoot = join(cache, "sources/quickjs-rs");
// A development package's key: the one the phone's first PocketJS build was signed with, so later ones install over each other.
const keystore = [join(homedir(), ".cache/pocket-nexus/redmi1s/signing/pocketjs-redmi1s-debug.keystore"), join(cache, "signing/pocket-tokyo-debug.keystore")];

function run(cmd: string[], cwd = root, env?: Record<string, string>, stdin?: Uint8Array): string {
  const p = Bun.spawnSync(cmd, { cwd, stdin, stdout: "pipe", stderr: "pipe", env: env ? { ...process.env, ...env } : undefined });
  if (p.exitCode) throw new Error(`${cmd.slice(0, 3).join(" ")}: ${p.stdout.toString()}${p.stderr.toString()}`);
  return p.stdout.toString().trim();
}
const sha = (file: string) => createHash("sha256").update(readFileSync(file)).digest("hex");

// ---------------------------------------------------------------- the phone

function serial(): string {
  if (process.env.ANDROID_SERIAL) return process.env.ANDROID_SERIAL;
  const devices = run(["adb", "devices"]).split("\n").slice(1).map((l) => l.trim().split(/\s+/)).filter((p) => p[1] === "device").map((p) => p[0]);
  if (devices.length !== 1) throw new Error(`expected one device on adb, found ${devices.length}: set ANDROID_SERIAL`);
  return devices[0];
}
let phone = "";
/** `adb` on the phone. Its shell prints "open: Permission denied" before some commands on this ROM: dropped. */
function adb(...words: string[]): string {
  phone ||= serial();
  return run(["adb", "-s", phone, ...words]).replace(/\r/g, "").split("\n").filter((l) => l !== "open: Permission denied").join("\n");
}
const shell = (script: string) => adb("shell", script);
/** A script as the app's own user, in the app's data (the package is debuggable). */
const inApp = (script: string) => shell(`run-as ${id} sh -c '${script}'`);

function status(): Record<string, any> | undefined {
  try {
    return JSON.parse(inApp("cat files/status.json"));
  } catch {
    return undefined;
  }
}

/** Puts a file into the app's data: through the shell's own folder, since only the app may write its data. */
function give(from: string, to: string) {
  const name = `tokyo-${randomBytes(4).toString("hex")}`;
  adb("push", from, `/data/local/tmp/${name}`);
  // (this Android's shell has no `dirname`)
  inApp(`mkdir -p ${to.slice(0, to.lastIndexOf("/"))}; cat /data/local/tmp/${name} > ${to}.part && mv ${to}.part ${to}`);
  shell(`rm /data/local/tmp/${name}`);
}

/** Takes a file out of the app's data. `adb shell` turns line ends on this Android, so the bytes go by the card. */
function take(from: string, to: string) {
  const name = `/sdcard/tokyo-${randomBytes(4).toString("hex")}`;
  shell(`run-as ${id} cat ${from} > ${name}`);
  adb("pull", name, to);
  shell(`rm ${name}`);
}

async function send(words: string, wait = 10): Promise<Record<string, any>> {
  const nonce = randomBytes(6).toString("hex");
  const local = join(out, "control.txt");
  mkdirSync(out, { recursive: true });
  writeFileSync(local, `${nonce}\n${words}\n`);
  give(local, "files/control.txt");
  for (let i = 0; i < wait * 4; i++) {
    await Bun.sleep(250);
    const s = status();
    if (s?.command === nonce) return s;
  }
  throw new Error(`the app did not take the command (${words}): is it running?`);
}

/**
 * MIUI asks on the phone before every install over USB, on two screens ("Replace app": OK, then "Install").
 * This answers them: it finds each button's bounds in the screen's hierarchy and taps its middle.
 */
async function install(file: string) {
  phone ||= serial();
  const installing = Bun.spawn(["adb", "-s", phone, "install", "-r", file], { stdout: "pipe", stderr: "pipe" });
  let done = false;
  installing.exited.then(() => (done = true));
  while (!done) {
    await Bun.sleep(1500);
    if (!/mCurrentFocus=.*PackageInstallerActivity/.test(shell("dumpsys window"))) continue;
    shell("uiautomator dump /sdcard/tokyo-ui.xml");
    const m = shell("cat /sdcard/tokyo-ui.xml").match(/text="(?:OK|Install)"[^>]*bounds="\[(\d+),(\d+)\]\[(\d+),(\d+)\]"/);
    if (m) shell(`input tap ${(+m[1] + +m[3]) >> 1} ${(+m[2] + +m[4]) >> 1}`);
  }
  const said = (await new Response(installing.stdout).text()) + (await new Response(installing.stderr).text());
  if (!/Success/.test(said)) throw new Error(`install: ${said.trim().split("\n").pop()}`);
}

// ---------------------------------------------------------------- build

function doctor(): boolean {
  const checks: [string, boolean, string][] = [
    ["NDK clang for API 18", existsSync(clang), clang],
    ["native_app_glue", existsSync(join(glue, "android_native_app_glue.c")), glue],
    ["aapt, zipalign, apksigner", ["aapt", "zipalign", "apksigner"].every((t) => existsSync(join(buildTools, t))), buildTools],
    ["android.jar", existsSync(androidJar), androidJar],
    ["Rust target", run(["rustup", "target", "list", "--installed", "--toolchain", TOOLCHAIN.rust]).split("\n").includes(TOOLCHAIN.rustTarget), `${TOOLCHAIN.rustTarget} on ${TOOLCHAIN.rust}`],
    ["PocketJS's packages", existsSync(join(pocket, "node_modules")), "bun install in vendor/pocketjs"],
    ["the city's pack", existsSync(pack), pack],
  ];
  let ok = true;
  for (const [label, good, detail] of checks) {
    console.log(`[${good ? "ok" : "missing"}] ${label}: ${detail}`);
    ok &&= good;
  }
  try {
    const p = (key: string) => shell(`getprop ${key}`).trim();
    console.log(`[ok] phone: ${serial()} ${p("ro.product.model")} (${p("ro.product.device")}), Android ${p("ro.build.version.release")}, API ${p("ro.build.version.sdk")}, ${p("ro.board.platform")}`);
  } catch (error) {
    console.log(`[missing] phone: ${(error as Error).message}`);
  }
  return ok;
}

function cook() {
  if (!existsSync(join(root, `.pocket-build/city/${area}/ir`))) throw new Error("no exported city: run `bun tools/tokyo.ts export` first");
  console.log(run(["cargo", "run", "--release", "-q", "-p", "tokyo-cook", "--", "--in", `.pocket-build/city/${area}/ir`, "--out", `.pocket-build/city/${area}/redmi1s60`,
    "--profile", "profiles/redmi1s60.json", "--area", `areas/${area}.json`]));
}

async function build(): Promise<{ engine: string; loader: string; ui: string; build: string }> {
  if (!existsSync(clang)) throw new Error(`no NDK ${TOOLCHAIN.ndk}: sdkmanager "ndk;${TOOLCHAIN.ndk}"`);
  const ui = await compileInterface("android", area);
  const objects = join(out, "objects");
  mkdirSync(objects, { recursive: true });
  ensureQuickJsCheckout("Pocket Tokyo", quickJsRoot, TOOLCHAIN.quickjs);
  const quickjs = quickJsCheckout(quickJsRoot);
  const cargo = (directory: string, target: string, extra: string[] = []) =>
    run(["rustup", "run", TOOLCHAIN.rust, "cargo", "build", "--release", "--target", TOOLCHAIN.rustTarget, ...extra], directory,
      { CARGO_TARGET_DIR: target, CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER: clang, RUSTFLAGS: "-C target-cpu=cortex-a7 -C target-feature=+neon,+vfp4" });
  // The city: the pack, the frame, the flight and its flow, the title card's frames. It also answers the guest's service wire.
  cargo(join(root, "android/core"), join(out, "core"));
  // The interface's runtime, from PocketJS: the UI core as a static library with its OpenGL ES backend.
  cargo(join(pocket, "engine/ui-cabi"), join(out, "ui-core"), ["--locked", "--features", "bare-platform"]);
  const libraries = [join(out, "core", TOOLCHAIN.rustTarget, "release/libtokyo_android_core.a"), join(out, "ui-core", TOOLCHAIN.rustTarget, "release/libpocketjs_symbian_core.a")];

  // The Cortex-A7 has NEON and a fused multiply-add: scalar float goes through them.
  const machine = ["-mcpu=cortex-a7", "-mfpu=neon-vfpv4", "-mfloat-abi=softfp", "-mthumb", "-fPIC", "-ffunction-sections", "-fdata-sections", "-DANDROID"];
  const compile = (source: string, extra: string[] = [], level = "-O2") => {
    const object = join(objects, source.replace(/[^A-Za-z0-9]/g, "_").slice(-80) + ".o");
    run([clang, "-std=gnu11", level, ...machine, ...extra, "-c", source, "-o", object]);
    return object;
  };
  const includes = ["-I", join(pocket, "engine/quickjs-c"), "-I", join(pocket, "engine/ui-cabi/include"), "-I", join(pocket, "contracts/generated"),
    "-I", join(pocket, "hosts/ios-legacy"), "-I", join(pocket, "hosts/shared"), "-I", join(pocket, "hosts/blackberry-classic"), "-isystem", quickjs.source, "-I", glue];
  const guest = [
    ...["quickjs.c", "cutils.c", "dtoa.c", "libregexp.c", "libunicode.c"].map((f) => compile(join(quickjs.source, f),
      ["-I", quickjs.source, "-funsigned-char", "-fno-strict-aliasing", "-fwrapv", "-D_GNU_SOURCE", "-w", `-DCONFIG_VERSION="${TOOLCHAIN.quickjs.version}"`])),
    compile(quickjs.staticFunctions, ["-I", quickjs.source, "-funsigned-char", "-D_GNU_SOURCE", "-w", `-DCONFIG_VERSION="${TOOLCHAIN.quickjs.version}"`]),
    // The guest's service ops go to the svcwire_* functions, which the core answers in the process.
    compile(join(pocket, "engine/quickjs-c/pocket_runtime.c"), [...includes, "-DPOCKET_SVC_WIRE", `-DPOCKETJS_TARGET_ID="${ui.inputs.target}"`,
      `-DPOCKETJS_HOST_ABI=${ui.inputs.hostAbi}`, `-DPOCKET_RASTER_DENSITY=${ui.inputs.viewport.rasterDensity}`]),
  ];
  const sources = ["android/src/main.c", "android/src/bionic18.c", "android/src/loader.c", "android/src/core.h"];
  const build = createHash("sha256").update([...sources.map((f) => join(root, f)), ...libraries, join(ui.directory, "tokyo.js"), join(ui.directory, "tokyo.pak")].map(sha).join() + (release ? " release" : "")).digest("hex").slice(0, 12);
  const [logicalW, logicalH] = ui.inputs.viewport.logical;
  // A release never looks in files/dev (android/src/main.c, android/src/loader.c).
  const door = release ? ["-DTOKYO_RELEASE"] : [];
  const strict = ["-Wall", "-Wextra", "-Werror", "-Wno-unused-parameter", ...door, `-DTOKYO_BUILD="${build}"`, `-DLOGICAL_WIDTH=${logicalW}`, `-DLOGICAL_HEIGHT=${logicalH}`, ...includes];
  for (const key of ["--samples", "--buffer-width", "--buffer-height"])
    if (option(key)) strict.push(`-D${key.slice(2).replace("-", "_").toUpperCase()}=${option(key)}`);
  const own = [compile(join(root, "android/src/main.c"), strict, "-O3"), compile(join(root, "android/src/bionic18.c"), ["-Wall", "-Wextra", "-Werror"]),
    compile(join(glue, "android_native_app_glue.c"), ["-I", glue])];
  const staged = join(out, "apk/lib/armeabi-v7a");
  mkdirSync(staged, { recursive: true });
  const engine = join(staged, "libtokyo-engine.so"), loader = join(staged, "libtokyo.so");
  // `--no-undefined`: a function this Android does not have fails here, not when the phone loads the library.
  run([clang, "-shared", "-Wl,--no-undefined", "-Wl,--gc-sections", "-Wl,-soname,libtokyo-engine.so", "-u", "ANativeActivity_onCreate", "-o", engine,
    ...own, ...guest, libraries[0], libraries[1], "-landroid", "-llog", "-lEGL", "-lGLESv3", "-ldl", "-lm"]);
  run([clang, "-shared", "-Wl,--no-undefined", "-Wl,-soname,libtokyo.so", "-o", loader, compile(join(root, "android/src/loader.c"), ["-Wall", "-Wextra", "-Werror", ...door]), "-landroid", "-llog", "-ldl"]);
  for (const library of [engine, loader]) run([join(llvm, "llvm-strip"), "--strip-unneeded", library]);
  console.log(JSON.stringify({ build, engine: statSync(engine).size, loader: statSync(loader).size, interface: `${logicalW}×${logicalH} @${ui.inputs.viewport.rasterDensity}x` }));
  return { engine, loader, ui: ui.directory, build };
}

/**
 * What a package is signed with. A release: the Pocket Nexus Android release key, one key for every Pocket Nexus
 * package, kept outside every repository. apksigner reads the password from its file, so it is in no command
 * line and no log.
 */
function signer(): string[] {
  if (!release) return ["--ks", signingKey(), "--ks-pass", "pass:android", "--key-pass", "pass:android", "--ks-key-alias", "pocketjs"];
  const key = process.env.POCKET_NEXUS_ANDROID_KEY || join(homedir(), ".config/pocket-nexus/signing/pocket-nexus-android-release.p12");
  const password = key.replace(/\.[^./]*$/, "") + ".password";
  for (const file of [key, password])
    if (!existsSync(file)) throw new Error(`a release is signed with the Pocket Nexus Android release key, and ${file} is not there (POCKET_NEXUS_ANDROID_KEY names the keystore; its password is the first line of the .password file beside it)`);
  return ["--ks", key, "--ks-type", "PKCS12", "--ks-pass", `file:${password}`, "--ks-key-alias", "pocket-nexus"];
}

function signingKey(): string {
  const found = keystore.find(existsSync);
  if (found) return found;
  mkdirSync(join(cache, "signing"), { recursive: true });
  run(["keytool", "-genkeypair", "-keystore", keystore[1], "-storepass", "android", "-keypass", "android", "-alias", "pocketjs", "-keyalg", "RSA", "-keysize", "2048", "-validity", "10000",
    "-dname", "CN=Pocket Tokyo,O=Pocket Nexus,C=US"]);
  return keystore[1];
}

async function packageApk() {
  if (!existsSync(pack)) throw new Error("the city is not cooked for this phone: bun tools/android.ts cook");
  // Before the build: a release without its key stops here.
  const key = signer();
  const built = await build();
  const staging = join(out, "apk");
  const res = join(staging, "res"), assets = join(staging, "assets");
  rmSync(res, { recursive: true, force: true });
  rmSync(assets, { recursive: true, force: true });
  mkdirSync(assets, { recursive: true });
  // The launcher's icon is the Pocket3D icon from PocketJS, the file of each density as it is.
  for (const [density, file] of Object.entries(POCKET3D_ICON_ANDROID)) {
    mkdirSync(join(res, `drawable-${density}`), { recursive: true });
    cpSync(file, join(res, `drawable-${density}`, "icon.png"));
  }
  for (const file of ["tokyo.js", "tokyo.pak"]) cpSync(join(built.ui, file), join(assets, file));
  cpSync(pack, join(assets, "city.pack"));
  const manifest = join(staging, "AndroidManifest.xml");
  writeFileSync(manifest, readFileSync(join(root, "android/AndroidManifest.xml"), "utf8").replace("@VERSION_CODE@", String(version.code)).replace("@VERSION_NAME@", version.name).replace("@DEBUGGABLE@", String(!release)));
  const unsigned = join(out, "unsigned.apk"), aligned = join(out, "aligned.apk");
  // `-0 pack`: the pack is stored as it is, so the app reads it in place through a file descriptor.
  // `--no-crunch`: the icons go in as PocketJS's files are, byte for byte.
  run([join(buildTools, "aapt"), "package", "-f", "--no-crunch", "-0", "pack", "-M", manifest, "-S", res, "-A", assets, "-I", androidJar, "-F", unsigned]);
  // aapt dates its entries 1980-01-01. `zip` dates an entry by its file and adds the file's access time; apksigner
  // dates its own entries by the last one it is given. The libraries take aapt's date and `-X` leaves the access
  // times out, so two packages of the same contents are the same bytes.
  for (const library of [built.engine, built.loader]) utimesSync(library, new Date(1980, 0, 1), new Date(1980, 0, 1));
  run(["zip", "-q", "-X", "-r", unsigned, "lib"], staging);
  run([join(buildTools, "zipalign"), "-f", "4", unsigned, aligned]);
  mkdirSync(join(root, "dist/android"), { recursive: true });
  cpSync(aligned, apk);
  run([join(buildTools, "apksigner"), "sign", ...key, "--min-sdk-version", String(TOOLCHAIN.api), "--v4-signing-enabled", "false", apk]);
  const badging = run([join(buildTools, "aapt"), "dump", "badging", apk]);
  for (const mark of [`package: name='${id}'`, `sdkVersion:'${TOOLCHAIN.api}'`, "native-code: 'armeabi-v7a'", "uses-gl-es: '0x30000'"])
    if (!badging.includes(mark)) throw new Error(`the package is missing ${mark}`);
  if (badging.includes("application-debuggable") === release) throw new Error(release ? "the release package is debuggable" : "the development package is not debuggable");
  const receipt = { apk: { path: apk.slice(root.length + 1), bytes: statSync(apk).size, sha256: sha(apk) }, release, build: built.build, pack: { bytes: statSync(pack).size, sha256: sha(pack) }, package: id, version };
  writeFileSync(join(root, "dist/android", release ? "receipt-release.json" : "receipt.json"), JSON.stringify(receipt, null, 2) + "\n");
  console.log(JSON.stringify(receipt));
}

// ---------------------------------------------------------------- run

async function launch(waitFor: "running" | "any" = "running") {
  shell(`am force-stop ${id}`);
  inApp("rm -f files/status.json files/control.txt");
  shell(`am start -n ${id}/android.app.NativeActivity`);
  for (let i = 0; i < 240; i++) {
    await Bun.sleep(500);
    const s = status();
    if (s?.shell === "failed") throw new Error(`the app failed: ${s.failure || s.message}`);
    if (s && (waitFor === "any" || (s.shell === "running" && s.frames > 30))) return s;
  }
  throw new Error("the app did not come up");
}

/** A picture of 8-bit RGBA rows as a PNG, turned a quarter when `turn` says so. */
function png(file: string, w: number, h: number, rgba: Uint8Array) {
  const raw = join(out, "capture.rgba");
  writeFileSync(raw, rgba);
  run(["magick", "-size", `${w}x${h}`, "-depth", "8", `rgba:${raw}`, "-alpha", "off", file]);
}

/** A picture the app read back from its window (`write_picture` in android/src/main.c) as a PNG. */
function picture(from: string, file: string) {
  const local = join(out, "picture.rgba");
  take(from, local);
  const bytes = readFileSync(local);
  const [w, h] = [bytes.readUInt32LE(0), bytes.readUInt32LE(4)];
  // OpenGL's rows run from the bottom.
  const rows = new Uint8Array(w * h * 4);
  for (let y = 0; y < h; y++) rows.set(bytes.subarray(8 + (h - 1 - y) * w * 4, 8 + (h - y) * w * 4), y * w * 4);
  png(file, w, h, rows);
  console.log(file);
}

async function capture(file: string) {
  mkdirSync(validation, { recursive: true });
  const s = await send("screen=1");
  for (let i = 0; i < 40 && status()?.captured !== s.command; i++) await Bun.sleep(250);
  picture("files/screen.rgba", file);
}

switch (command) {
  case "doctor":
    if (!doctor()) process.exitCode = 1;
    break;
  case "cook":
    cook();
    break;
  case "build":
    await build();
    break;
  case "apk":
    await packageApk();
    break;
  case "install":
    if (!args.includes("--no-build")) await packageApk();
    await install(apk);
    if (release) {
      // A release keeps its data to itself: it is started, and the system says whether it holds the screen.
      shell(`am start -n ${id}/android.app.NativeActivity`);
      await Bun.sleep(8000);
      if (!new RegExp(`mCurrentFocus=.*${id}/`).test(shell("dumpsys window windows"))) throw new Error("the release did not take the screen");
      console.log(`installed ${id} (release), started`);
      break;
    }
    // A development copy left in the app's data would hide what was just installed.
    inApp("rm -rf files/dev");
    console.log(`installed ${id}`);
    break;
  case "native": {
    const built = await build();
    give(built.engine, "files/dev/libtokyo-engine.so");
    give(join(built.ui, "tokyo.js"), "files/dev/tokyo.js");
    give(join(built.ui, "tokyo.pak"), "files/dev/tokyo.pak");
    if (args.includes("--pack")) give(pack, "files/dev/city.pack");
    console.log(inApp("ls -l files/dev"));
    break;
  }
  case "boot": {
    const local = join(out, "boot.txt");
    mkdirSync(out, { recursive: true });
    writeFileSync(local, (args[1] ?? "") + "\n");
    give(local, "files/dev/boot.txt");
    break;
  }
  case "launch":
    console.log(JSON.stringify(await launch()));
    break;
  case "stop":
    shell(`am force-stop ${id}`);
    break;
  case "status":
    console.log(JSON.stringify(status() ?? { stage: "not running" }, null, 1));
    break;
  case "ctl":
    console.log(JSON.stringify(await send(args[1] ?? "")));
    break;
  case "capture":
    await capture(option("--out", join(validation, "capture.png")));
    break;
  case "screen": {
    const file = option("--out", join(validation, "screen.png"));
    mkdirSync(validation, { recursive: true });
    shell("screencap -p /sdcard/tokyo-screen.png");
    adb("pull", "/sdcard/tokyo-screen.png", file);
    // The panel is 720 × 1280; the app holds it a quarter turn over.
    run(["magick", file, "-rotate", "-90", file]);
    console.log(file);
    break;
  }
  case "title": {
    const file = option("--out", join(validation, "title.png"));
    mkdirSync(validation, { recursive: true });
    shell(`am force-stop ${id}`);
    inApp("rm -f files/status.json files/title.rgba; echo > files/title.want");
    shell(`am start -n ${id}/android.app.NativeActivity`);
    for (let i = 0; i < 40 && !/title.rgba/.test(inApp("ls files")); i++) await Bun.sleep(250);
    picture("files/title.rgba", file);
    break;
  }
  case "bench": {
    const seconds = Number(option("--seconds", "150"));
    mkdirSync(validation, { recursive: true });
    await launch();
    // The flow as a person meets it: from the title into the tour, the instruments over it.
    await send(`ui=tour restart=1 ${option("--ctl")}`);
    await Bun.sleep(3000);
    await send(`mark=${seconds}`);
    const samples: Record<string, any>[] = [];
    const until = Date.now() + seconds * 1000 + 1500;
    while (Date.now() < until) {
      await Bun.sleep(5000);
      const s = status();
      if (s) samples.push(s);
    }
    let s = status()!;
    for (let i = 0; i < 20 && !s.mark?.frames; i++) {
      await Bun.sleep(500);
      s = status()!;
    }
    const of = (pick: (s: Record<string, any>) => number, digits = 0) => {
      const v = samples.map(pick).filter((x) => Number.isFinite(x));
      return { min: Math.min(...v), mean: +(v.reduce((a, b) => a + b, 0) / v.length).toFixed(digits), max: Math.max(...v) };
    };
    const result = {
      build: s.build, seconds, mark: s.mark, latePercent: +((100 * s.mark.late) / Math.max(1, s.mark.frames)).toFixed(2), window: s.window, samples: s.samples, panel: s.panel,
      triangles: of((x) => x.drawn), draws: of((x) => x.draws), gpuTimerMs: of((x) => x.gpuTimerMs, 1), scale: of((x) => x.governor.scale, 2), cpuMs: s.cpuMs, governor: s.governor, hours: [samples[0]?.clock?.hour, s.clock?.hour], thermal: of((x) => x.thermal),
      interface: s.interface, memory: s.memory, residentBytes: s.residentBytes,
    };
    const file = join(validation, `bench-${new Date().toISOString().replace(/[:.]/g, "-")}.json`);
    writeFileSync(file, JSON.stringify({ result, samples }, null, 1));
    console.log(JSON.stringify(result, null, 1));
    console.log(file);
    break;
  }
  case "reset":
    shell(`am force-stop ${id}`);
    inApp(`rm -f files/interface.json files/status.json files/control.txt files/screen.rgba files/title.rgba${args.includes("--dev") ? "; rm -rf files/dev" : ""}`);
    break;
  default:
    throw new Error("usage: bun tools/android.ts <doctor|cook|build|apk|install|native|boot|launch|stop|status|ctl|capture|screen|title|bench|reset>");
}
