// Writes pictures of the interface on one device to .pocket-build/ui/preview/.
//   bun ui/test/preview.ts <psp|vita|3ds|ipod|android>     (after `bun tools/ui.ts <device>`)
import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { BTN } from "../../vendor/pocketjs/contracts/spec/spec.ts";
import { anchored, boot, type Device } from "./harness.ts";

const device = (process.argv[2] ?? "psp") as Device;
const build = resolve(import.meta.dir, "../../.pocket-build/ui");
const out = join(build, "preview");
mkdirSync(out, { recursive: true });
const rig = await boot(device);
const scene = join(build, "backdrops/scene.png");
const names: string[] = [];
const save = async (name: string) => {
  const file = join(out, `${device}-${name}.png`);
  writeFileSync(file, (await rig.shot(scene)).toBuffer("image/png"));
  names.push(file);
};
const touch = device === "ipod" || device === "android", dual = device === "3ds";
/** A touch panel's points are written as they lie on the iPod touch, with the edges their control hangs from. */
const at = anchored(rig.view);
const tap = (...point: Parameters<typeof at>) => rig.tap(...at(...point));
/** A point of the 3DS's lower screen, as the rig takes touches. */
const press = (index: number) => {
  for (let i = 0; i < index; i++) rig.press(BTN.DOWN);
  rig.press(BTN.CIRCLE);
};
/** One list up: the heading's left end on a touch panel (`x`: where its sheet begins), the back button elsewhere. */
const back = (x = 70) => (touch ? tap(x + 30, 34, "c", "c") : rig.press(BTN.CROSS));

rig.step(20);
await save("title");
// The hours, from the title.
if (touch) tap(80, 280, "l", "b");
else press(2);
rig.step(12);
await save("title-time");
if (touch) tap(240, 174, "c", "c");
else press(4);
rig.mock.fly(1090, 420, 216, 318, 900, 1000);
rig.step(8);
back();
rig.step(6);
if (touch) tap(260, 280, "l", "b");
else press(4);
rig.step(10);
await save("about");
back();
rig.step(6);
// The tour.
if (touch) tap(130, 146);
else press(0);
rig.step(20);
await save("tour-start");
rig.mock.fly(1096, 268, 214, 232, 310, 260);
rig.step(460);
await save("tour");
// A stick takes the eye.
rig.mock.state.tour = false;
rig.mock.fly(1099, 190, 96, 262, -120, 420);
rig.step(12);
await save("flight");
if (touch) {
  // Both thumbs down: fly forward, climb.
  const [stick, up] = [at(86, 234, "l", "b"), at(424, 170, "r", "b")];
  for (let i = 0; i < 12; i++) rig.step(1, { touch: [{ id: 1, x: stick[0], y: stick[1] - i * 3 }, { id: 2, x: up[0], y: up[1] }] });
  await save("flight-thumbs");
  rig.step(4);
  // The clock opens the day as a bar.
  tap(426, 34, "r");
  rig.step(8);
  for (let i = 0; i < 8; i++) rig.step(1, { touch: [{ id: 3, x: at(200, 86, "c")[0] + i * 14, y: 86 }] });
  rig.step(4);
  await save("flight-clock");
  tap(426, 34, "r");
  rig.step(4);
  tap(32, 32);
} else if (dual) {
  // The stylus turns the clock on the lower screen.
  for (let i = 0; i < 8; i++) rig.step(1, { touch: [{ id: 3, x: 60 + i * 24, y: 222 }] });
  rig.step(4);
  await save("flight-clock");
  rig.press(BTN.START);
} else rig.press(BTN.START);
rig.step(14);
await save("menu");
if (touch) tap(340, 166, "c", "c");
else press(3);
rig.step(10);
await save("settings");
back(226);
rig.step(6);
if (touch) tap(340, 210, "c", "c");
else press(4);
rig.step(10);
await save("controls");

const sheet = join(out, `sheet-${device}.png`);
Bun.spawnSync(["magick", "montage", ...names, "-tile", "4x", "-geometry", `${rig.view.density === 2 ? "50%x50%+4+4" : "+4+4"}`, "-background", "#333333", sheet]);
console.log(sheet);
console.log(JSON.stringify(rig.mock.log.filter((command) => !["drive", "look", "idle", "wake"].includes(command.type))));
