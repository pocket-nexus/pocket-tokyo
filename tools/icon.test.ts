// The app icon on every console is Pocket3D's, read from the PocketJS pin
// (vendor/pocketjs/engine/pocket3d/icon/). This repository tracks no icon file,
// and each console's build names the PocketJS one.
//
//   bun test ./tools/icon.test.ts

import { expect, test } from "bun:test";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { basename, join, resolve } from "node:path";

const ROOT = resolve(import.meta.dir, "..");
// Loaded by path at run time, as tools/vita.ts does: the module's rasterizer types come with PocketJS's own dependencies.
// A failure here means the submodule is not checked out: `bun run setup`.
const iconModule: string = join(ROOT, "vendor/pocketjs/tools/pocket3d-icon.ts");
const { POCKET3D_ICON } = (await import(iconModule)) as { POCKET3D_ICON: Record<"psp" | "vita" | "n3ds" | "n3dsSmall", string> };
const ICONS = join(ROOT, "vendor/pocketjs/engine/pocket3d/icon");
const read = (path: string) => readFileSync(join(ROOT, path), "utf8");

/** What `NAME := value` assigns in a Makefile, with `$(OTHER)` replaced from `values`. */
function makeValue(makefile: string, name: string, values: Record<string, string>): string {
  const line = makefile.split("\n").find((l) => l.startsWith(`${name} := `));
  if (!line) throw new Error(`no ${name} in the Makefile`);
  return line.slice(name.length + 4).replace(/\$\((\w+)\)/g, (_, key: string) => values[key] ?? `$(${key})`);
}

test("the PocketJS pin holds the icon of each console", () => {
  for (const file of [POCKET3D_ICON.psp, POCKET3D_ICON.vita, POCKET3D_ICON.n3ds, POCKET3D_ICON.n3dsSmall]) {
    expect(existsSync(file), file).toBe(true);
    expect(resolve(file).startsWith(ICONS), file).toBe(true);
  }
});

test("no icon file is tracked outside vendor/", () => {
  const tracked = Bun.spawnSync(["git", "ls-files", "-z"], { cwd: ROOT });
  expect(tracked.exitCode).toBe(0);
  const icons = tracked.stdout
    .toString()
    .split("\0")
    .filter((path) => path && !path.startsWith("vendor/") && /^icon[^/]*\.png$/i.test(basename(path)));
  expect(icons).toEqual([]);
});

test("PSP: Psp.toml names Pocket3D's ICON0.PNG", () => {
  const icon = read("psp/Psp.toml").match(/^xmb_icon_png\s*=\s*"([^"]+)"/m)?.[1];
  // cargo-psp reads the path from the crate's directory.
  expect(resolve(ROOT, "psp", icon ?? "")).toBe(resolve(POCKET3D_ICON.psp));
});

test("PS Vita: every VPK is packaged with POCKET3D_ICON.vita", () => {
  let calls = 0;
  for (const name of readdirSync(join(ROOT, "tools")).filter((f) => f.endsWith(".ts") && !f.endsWith(".test.ts"))) {
    const source = read(`tools/${name}`);
    for (const call of source.matchAll(/packageVitaVpk\(\{[^\n]*/g)) {
      calls++;
      expect(call[0], `tools/${name}`).toContain("icon: POCKET3D_ICON.vita");
      expect(source, `tools/${name}`).toContain("tools/pocket3d-icon.ts");
    }
  }
  expect(calls).toBeGreaterThan(0);
});

test("Nintendo 3DS: the SMDH takes both of Pocket3D's sizes, and the snapshot carries them", () => {
  const makefile = read("n3ds/Makefile");
  const values: Record<string, string> = { ROOT };
  for (const name of ["SOURCE", "ICONS", "ICON", "SMALL_ICON"]) values[name] = makeValue(makefile, name, values);
  expect(resolve(values.ICON!)).toBe(resolve(POCKET3D_ICON.n3ds));
  expect(resolve(values.SMALL_ICON!)).toBe(resolve(POCKET3D_ICON.n3dsSmall));
  // smdhtool --create <title> <description> <author> <large icon> <output> <small icon>
  expect(makefile).toMatch(/smdhtool --create [^\n]* \$\(ICON\) \$@ \$\(SMALL_ICON\)\n/);

  // tools/n3ds.ts compiles a snapshot: both files are in it, under the path the Makefile reads.
  const tool = read("tools/n3ds.ts");
  const directory = tool.match(/^const ICONS = "([^"]+)";$/m)?.[1];
  expect(resolve(ROOT, directory ?? "")).toBe(resolve(ICONS, "3ds"));
  const tar = tool.split("\n").find((line) => line.includes("tar --no-xattrs -cf")) ?? "";
  expect(tar).toContain("${ICONS}/icon.png");
  expect(tar).toContain("${ICONS}/icon-small.png");
});
