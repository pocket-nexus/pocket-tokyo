#!/usr/bin/env bun
/**
 * Pocket Tokyo on the iPod touch 4: cook, build, install, drive and measure.
 * PocketJS supplies the pinned iOS 6 sysroot, the startup objects, the
 * MobileInstallation transaction, the app icon, and the interface's runtime:
 * its UI core (with the OpenGL ES 2 backend), QuickJS and the guest driver.
 * The app is the C in ipod/src, the Rust core in ipod/core (the 3DS core's
 * source) and the interface in ui/.
 *
 *   bun tools/ipod.ts cook [--area shiba]     the city for profiles/ipod60.json → .pocket-build/city/<area>/ipod60/city.pack
 *   bun tools/ipod.ts build | package         the app bundle (and its .ipa) → .pocket-build/ipod/
 *   bun tools/ipod.ts deploy                  build, then install through MobileInstallation
 *   bun tools/ipod.ts native [--pack]         build, then replace the installed executable and interface (and the pack)
 *   bun tools/ipod.ts launch                  start it and wait for its first status
 *   bun tools/ipod.ts status
 *   bun tools/ipod.ts ctl "ui=fly hour=19"    words for the shell, the flow and the flight (see ipod/src/main.c)
 *   bun tools/ipod.ts capture [--out PNG]     the frame as presented, interface and all
 *   bun tools/ipod.ts title [--out PNG]       launches, and brings back the Pocket3D title card's held frame
 *   bun tools/ipod.ts reset                   stops the app and removes what it kept (the settings)
 *   bun tools/ipod.ts bench [--seconds 150] [--ctl "budget=30000"]   the tour's frame timings, the instruments over it → .pocket-build/validation/ipod/
 *
 * The device is the one connected iPod4,1 (or POCKETJS_IPODTOUCH4_UDID), over
 * a USB tunnel to its SSH server.
 */
import { createHash, randomBytes } from "node:crypto";
import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { join, resolve } from "node:path";
import { IPOD_INSTALLER, parseInstalledIPodApp, shellQuote, userDeploymentScript } from "../vendor/pocketjs/tools/ipodtouch4-installation";
import { IPODTOUCH4_TOOLCHAIN, inspectIPodTouch4Toolchain, ipodtouch4CacheRoot, ipodtouch4CsuPath, ipodtouch4QuickJsPath, ipodtouch4SysrootPath } from "../vendor/pocketjs/tools/ipodtouch4-toolchain";
import { POCKET3D_ICON } from "../vendor/pocketjs/tools/pocket3d-icon.ts";
import { compileInterface } from "./ui.ts";

const root = resolve(import.meta.dir, "..");
const args = Bun.argv.slice(2);
const command = args[0] ?? "build";
const option = (key: string, fallback = "") => (args.includes(key) ? (args[args.indexOf(key) + 1] ?? fallback) : fallback);
const area = option("--area", "shiba");
const out = join(root, ".pocket-build/ipod");
const validation = join(root, ".pocket-build/validation/ipod");
const pack = join(root, `.pocket-build/city/${area}/ipod60/city.pack`);
const bundleId = "dev.pocket-nexus.tokyo";
const bundleName = "PocketTokyo.app";
const executableName = "PocketTokyo";
const bundle = join(out, "Payload", bundleName);

function run(cmd: string[], cwd = root, stdin?: string, env?: Record<string, string>): string {
  const p = Bun.spawnSync(cmd, { cwd, stdin: stdin === undefined ? undefined : Buffer.from(stdin), stdout: "pipe", stderr: "pipe", env: env ? { ...process.env, ...env } : undefined });
  if (p.exitCode) throw new Error(`${cmd.slice(0, 2).join(" ")}: ${p.stdout.toString()}${p.stderr.toString()}`);
  return p.stdout.toString().trim();
}
const sha = (file: string) => createHash("sha256").update(readFileSync(file)).digest("hex");
const files = (dir: string): string[] => readdirSync(dir, { withFileTypes: true }).flatMap((e) => (e.isDirectory() ? files(join(dir, e.name)) : join(dir, e.name)));

function cook() {
  if (!existsSync(join(root, `.pocket-build/city/${area}/ir`))) throw new Error("no exported city: run `bun tools/tokyo.ts export` first");
  console.log(run(["cargo", "run", "--release", "-q", "-p", "tokyo-cook", "--", "--in", `.pocket-build/city/${area}/ir`, "--out", `.pocket-build/city/${area}/ipod60`,
    "--profile", "profiles/ipod60.json", "--area", `areas/${area}.json`]));
}

async function build() {
  const toolchain = inspectIPodTouch4Toolchain();
  if (!toolchain.sysroot || !toolchain.csu || !toolchain.quickjs) throw new Error("no iPod touch 4 toolchain: run `bun ipodtouch4 doctor` in vendor/pocketjs");
  if (!existsSync(pack)) throw new Error("the city is not cooked for the iPod: bun tools/ipod.ts cook");
  const ui = await compileInterface("ipod", area);
  const objects = join(out, "native");
  rmSync(bundle, { recursive: true, force: true });
  mkdirSync(objects, { recursive: true });
  mkdirSync(bundle, { recursive: true });
  const clang = run(["xcrun", "--find", "clang"]), ld = run(["xcrun", "--find", "ld-classic"]), sdk = run(["xcrun", "--sdk", "macosx", "--show-sdk-path"]);
  const pocket = join(root, "vendor/pocketjs"), rust = IPODTOUCH4_TOOLCHAIN.compiler.rustToolchain;
  const rustup = (tool: string) => run(["rustup", "which", "--toolchain", rust, tool]);
  const target = join(pocket, "hosts/ipodtouch4/armv7-apple-ios.json");

  const cargo = (directory: string, targetDirectory: string, extra: string[]) =>
    run([rustup("cargo"), "build", "--release", "--target", target, "-Z", "json-target-spec", ...extra], directory, undefined,
      { RUSTC: rustup("rustc"), CARGO_TARGET_DIR: targetDirectory, IPHONEOS_DEPLOYMENT_TARGET: "6.0" });

  // The flight, its flow and what a frame draws, behind core.h, with the title card's frames. It also answers the guest's service wire.
  cargo(join(root, "ipod/core"), join(out, "core"), ["-Z", "build-std=core,alloc,compiler_builtins"]);
  // The interface's runtime, from PocketJS: the UI core as a static library
  // (ES 2 backend; PocketJS's own iPod host builds the ES 1.1 one), QuickJS,
  // and the driver that runs the guest.
  cargo(join(pocket, "engine/ui-cabi"), join(out, "ui-core"), ["--locked", "--no-default-features", "--features", "bare-platform",
    "-Z", "build-std=core,alloc,compiler_builtins", "-Z", "build-std-features=compiler-builtins-mem"]);

  const sources = ["ipod/src/main.c", "ipod/src/render.c", "ipod/src/render.h", "n3ds/src/core.h"];
  const libraries = [join(out, "core", "armv7-apple-ios/release/libtokyo_ipod_core.a"), join(out, "ui-core", "armv7-apple-ios/release/libpocketjs_symbian_core.a")];
  const build = createHash("sha256").update([...sources.map((f) => join(root, f)), ...libraries, join(ui.directory, "tokyo.js"), join(ui.directory, "tokyo.pak"), pack].map(sha).join()).digest("hex").slice(0, 12);
  const compile = (source: string, extra: string[] = []) => {
    const object = join(objects, source.replace(/[^A-Za-z0-9]/g, "_") + ".o");
    // cortex-a8: scalar float goes through NEON (its VFP unit is not pipelined).
    run([clang, "-target", "armv7-apple-ios6.0", "-miphoneos-version-min=6.0", "-mcpu=cortex-a8", "-O3", "-fno-stack-protector", "-fno-common",
      "-U_FORTIFY_SOURCE", "-D_FORTIFY_SOURCE=0", "-isysroot", sdk, "-Wno-incompatible-sysroot", ...extra, "-c", source, "-o", object]);
    return object;
  };
  const csu = ipodtouch4CsuPath();
  const boot = [
    compile(join(csu, "start.s"), ["-x", "assembler-with-cpp"]),
    compile(join(csu, "dyld_glue.s"), ["-x", "assembler-with-cpp", "-DMACH_HEADER_SYMBOL_NAME=__mh_execute_header", "-DCRT"]),
    compile(join(pocket, "hosts/ios-legacy/crt_globals.c")),
  ];
  const link = (output: string, inputs: string[], frameworks: string[]) => {
    run([ld, "-arch", "armv7", "-syslibroot", ipodtouch4SysrootPath(), "-L/usr/lib", "-F/System/Library/Frameworks", "-iphoneos_version_min", "6.0",
      "-no_pie", "-no_uuid", "-no_function_starts", "-no_data_in_code_info", "-no_source_version", "-no_compact_unwind", "-no_adhoc_codesign", "-no_encryption",
      "-e", "start", "-o", output, ...boot, ...inputs, ...frameworks.flatMap((f) => ["-framework", f]), "-lobjc", "-lSystem", "-lgcc_s.1"]);
    run(["chmod", "755", output]);
  };
  const quickjs = join(ipodtouch4QuickJsPath(), "libquickjs-sys/embed/quickjs");
  const includes = ["-I", join(pocket, "engine/quickjs-c"), "-I", join(pocket, "engine/ui-cabi/include"), "-I", join(pocket, "contracts/generated"),
    "-I", join(pocket, "hosts/ios-legacy"), "-I", join(pocket, "hosts/shared"), "-isystem", quickjs];
  const guest = [
    ...["quickjs.c", "cutils.c", "dtoa.c", "libregexp.c", "libunicode.c"].map((f) => compile(join(quickjs, f), ["-I", quickjs, "-funsigned-char", "-fwrapv", `-DCONFIG_VERSION="${IPODTOUCH4_TOOLCHAIN.compiler.quickJsVersion}"`])),
    // The guest's service ops go to the svcwire_* functions, which the core library answers in the process.
    compile(join(pocket, "engine/quickjs-c/pocket_runtime.c"), [...includes, "-DPOCKET_SVC_WIRE", `-DPOCKETJS_TARGET_ID="${ui.inputs.target}"`,
      `-DPOCKETJS_HOST_ABI=${ui.inputs.hostAbi}`, `-DPOCKET_RASTER_DENSITY=${ui.inputs.viewport.rasterDensity}`]),
    compile(join(pocket, "hosts/ios-legacy/compat.c")),
  ];
  const strict = ["-Wall", "-Wextra", "-Werror", `-DTOKYO_BUILD="${build}"`, ...(option("--budget") ? [`-DBUDGET=${option("--budget")}`] : []), ...includes];
  const executable = join(bundle, executableName);
  link(executable, [...["ipod/src/main.c", "ipod/src/render.c"].map((f) => compile(join(root, f), strict)), ...guest, libraries[0], "-force_load", libraries[1]],
    ["UIKit", "Foundation", "QuartzCore", "OpenGLES"]);
  run(["ldid", "-S", executable]);
  link(join(out, "installer"), [compile(join(pocket, "hosts/ipodtouch4/installer.c"))], ["Foundation"]);
  run(["ldid", `-S${pocket}/hosts/ipodtouch4/installer-entitlements.plist`, join(out, "installer")]);

  const plist = (entries: Record<string, string>) => Object.entries(entries).map(([k, v]) => `<key>${k}</key>${v}`).join("");
  const text = (v: string) => `<string>${v}</string>`;
  writeFileSync(join(bundle, "Info.plist"), `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>${plist({
    CFBundleIdentifier: text(bundleId), CFBundleExecutable: text(executableName), CFBundleName: text("Pocket Tokyo"), CFBundleDisplayName: text("Tokyo"),
    CFBundleIconFiles: `<array>${text("Icon.png")}${text("Icon@2x.png")}</array>`, UIPrerenderedIcon: "<true/>", CFBundlePackageType: text("APPL"),
    CFBundleVersion: text("1"), CFBundleShortVersionString: text("0.1.0"), MinimumOSVersion: text("6.0"), UIDeviceFamily: "<array><integer>1</integer></array>",
    // Leaving the app ends it: UIKit would do so anyway (the link stubs carry no UIKit version).
    UIStatusBarHidden: "<true/>", UIApplicationExitsOnSuspend: "<true/>", UIRequiredDeviceCapabilities: `<array>${text("armv7")}${text("opengles-2")}</array>`,
    // `launch` opens the app through its own URL scheme.
    CFBundleURLTypes: `<array><dict><key>CFBundleURLSchemes</key><array>${text(bundleId)}</array></dict></array>`,
  })}</dict></plist>\n`);
  // SpringBoard's icon is the Pocket3D icon from PocketJS, at both sizes, as the files are.
  cpSync(POCKET3D_ICON.ios, join(bundle, "Icon.png"));
  cpSync(POCKET3D_ICON.ios2x, join(bundle, "Icon@2x.png"));
  for (const file of ["tokyo.js", "tokyo.pak"]) cpSync(join(ui.directory, file), join(bundle, file));
  cpSync(pack, join(bundle, "city.pack"));
  rmSync(join(out, "PocketTokyo.ipa"), { force: true });
  // (the pack is texels and vertices: storing it takes the time compressing it would, and installs as fast)
  if (command !== "build" && command !== "native") run(["zip", "-q", "-r", "-1", join(out, "PocketTokyo.ipa"), "Payload"], out);
  console.log(JSON.stringify({ bundle, build, bundleId, executable: sha(executable), pack: sha(pack) }));
}

type Device = { ssh: (script: string, stdin?: string) => string; pull: (from: string, to: string) => void; push: (from: string, to: string) => void; tmp: () => string; path: () => string };
/** The one connected, provisioned iPod4,1 (iOS 6.1.6) over a USB tunnel to its SSH server. */
async function device<T>(operation: (d: Device) => Promise<T> | T): Promise<T> {
  const ids = run(["idevice_id", "-l"]).split("\n").filter(Boolean);
  const udid = process.env.POCKETJS_IPODTOUCH4_UDID ?? (ids.length === 1 ? ids[0] : "");
  if (!/^[0-9a-f]{40}$/.test(udid)) throw new Error("connect one iPod touch 4 or set POCKETJS_IPODTOUCH4_UDID");
  for (const [key, value] of [["ProductType", "iPod4,1"], ["BuildVersion", "10B500"]])
    if (run(["ideviceinfo", "-u", udid, "-k", key]) !== value) throw new Error(`unexpected device ${key}`);
  const keys = [join(ipodtouch4CacheRoot(), "ssh", udid), join(ipodtouch4CacheRoot(), "ssh")].find((d) => existsSync(join(d, "id_rsa")))!;
  const port = await new Promise<number>((done, fail) => {
    const server = createServer().once("error", fail).listen(0, "127.0.0.1", () => {
      const p = (server.address() as { port: number }).port;
      server.close(() => done(p));
    });
  });
  const tunnel = Bun.spawn(["iproxy", "-u", udid, `${port}:22`], { stdout: "ignore", stderr: "ignore" });
  const auth = ["-i", process.env.POCKETJS_IPODTOUCH4_KEY ?? join(keys, "id_rsa"), "-o", "BatchMode=yes", "-o", "ConnectTimeout=5", "-o", "HostKeyAlias=[127.0.0.1]:2224",
    "-o", "StrictHostKeyChecking=yes", "-o", `UserKnownHostsFile=${process.env.POCKETJS_IPODTOUCH4_KNOWN_HOSTS ?? join(keys, "known_hosts")}`,
    "-o", "HostKeyAlgorithms=+ssh-rsa", "-o", "PubkeyAcceptedAlgorithms=+ssh-rsa"];
  const ssh = (script: string, stdin?: string) => run(["ssh", "-p", String(port), ...auth, "root@127.0.0.1", script], root, stdin);
  const copy = (from: string, to: string) => void run(["scp", "-O", "-P", String(port), ...auth, from, to]);
  let app: ReturnType<typeof parseInstalledIPodApp> | undefined;
  const installed = () => (app ??= parseInstalledIPodApp(ssh(`${IPOD_INSTALLER} lookup ${shellQuote(bundleId)}`), bundleId, bundleName));
  try {
    for (let attempt = 0; ; attempt++) {
      await Bun.sleep(200);
      try { ssh("true"); break; } catch (error) { if (attempt === 20) throw error; }
    }
    return await operation({ ssh, pull: (from, to) => copy(`root@127.0.0.1:${from}`, to), push: (from, to) => copy(from, `root@127.0.0.1:${to}`),
      tmp: () => installed().Container + "/tmp", path: () => installed().Path });
  } finally {
    tunnel.kill();
    await tunnel.exited;
  }
}
const status = (d: Device) => JSON.parse(d.ssh(`cat ${shellQuote(d.tmp() + "/status.json")}`));

/**
 * The screen is shared with other work on this device. A tool that drives
 * another app leaves control files and captures in that app's container:
 * when one was written in the last three minutes, this waits (up to five
 * more) before taking the screen or loading the one core. An app that was
 * left running only rewrites its status, and gives way. The device has no
 * `ps`; `find` answers the question.
 */
async function waitForScreen(d: Device) {
  // The installer reports the container under /private; `find` lists it without.
  const own = d.tmp().replace(/^\/private/, "");
  for (let attempt = 0; attempt < 15; attempt++) {
    const seen = d.ssh(`find /var/mobile/Applications/*/tmp -maxdepth 1 -type f -mmin -3 ! -name 'status.json*'; true`)
      .split("\n").filter((line) => line && !line.startsWith(own));
    if (!seen.length) return;
    console.error(`ipod: another tool is driving the device (${seen[0]}); waiting 20 s (${attempt + 1}/15)`);
    await Bun.sleep(20000);
  }
  throw new Error("another tool kept the device for five minutes: try again later");
}

/** Sends words and waits until a presented frame acknowledges them. */
async function control(d: Device, words: string) {
  const nonce = randomBytes(8).toString("hex"), file = shellQuote(d.tmp() + "/control.txt");
  d.ssh(`cat > ${file}.new && mv ${file}.new ${file}`, `${nonce}\n${words}\n`);
  const screen = /\bscreen=1\b/.test(words);
  for (let attempt = 0; attempt < 120; attempt++) {
    await Bun.sleep(300);
    const s = status(d);
    if (s.lastCommand === nonce && (!screen || s.capture === nonce)) return s;
  }
  throw new Error("the device did not acknowledge the command");
}
/** A frame the shell wrote as the portrait drawable's rows, bottom row first: turned to the screen as it is held. */
function picture(d: Device, name: string, output: string) {
  const raw = join(out, name);
  d.pull(`${d.tmp()}/${name}`, raw);
  mkdirSync(resolve(output, ".."), { recursive: true });
  run(["magick", "-size", "320x480", "-depth", "8", `rgba:${raw}`, "-alpha", "off", "-flip", "-rotate", "-90", output]);
  console.log(output);
}
async function launch(d: Device) {
  await waitForScreen(d);
  d.ssh(`killall ${executableName} 2>/dev/null; rm -f ${shellQuote(d.tmp() + "/status.json")}; su mobile -c ${shellQuote(`uiopen ${bundleId}://launch`)}`);
  for (let attempt = 0; ; attempt++) {
    await Bun.sleep(500);
    try {
      const s = status(d);
      if (s.stage !== "loading" || attempt === 60) return s;
    } catch (error) {
      if (attempt === 60) throw error;
    }
  }
}

if (command === "cook") cook();
else if (command === "build" || command === "package") await build();
else if (command === "deploy") {
  await build();
  await device((d) => {
    const remote = `/private/var/tmp/tokyo-${randomBytes(8).toString("hex")}`, ipa = join(out, "PocketTokyo.ipa");
    d.ssh(`mkdir -p ${remote} /var/root/Library/PocketJS`);
    d.push(join(out, "installer"), `${remote}/installer`);
    d.push(ipa, `${remote}/app.ipa`);
    writeFileSync(join(out, "deploy.sh"), userDeploymentScript({ bundleId, bundleName, executable: executableName, archive: `${remote}/app.ipa`, archiveHash: sha(ipa),
      files: Object.fromEntries(files(bundle).map((f) => [f.slice(bundle.length + 1), sha(f)])) }));
    d.push(join(out, "deploy.sh"), `${remote}/deploy.sh`);
    // Installs, then reads every installed file back against its hash.
    console.log(d.ssh(`chmod 700 ${remote}/installer; mv ${remote}/installer ${IPOD_INSTALLER}; ${IPOD_INSTALLER} lock ${shellQuote(bundleId)} ${remote}/deploy.sh; rm -rf ${remote}`));
  });
} else if (command === "native") {
  // Replaces the installed executable and interface, and with --pack the city.
  await build();
  await device((d) => {
    d.ssh(`killall ${executableName} 2>/dev/null; true`);
    for (const file of ["tokyo.js", "tokyo.pak", ...(args.includes("--pack") ? ["city.pack"] : [])]) d.push(join(bundle, file), `${d.path()}/${file}`);
    d.push(join(bundle, executableName), `${d.path()}/${executableName}.new`);
    const digest = d.ssh(`cd ${shellQuote(d.path())} && chmod 755 ${executableName}.new && mv ${executableName}.new ${executableName} && openssl dgst -sha256 ${executableName}`);
    if (!digest.endsWith(sha(join(bundle, executableName)))) throw new Error("the installed executable differs");
  });
} else if (command === "launch") await device(async (d) => console.log(JSON.stringify(await launch(d))));
else if (command === "status") await device((d) => console.log(JSON.stringify(status(d))));
else if (command === "ctl") await device(async (d) => console.log(JSON.stringify(await control(d, args[1] ?? ""))));
else if (command === "capture") await device(async (d) => {
  await control(d, `${option("--ctl")} screen=1`.trim());
  picture(d, "screen.rgba", resolve(option("--out", join(validation, "capture.png"))));
});
else if (command === "title") await device(async (d) => {
  // The card's held frame, as the device presents it at the next launch.
  d.ssh(`touch ${shellQuote(d.tmp() + "/title.want")}; rm -f ${shellQuote(d.tmp() + "/title.rgba")}`);
  await launch(d);
  picture(d, "title.rgba", resolve(option("--out", join(validation, "title.png"))));
});
else if (command === "reset") await device((d) => {
  // As after a first install: the app stopped, nothing kept from earlier runs (the settings).
  const home = d.tmp().replace(/\/tmp$/, "");
  d.ssh(`killall ${executableName} 2>/dev/null; rm -f ${shellQuote(home + "/Documents/interface.json")} ${shellQuote(home + "/tmp")}/*; true`);
  console.log(d.ssh(`ls -la ${shellQuote(home + "/Documents")} ${shellQuote(home + "/tmp")}`));
});
else if (command === "bench") {
  // The tour from its start, as a flight: the instruments stand over it. The device measures the window itself
  // (`mark=`): a status read over SSH costs its one core a few frames, so
  // nothing is asked until it is done.
  const seconds = Number(option("--seconds", "150"));
  await device(async (d) => {
    await waitForScreen(d);
    const first = await control(d, `mode=flight tour=1 restart=1 view=off ${option("--ctl")} mark=${seconds}`.replace(/ +/g, " "));
    if (first.stage !== "running") throw new Error(`the device is not flying (stage ${first.stage})`);
    await Bun.sleep((seconds + 4) * 1000);
    const s = status(d);
    if (s.build !== first.build) throw new Error("the build changed during the window");
    const w = s.window;
    if (!w.done) throw new Error("the window did not finish: is the app on the screen?");
    const summary = { seconds: w.seconds, frames: w.frames, lateFrames: w.late, lateShare: w.late / Math.max(w.frames, 1), averageFrameMs: (w.seconds * 1000) / w.frames, fps: w.frames / w.seconds,
      worstFrameMs: w.worstMs, triangles: { min: w.minTris, mean: w.meanTris, max: w.maxTris }, draws: { mean: w.meanDraws, max: w.maxDraws }, meanMs: w.meanMs,
      guest: { running: s.guest.running, turnsPerSecond: w.turns / w.seconds, redrawsPerSecond: w.redraws / w.seconds, turnMs: w.turnMs, redrawMs: w.redrawMs },
      residentBytes: s.residentBytes, governor: s.governor, reach: s.reach, control: option("--ctl") };
    const directory = join(validation, `bench-${new Date().toISOString().replace(/[:.]/g, "-")}`);
    mkdirSync(directory, { recursive: true });
    writeFileSync(join(directory, "device.json"), JSON.stringify({ summary, identity: { device: "ipod:iPod4,1", build: s.build }, status: s }, null, 1));
    console.log(join(directory, "device.json"));
    console.log(JSON.stringify(summary, null, 1));
  });
} else throw new Error("usage: cook | build | package | deploy | native [--pack] | launch | status | ctl WORDS | capture [--out PNG] [--ctl WORDS] | title [--out PNG] | reset | bench [--seconds N] [--ctl WORDS]");
