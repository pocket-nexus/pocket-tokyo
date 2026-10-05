// A game's interface on the page: the PocketJS guest that draws it, in a realm
// of its own (pocket3d-realm.js), and what passes between it and the game.
//
//   const ui = await openInterface({ realm, wasm, bundle, pak, plan });
//   …each frame, after the game's own step:
//   if (line) ui.send(line);                 // the game's state, as the guest's service delivers it
//   ui.turn(buttons, contacts, ticks);       // the guest's turn
//   for (const line of ui.drain()) …         // what the guest asks of the game
//   if (ui.changed()) upload(ui.pictures()); // only when what it shows has changed
//
// `plan` is the build plan PocketJS wrote for the device the bundle is for
// (plan.json): the surfaces, the raster density and which surface takes touch
// are read from it, so a page that offers several devices holds no table of
// its own.

/** What a plan says of the screens: sizes in logical pixels. */
export function screens(plan) {
  const primary = plan.modality.screens.find((s) => s.role === "primary");
  const auxiliary = plan.modality.screens.find((s) => s.role === "auxiliary");
  return {
    viewport: plan.viewport.logical,
    density: plan.viewport.rasterDensity,
    /** The primary surface in the pixels it is drawn with. */
    physical: plan.viewport.physical,
    auxiliary: auxiliary?.logical,
    /** The surface a contact lands on: "primary", "auxiliary" or "none". */
    touch: primary?.touch ? "primary" : auxiliary?.touch ? "auxiliary" : "none",
    buttons: plan.modality.buttons,
    /** The second stick: PocketJS counts the sticks of a target that has a touch panel and buttons. */
    glyphs: plan.modality.glyphs,
  };
}

/**
 * Starts the guest of `bundle` and `pak` for the device of `plan`, in a
 * hidden frame that loads `realm` (pocket3d-realm.html). Resolves when the
 * guest has run its first lines and opened `service`.
 */
export async function openInterface({ realm, wasm, bundle, pak, plan, service = "pocket.overlay" }) {
  const frame = document.createElement("iframe");
  frame.hidden = true;
  frame.tabIndex = -1;
  frame.setAttribute("aria-hidden", "true");
  const loaded = new Promise((resolve, reject) => {
    frame.addEventListener("load", resolve, { once: true });
    frame.addEventListener("error", () => reject(new Error("the interface's realm did not load")), { once: true });
  });
  frame.src = realm;
  document.body.append(frame);
  let guest;
  try {
    await loaded;
    // (the realm's module has run when its document has loaded)
    const shape = screens(plan);
    guest = await frame.contentWindow.Pocket3DRealm.create({ wasm, bundle, pak, viewport: shape.viewport, density: shape.density, auxiliary: shape.auxiliary, touch: shape.touch, service, id: plan.app.id });
    if (!guest.opened()) throw new Error(`the interface did not open ${service}`);
  } catch (error) {
    frame.remove();
    throw error;
  }
  let shown = null;
  return {
    plan,
    ...screens(plan),
    send: guest.send,
    drain: guest.drain,
    turn: guest.turn,
    /** Whether the primary surface shows something else than at the last `pictures()`. */
    changed: () => guest.hash() !== shown,
    pictures() {
      shown = guest.hash();
      return guest.pictures();
    },
    lower: guest.lower,
    /** Ends the guest and its realm. */
    close: () => frame.remove(),
  };
}
