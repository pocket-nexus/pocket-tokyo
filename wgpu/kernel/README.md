# pocket-web-wgpu

What a Pocket3D game drawn with wgpu needs in a browser tab and that is not the game's. Nothing here names Tokyo or depends on its crates; another game's tab takes the same parts. The Rust crate also builds for the machine that builds the game, where a frame goes to a file.

| Part | What it is |
| --- | --- |
| `src/gpu.rs` | The device, and the screen a frame goes to: a canvas (WebGPU), or a texture that is read back. Several samples a pixel, a depth buffer, another size while the game runs. |
| `src/overlay.rs` | A picture laid over a frame in a pass of its own, premultiplied: a game's interface. `write_pair` takes the picture as an opaque rasterizer drew it over black and over white and recovers what it covers. |
| `src/picture.rs` | Pictures of 16 bits a texel (`r5 g6 b5`) as `rgba8unorm` textures, with the levels a pack leaves out. |
| `src/source.rs` | Ranges of a pack: HTTP `Range` on its file, or whole pieces of a pack cut into files of one size with a manifest. From disk outside a tab. |
| `src/task.rs` | Something that waits, started from a frame; a clock. |
| `web/pocket3d-shell.js` | Whether the browser has WebGPU; the frame loop, in step with the display at a rate the game asks for. |
| `web/pocket3d-realm.html`, `web/pocket3d-realm.js` | One realm for a game's interface: a hidden frame of the page in which a PocketJS guest runs on PocketJS's UI core, with the service it opens answered by the page. |
| `web/pocket3d-interface.js` | The page's side of that realm: start a guest for a device from its build plan, hand it its turns, read what it drew when that has changed. |
| `web/pocket3d-controls.js` | A handheld's controls: its buttons and sticks from the keyboard and from buttons drawn on the page, and the contacts of pointers on a surface that takes touch. |
| `web/pocket3d-stage.js` | A handheld's screens on the page at a whole number of display pixels, a second screen under the first, room for the buttons at its sides; the choice of device as text. |

The realm and the controls import `./wasm-ops.js` and `./pocketjs-host.js`. The first is PocketJS's `hosts/web/wasm-ops.js` as it ships. The second is one module a game's build bundles from PocketJS: `__packTouch` and `createTouchHitFacts` of `framework/src/touch.ts`, `PROP` and `BTN` of `contracts/spec/spec.ts` (`tools/wgpu.ts` does it for Pocket Tokyo).

What the game keeps: its pack and its renderer, what its buttons do, and the page that puts these parts together (`../page/main.js` is that page for Pocket Tokyo, 250 lines).

## What PocketJS's UI core would add

- **A picture with its alpha.** `engine/wasm` exports `ui_render_scaled(scale)`, whose rasterizer writes every pixel's alpha as 255 (`engine/core/src/raster.rs`). An interface over a scene needs what it covers, so the realm draws it twice, over a black and over a white root, and `overlay.rs` takes the difference. An export that rasterizes the same draw list once into a buffer cleared to zero and keeps the alpha it blends (`dst.a = src.a + dst.a × (1 − src.a)`, colour premultiplied), for the primary surface at a scale, would be one drawing instead of two and no recovery. `Overlay::write` takes that picture as it is.
- **A draw hash for the auxiliary surface.** `ui_draw_hash` covers the primary surface. A second screen is drawn on every turn of the guest because nothing says whether it changed.
- **A realm without the text worker.** `hosts/web/app-instance.js` is this realm with a text worker and its wasm started for every guest; an interface with baked glyphs has no use for them, which is why `pocket3d-realm.js` is its own file.
