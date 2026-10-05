// Drives one device's bundle through a flight against the mock renderer and
// checks what it asked for.
//   bun ui/test/flow.ts <psp|vita|3ds|ipod|android>     (after `bun tools/ui.ts <device>`)
import assert from "node:assert/strict";
import { BTN } from "../../vendor/pocketjs/contracts/spec/spec.ts";
import { FLY, type Command } from "../app/protocol.ts";
import { anchored, boot, type Device } from "./harness.ts";

const device = (process.argv[2] ?? "psp") as Device;
const rig = await boot(device);
const mock = rig.mock;
const touch = device === "ipod" || device === "android", dual = device === "3ds";
/** A touch panel's points are written as they lie on the iPod touch, with the edges their control hangs from. */
const at = anchored(rig.view);
const tap = (...point: Parameters<typeof at>) => rig.tap(...at(...point));
/** The commands since the last call, without the touch panel's streams. */
let read = 0;
const asked = (): Command[] => {
  const all = mock.log.slice(read).filter((command) => !["drive", "look", "idle", "wake"].includes(command.type));
  read = mock.log.length;
  return all;
};
/** Chooses row `index` of the list that is up with the d-pad. */
const choose = (index: number) => {
  for (let i = 0; i < index; i++) rig.press(BTN.DOWN);
  rig.press(BTN.CIRCLE);
  rig.step(4);
};

// What was stored comes back: the settings are told again.
mock.state.prefs = JSON.stringify({ options: { invert: 1 } });
rig.step(10);
assert.deepEqual(asked(), [{ type: "option", key: "invert", value: 1 }]);
assert.equal(mock.state.options[1].value, 1);

// The hours, from the title: the clock is turned to dusk.
if (touch) {
  tap(100, 200);
  rig.step(6);
  tap(145, 206, "c", "c");
  rig.step(4);
  tap(100, 30, "c", "c");
} else {
  choose(2);
  choose(4);
  rig.press(BTN.CROSS);
}
rig.step(6);
assert.deepEqual(asked(), [{ type: "hour", minutes: 1090 }]);
assert.equal(mock.state.t[0], 1090);

// The title's first choice starts the tour.
if (touch) tap(100, 128);
else choose(0);
rig.step(4);
assert.deepEqual(asked(), [{ type: "start", tour: true }]);
assert.equal(mock.state.mode, "flight");
// The hint fades in, stands with nothing scheduled (the renderer counts its seconds), and fades out.
assert.equal(mock.idle, false);
rig.step(60);
assert.equal(mock.idle, true);
assert.equal(mock.state.woke, 0);
rig.step(360);
assert.notEqual(mock.state.woke, 0);
assert.equal(mock.idle, false);
rig.step(60);
assert.equal(mock.idle, true);
// The eye changing hands is said for a moment.
mock.state.tour = false;
rig.step(2);
assert.equal(mock.idle, false);
rig.step(60);
assert.equal(mock.idle, true);
rig.step(120);
assert.equal(mock.idle, true);

if (touch) {
  // A thumb on the stick and one on the climb key: the stick forward, the key held.
  const [stick, up] = [at(86, 234, "l", "b"), at(424, 170, "r", "b")];
  for (let i = 0; i < 8; i++) rig.step(1, { touch: [{ id: 1, x: stick[0], y: stick[1] - i * 6 }, { id: 2, x: up[0], y: up[1] }] });
  assert.equal(mock.drive.b, FLY.up);
  assert.ok(mock.drive.my > 60 && Math.abs(mock.drive.mx) < 10, `stick ${mock.drive.mx},${mock.drive.my}`);
  rig.step(2);
  assert.deepEqual(mock.drive, { mx: 0, my: 0, b: 0 });
  // A finger on the city turns the view by what it travels.
  for (let i = 0; i < 6; i++) rig.step(1, { touch: [{ id: 3, x: at(200, 150, "c", "c")[0] + i * 10, y: at(200, 150, "c", "c")[1] }] });
  rig.step(2);
  assert.ok(mock.looked.dx >= 40, `looked ${mock.looked.dx}`);
  assert.deepEqual(asked(), []);
  // The tour's key hands the eye over.
  tap(332, 34, "r");
  rig.step(2);
  assert.deepEqual(asked(), [{ type: "tour", on: true }]);
  // The clock opens the day as a bar: a finger at its middle turns the clock to noon.
  tap(426, 34, "r");
  rig.step(4);
  rig.step(3, { touch: [{ id: 4, x: at(240, 86, "c")[0], y: 86 }] });
  rig.step(2);
  assert.deepEqual(asked(), [{ type: "hour", minutes: 720 }]);
  tap(426, 34, "r");
  rig.step(2);
}
if (dual) {
  // The lower screen: the stylus turns the clock to noon, and the tour's key hands the eye over.
  rig.step(3, { touch: [{ id: 4, x: 160, y: 222 }] });
  rig.step(2);
  assert.deepEqual(asked(), [{ type: "hour", minutes: 720 }]);
  rig.tap(288, 70);
  rig.step(2);
  assert.deepEqual(asked(), [{ type: "tour", on: true }]);
}
const touring = touch || dual;

// The menu: a setting, and the way back to the flight.
if (touch) tap(32, 32);
else rig.press(BTN.START);
rig.step(8);
assert.deepEqual(asked(), [{ type: "menu", on: true }]);
assert.equal(mock.state.mode, "menu");
if (touch) {
  tap(340, 162, "c", "c");
  rig.step(6);
  tap(340, 118, "c", "c");
} else {
  choose(3);
  choose(1);
}
rig.step(4);
const kept = { options: { invert: 1, stats: 1 } };
assert.deepEqual(asked(), [{ type: "option", key: "stats", value: 1 }, { type: "prefs", value: JSON.stringify(kept) }]);
assert.ok(mock.state.stats.length > 0);
if (touch) {
  tap(256, 30, "c", "c");
  rig.step(6);
  // The second row hands the eye to the tour, or takes it, and leaves the menu.
  tap(340, 74, "c", "c");
} else {
  rig.press(BTN.CROSS);
  rig.step(6);
  choose(1);
}
rig.step(6);
assert.deepEqual(asked(), [{ type: "tour", on: !touring }, { type: "menu", on: false }]);
assert.equal(mock.state.mode, "flight");

// START closes the menu it opened, and the title is one row away.
if (touch) {
  tap(32, 32);
  rig.step(8);
  tap(340, 250, "c", "c");
} else {
  rig.press(BTN.START);
  rig.step(8);
  rig.press(BTN.START);
  rig.step(8);
  assert.deepEqual(asked(), [{ type: "menu", on: true }, { type: "menu", on: false }]);
  rig.press(BTN.START);
  rig.step(8);
  choose(5);
}
rig.step(6);
assert.deepEqual(asked(), [{ type: "menu", on: true }, { type: "title" }]);
assert.equal(mock.state.mode, "title");
console.log(`${device}: the interface asked for what the flow expects`);
