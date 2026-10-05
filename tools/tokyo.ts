#!/usr/bin/env bun
// Pocket Tokyo's command line.
//
//   bun tools/tokyo.ts fetch [--area shiba]        public records → web/data/raw (web/tools/pipeline)
//   bun tools/tokyo.ts tiles [--area shiba]        records → web/public/tiles/<area>
//   bun tools/tokyo.ts export [--area shiba]       the city as the reference draws it → .pocket-build/city/<area>/ir
//   bun tools/tokyo.ts cook [--area shiba] [--profile vita60]   CityIR → a device pack and its receipt
//   bun tools/tokyo.ts build|sync|native|push|serve|status|capture|ctl|bench|hold|vpk   the Vita loop (tools/vita.ts)

import { $ } from "bun";
import { join, resolve } from "node:path";

export const ROOT = resolve(import.meta.dir, "..");
export const BUILD = join(ROOT, ".pocket-build");

function value(argv: string[], flag: string, fallback: string): string {
  const i = argv.indexOf(flag);
  return i >= 0 && argv[i + 1] ? argv[i + 1]! : fallback;
}

const [cmd, ...rest] = process.argv.slice(2);
const area = value(rest, "--area", "shiba");
switch (cmd) {
  case "fetch":
    // Node reads the proxy settings of the environment only when asked.
    await $`node tools/pipeline/fetch.mjs --area=${area}`.cwd(join(ROOT, "web")).env({ ...process.env, NODE_USE_ENV_PROXY: "1" });
    break;
  case "tiles":
    await $`node --max-old-space-size=8192 tools/pipeline/compile.mjs --area=${area}`.cwd(join(ROOT, "web"));
    break;
  case "export": {
    const { exportCity } = await import("./export.ts");
    await exportCity(rest);
    break;
  }
  case "cook": {
    const profile = value(rest, "--profile", "vita60");
    const extra = rest.filter((a, i) => !["--area", "--profile"].includes(a) && !["--area", "--profile"].includes(rest[i - 1] ?? ""));
    await $`cargo run --release -q -p tokyo-cook -- --in .pocket-build/city/${area}/ir --out .pocket-build/city/${area}/${profile} --profile profiles/${profile}.json --area areas/${area}.json ${extra}`.cwd(ROOT);
    break;
  }
  case "build":
  case "sync":
  case "native":
  case "push":
  case "serve":
  case "status":
  case "capture":
  case "ctl":
  case "hold":
  case "vpk":
  case "push-vpk":
  case "bench": {
    const vita = await import("./vita.ts");
    if (cmd === "build") await vita.build(rest);
    else if (cmd === "sync") await vita.sync(rest);
    else if (cmd === "native") {
      await vita.sync(rest);
      await vita.build(rest);
      await vita.dev(rest, "native");
    } else if (cmd === "push") {
      // The build already in dist/, without rebuilding it.
      await vita.sync(rest);
      await vita.dev(rest, "native");
    } else if (cmd === "serve") await vita.dev(rest, "serve");
    else if (cmd === "status") console.log(JSON.stringify(vita.status(rest), null, 1));
    else if (cmd === "capture") await vita.dev(rest, "capture", ...(rest.includes("--out") ? ["--out", resolve(rest[rest.indexOf("--out") + 1])] : []));
    else if (cmd === "ctl") vita.ctl(rest, rest.find((a) => a.startsWith("{")) ?? "{}");
    else if (cmd === "hold") await vita.hold(rest);
    else if (cmd === "vpk") await vita.vpk(rest);
    else if (cmd === "push-vpk") await vita.pushVpk(rest);
    else {
      const { bench } = await import("./bench.ts");
      await bench(rest);
    }
    break;
  }
  default:
    console.log("usage: bun tools/tokyo.ts <fetch|tiles|export|cook|build|sync|native|push|serve|status|capture|ctl|bench|hold|vpk>");
    process.exit(cmd ? 1 : 0);
}
