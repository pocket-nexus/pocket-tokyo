# Pocket Tokyo in a browser tab

The city of the handheld builds, drawn with [wgpu](https://wgpu.rs) 25: over WebGPU in a tab (wasm32), and over Metal on the machine that builds it, where a frame goes to a file. It reads the iPod touch's pack (`profiles/ipod60.json`) as it is, and draws the iPod touch's passes. `web/` is something else: the three.js model the city is compiled from.

| Part | What it is |
| --- | --- |
| `core/` | A Cargo manifest only. Its `[lib] path` is the 3DS core's source (`n3ds/core/src/lib.rs`), as the iPod touch's is, built with the `host` feature: the flight, the clock and the light, what a frame draws, the slots of the cells, the shadows' sweep and the flow around the flight, as the same `tk_*` functions, called from Rust. With `host` the program keeps its own allocator, the shadows' texels are in rows, and the screen's shape is a value (`tk_shape`). |
| `src/render.rs`, `src/shaders/city.wgsl` | The renderer: the seven programs of `ipod/src/render.c` in WGSL, and its draws. |
| `src/pack.rs` | The pack's head: the tables for the core, and what the GPU draws from before any cell has arrived. |
| `src/app.rs` | The shell: a frame's steps in the iPod touch's order, the reads of the cells the slots want, the screen's shape. |
| `src/web.rs`, `page/` | The tab: what the page calls, the page itself, and the worker that sweeps the shadows. |
| `src/bin/shot.rs` | A frame on this machine's GPU, written to a PNG, and two pictures compared. |
| `kernel/` | `pocket-web-wgpu` and `web/pocket3d-shell.js`: what is not this game's. The GPU device and the screen a frame goes to (a canvas, or a texture that is read back), pictures of 16 bits a texel, ranges of a pack, the frame loop, the keys, a drag. Nothing in it names Tokyo: it is the part another Pocket3D game's tab would share. |
| `tools/wgpu.ts` | cook, build, serve, dist, shot, counts, check. |

```
bun tools/wgpu.ts cook               # the iPod touch's pack → .pocket-build/city/shiba/ipod60/city.pack
bun tools/wgpu.ts serve              # build, then http://127.0.0.1:8787/
bun tools/wgpu.ts shot --words "hour=13 rate=0 view=230,120,-200,0,150,0" --out a.png
bun tools/wgpu.ts counts             # an eye the iPod touch reported: the same triangles and draws here
bun tools/wgpu.ts check              # the tab in Chrome: pictures, frames a second, the keys
bun tools/wgpu.ts dist               # the directory a static host serves
bun tools/wgpu.ts check --dist       # the same check of that directory
```

The build needs the `wasm32-unknown-unknown` target and `wasm-bindgen` 0.2.126 on the path (the version in `Cargo.lock`; `build` refuses another). `cargo test --workspace` in `wgpu/` runs the kernel's tests: the pictures' levels, a manifest, and a pack in pieces read beside its file.

## The launch

The page plays the Pocket3D title card first (`playTitle()` of PocketJS's `pocket3d-title`, copied into the site as it is): 144 ticks, 2.4 s, over the whole page. The module and the pack's head are read while it plays, and the canvas is shown when it has ended. A browser without WebGPU is told so in one sentence after the card; there is no other renderer.

## The pack over HTTP

Every read of the pack is `Source::range(offset, size)` (`kernel/src/source.rs`). The head is one read: everything before `NEAR`, 42.9 MB (the tables, the levels that stay, 34 MB of block pictures). A cell's near level is one more read of its record (227 KiB at most) when the core's slot wants it: up to four reads at once, the nearest cell first, and a frame hands at most two records to the GPU. Until a cell's record has arrived the core draws it at the mid level.

The source has two forms:

- **The pack's file**, on a server that answers byte ranges: each read is a request with a `Range` header. `serve` does this.
- **The pack cut into pieces of one size** with a manifest that lists them (`<meta name="pocket-pack">` names a `.json`). A read fetches the pieces it lies in, whole and with plain requests, all at once; two reads that need one piece share its request; the last 32 MiB of pieces stay in the page's memory, outside the module's. `dist` writes this form.

`bun tools/wgpu.ts dist` writes `.pocket-build/wgpu/dist` for a host that limits a file to 32 MiB and keeps a file ten minutes in a browser's cache (Pocket Studio's site deployments): `index.html` and `icon.png`; the module and its scripts under `app/<build>/`, named by a hash of their contents; `pack/<hash>.json` and the pieces, each named by its own hash. Only the page is asked for again at every visit, so a deployment of new code cannot meet an old piece of the pack or half of an old module. With pieces of 2 MiB the directory is **95 files and 177.7 MB**: 85 pieces and their manifest, 7 files of the module (0.6 MB), the page and the icon. `dist` checks the pieces against the pack's hash and refuses a directory the host would (a file over 32 MiB, more than 4 000 files or 1 GiB, a top-level `play/` or `runtime/`). Nothing uploads it.

## The frame

1. `tk_step`: one tick of the flight per sixtieth of a second since the last frame, with the pad the page made of its keys.
2. What has arrived goes to the GPU: up to two cells' records (vertices and indices into the slot's buffers, the picture into the slot's texture).
3. The shadows: when the sun has moved a degree a sweep is asked for, and a sweep that has arrived is uploaded. In a tab a worker sweeps (`page/sweep.js` runs the module again with the city's heights alone): a sweep takes 4.2 ms of an M3 Max in wasm. The first sweep is the first frame's own.
4. `tk_choose` and `tk_refill`: the frame's draws, and which cells the slots are for; then the reads for the slots that want a cell.
5. One pass into a target with four samples a pixel, resolved into the canvas: the sky's dome, the ground and the roofs, the walls, what is painted, the landmarks' members.

What differs from the iPod touch's renderer:

- **A vertex's place is read as four 16-bit numbers.** WebGPU has no vertex format of three; the fourth is the two bytes after the place and is not used.
- **A picture goes to the GPU as `rgba8unorm`.** WebGPU has no texture of 16 bits with three colours. The levels the pack leaves out are made as on the device, by halving in the 16-bit colours.
- **Between two levels of a picture the GPU mixes them.** OpenGL ES reads the ground from the nearer level with two samples along its slant; WebGPU allows several samples only with mixed levels.
- **The matrix and the places of the pictures are records of one buffer**, written once a frame and picked by an offset at each draw.
- **Colours are computed in floats.** The device's `lowp` holds a value up to 2; a wall at night whose light and rooms sum above that is brighter here before the haze.
- **Nothing is laid over the frame.** The interface is not drawn yet; it will be a pass of its own.

## The screen

The screen is not one machine's. A shape is a size, the samples a pixel, the triangles a frame may draw and the frames a second (`SHAPES` in `src/app.rs`): `psp` 480 × 272 at 30, `vita` 960 × 544 at 60, `3ds` 400 × 240 at 30, `ipod` 480 × 320 at 60, each with its own build's budget. `?shape=` picks one (the default is `vita`), `?size=`, `?samples=`, `?budget=` and `?hz=` change it, and `pocketTokyo.reshape("psp")` in the console changes it while the city flies. The canvas is shown at the largest whole multiple of its pixels that fits the window.

Whatever the shape, the pack is the iPod touch's: a shape changes how much of that pack a frame draws, not what the city was lowered to.

The frame loop counts the display's refreshes: a shape of 60 a second takes every refresh of a display of 60 and every second one of 120. A frame's ticks are whole when the time since the last frame is a whole number of sixtieths (within a twentieth); on another display the part of a tick left over is owed to the next frame, so the flight keeps its speed.

## Keys

| Fly | Look | Climb, descend | Faster | Clock | Tour |
| --- | --- | --- | --- | --- | --- |
| W A S D | the arrows; a drag on the city | E, Q | Shift | [ and ] | T |

A key, or a drag, takes the eye off the tour where it is; T hands it back. A drag is the interface's `look` command, in pixels of a screen 480 wide.

`pocketTokyo.tokyo.control("hour=19 rate=0 view=230,120,-200,0,150,0")` in the console, or `?words=`, sends the flight a development host's words (`Flight::control`); `pocketTokyo.tokyo.status()` is the run as JSON.

## Measured

Chrome 154 (headless, WebGPU on the Apple GPU, Metal 3) on an M3 Max, the pack of Shiba at `ipod60`, served from the same machine.

| | 480 × 320, 4 samples | 960 × 544, 4 samples |
| --- | --- | --- |
| Frames a second on the tour, over 5 s | 59.96 (the display's rate) | 59.97 |
| A frame's cost without the display, over 300 frames | 0.09 ms | 0.10 ms |
| Triangles and draws of the frame measured | 21 000, 256 | 53 500, 357 |

- **The module is 489 041 bytes, 156 917 gzipped**, and its JavaScript side 65.7 KB (13.5 KB gzipped).
- **44.0 MB are read before the first frame** from the file (45.2 MB from pieces of 2 MiB): the module and the pack's head. The city could be drawn 0.11 to 0.17 s after the page started; the first frame is at 2.44 s, when the title card has ended.
- **90 s of the tour at 960 × 544 through dusk into the night**: 5 400 frames, 1 late, worst 16.8 ms, 273 cells read, 99 MB received from pieces (94 MB by ranges).
- **The eye of an iPod touch's status** (`counts`): 10 362, 12 363, 0 and 1 272 triangles in 231 draws, 11 blocks and 25 regions, 8 647 triangles turned away, on the device and here.
- **Beside the iPod touch's own captures** of `view=230,120,-200,0,150,0`: the mean difference of a colour is 2.8 of 255 by day, 1.5 at dusk and 1.7 by night; 6 %, 3 % and 3 % of the pixels differ by more than 16, along the tower's members and the edges of buildings.
- **The tab's frame beside this machine's own** of that view: a mean difference of 0.003 of 255.

Not measured: another browser, another GPU, a phone, a network that is not this machine's.
