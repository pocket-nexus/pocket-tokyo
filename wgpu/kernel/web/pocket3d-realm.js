// One realm for a game's interface: a hidden frame of the page loads this
// module (pocket3d-realm.html), so the PocketJS guest that draws the interface
// has globals of its own and its own instance of PocketJS's UI core
// (pocketjs.wasm). Removing the frame ends the guest; another presentation of
// the interface is another realm.
//
// The guest is the bundle the game's build wrote for a device, as it is. It
// meets the same host a device gives it: `globalThis.ui` (PocketJS's HostOps,
// from wasm-ops.js as PocketJS ships it), its pak, and a service it opens by
// name, whose lines the page carries to the game and back. The page calls
// `turn` for the guest's turns and reads what it drew.
//
// PocketJS's own realm for a page (hosts/web/app-instance.js) starts a text
// worker and its wasm with every guest; an interface with baked glyphs has no
// use for them, so this realm is its own.
import { createWasmUi } from "./wasm-ops.js";
import { __packTouch, createTouchHitFacts, PROP } from "./pocketjs-host.js";

async function read(url, what) {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${what}: ${url} answered ${response.status}`);
  return response;
}

/**
 * Starts a guest.
 *
 *   wasm, bundle, pak   URLs of pocketjs.wasm and of the guest's script and pak
 *   viewport            the primary surface in logical pixels, [width, height]
 *   density             raster samples per logical pixel along an axis
 *   auxiliary           a second surface in logical pixels, or undefined
 *   touch               the surface contacts are on: "primary", "auxiliary" or "none"
 *   service             the name of the service the guest opens ("pocket.overlay")
 */
export async function create({ wasm: wasmUrl, bundle, pak: pakUrl, viewport, density = 1, auxiliary, touch = "none", service, id = "interface" }) {
  const [wasmBytes, source, pak] = await Promise.all([
    read(wasmUrl, "PocketJS's UI core").then((r) => r.arrayBuffer()),
    read(bundle, "the interface's script").then((r) => r.text()),
    read(pakUrl, "the interface's pak").then((r) => r.arrayBuffer()),
  ]);
  const wasm = await createWasmUi(wasmBytes, { width: viewport[0], height: viewport[1], rasterDensity: density, auxiliary });
  const ops = wasm.ops;
  const incoming = [];
  const outgoing = [];
  let opened = false;
  ops.svcOpen = (name) => (opened = name === service);
  ops.svcPoll = () => (incoming.length ? `${incoming.splice(0).join("\n")}\n` : null);
  ops.svcSend = (line) => {
    if (typeof line === "string" && outgoing.length < 1024) outgoing.push(line);
  };
  globalThis.ui = ops;
  globalThis.frame = undefined;
  globalThis.__simHz = 60;
  globalThis.__pocketApp = id;
  globalThis.__pak = pak;
  new Function(`${source}\n//# sourceURL=${id}.js`)();
  if (typeof globalThis.frame !== "function") throw new Error("the interface's script installed no frame()");

  // A contact's node is found once, where it comes down (PocketJS's touch hit facts).
  const facts = createTouchHitFacts((x, y) => (touch === "auxiliary" ? ops.hitTestBoundsAuxiliary(x, y) : ops.hitTestBounds(x, y)));
  const surface = touch === "auxiliary" ? 1 : 0;
  const [width, height] = [viewport[0] * density, viewport[1] * density];

  return {
    /** Whether the guest opened the service. */
    opened: () => opened,
    /**
     * One turn of the guest: `buttons` are PocketJS's bits; `contacts` are
     * `{ id, x, y }` on the touch surface, in its logical pixels. The UI core
     * then advances `ticks` sixtieths of a second.
     */
    turn(buttons, contacts, ticks = 1) {
      const packed = contacts.length ? contacts.map((c) => __packTouch(c.id, Math.round(c.x), Math.round(c.y))) : undefined;
      globalThis.frame(buttons, 0x8080, packed, facts(packed), packed?.map(() => surface));
      for (let i = 0; i < ticks; i++) wasm.tick();
    },
    /** A line for the guest's next turn. */
    send(line) {
      incoming.push(line.endsWith("\n") ? line.slice(0, -1) : line);
    },
    /** The lines the guest sent since the last call. */
    drain: () => outgoing.splice(0),
    /** What the primary surface shows, as a number that changes when it does. */
    hash: () => (wasm.drawHash ? wasm.drawHash() : -1n),
    /**
     * The primary surface drawn twice, over black and over white: the UI
     * core's rasterizer writes opaque pixels, and what the interface covers
     * is in the difference of the two. RGBA rows, `width` by `height`.
     * `black` is a copy; `white` is the core's own buffer, good until the
     * guest's next turn. `drawMs` is what the two drawings took.
     */
    pictures() {
      const from = performance.now();
      ops.setProp(1, PROP.bgColor, 0xff000000);
      const black = wasm.renderScaled(density).slice();
      ops.setProp(1, PROP.bgColor, 0xffffffff);
      const white = wasm.renderScaled(density);
      ops.setProp(1, PROP.bgColor, 0);
      return { black, white, width, height, drawMs: performance.now() - from };
    },
    /** The second surface as opaque RGBA rows (the core's own buffer), one sample a logical pixel. */
    lower: () => wasm.renderAuxiliary(),
  };
}

globalThis.Pocket3DRealm = { create };
