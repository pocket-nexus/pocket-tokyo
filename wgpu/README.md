# Pocket Tokyo in a browser tab

The city of the handheld builds, drawn with [wgpu](https://wgpu.rs) 25: over WebGPU in a tab (wasm32), and over Metal on the machine that builds it, where a frame goes to a file. It reads the iPod touch's pack (`profiles/ipod60.json`) as it is and draws the iPod touch's passes. The game's own interface (`ui/`) runs over it, and the page shows it as one of four handhelds (PS Vita, PSP, Nintendo 3DS, iPod touch): its screens, its presentation of the interface, its buttons. `web/` is something else: the three.js model the city is compiled from.

| Part | What it is |
| --- | --- |
| `core/` | A Cargo manifest only. Its `[lib] path` is the 3DS core's source (`n3ds/core/src/lib.rs`), as the iPod touch's is, built with the `host` feature: the flight, the clock and the light, what a frame draws, the slots of the cells, the shadows' sweep and the flow around the flight, as the same `tk_*` functions, called from Rust. With `host` the program keeps its own allocator, the shadows' texels are in rows, and the screen's shape is a value (`tk_shape`). |
| `src/render.rs`, `src/shaders/city.wgsl` | The renderer: the seven programs of `ipod/src/render.c` in WGSL, and its draws. |
| `src/pack.rs` | The pack's head: the tables for the core, what the GPU draws from before any cell has arrived, and where each block's picture is. |
| `src/app.rs` | The shell: a frame as `step`, the guest's turn, `draw`; the reads of cells and of blocks' pictures; the screen's shape and each handheld's pad. |
| `src/web.rs`, `page/` | The tab: what the page calls, the page itself, and the worker that sweeps the shadows. |
| `src/bin/shot.rs` | A frame on this machine's GPU, written to a PNG, and two pictures compared. |
| `vendor/pocketjs/devices/web/pocket-web-wgpu` | PocketJS's browser kernel: what is not this game's. The device and the screens, the overlay pass, 16-bit pictures, ranges of a pack, and the page itself: the title card first and the frame loop, the interface's guest in its realm, and **the Pocket3D player** (the bar, each handheld's shell with its keys as the controls, the way to Pocket Studio). Its README states what a game implements; this directory is that for Pocket Tokyo. |
| `tools/wgpu.ts` | cook, build, serve, dist, shot, counts, check. |

```
bun tools/wgpu.ts cook               # the iPod touch's pack → .pocket-build/city/shiba/ipod60/city.pack
bun tools/wgpu.ts serve              # build, then http://127.0.0.1:8787/
bun tools/wgpu.ts shot --words "hour=13 rate=0 view=230,120,-200,0,150,0" --out a.png
bun tools/wgpu.ts counts             # an eye the iPod touch reported: the same triangles and draws here
bun tools/wgpu.ts check              # Chrome: every device from the title into a flight, by keys and pointer
bun tools/wgpu.ts dist               # the directory a static host serves
bun tools/wgpu.ts check --dist       # the same check of that directory
```

The build needs the `wasm32-unknown-unknown` target and `wasm-bindgen` 0.2.126 on the path (the version in `Cargo.lock`; `build` refuses another). It compiles the interface for the four devices as `bun tools/ui.ts` does, so it needs what that needs: `bun install` in `vendor/pocketjs`, and the exported area under `.pocket-build/city/<area>/ir` for the map. The kernel's page modules, the title card, the realm and PocketJS's UI core are staged by PocketJS's own tool (`stagePocket3dWeb` of `vendor/pocketjs/tools/pocket3d-web.ts`), which builds the UI core when the pin has none.

## The launch

The page plays the Pocket3D title card first (`playTitle()` of PocketJS's `pocket3d-title`, copied into the site as it is): 144 ticks, 2.4 s, over the whole page. While it plays the page opens the renderer on the canvas, starts the interface's guest and reads the pack's head and the device's shell. When the card has ended the canvas is shown. If the head is still on its way the frames show the interface alone, which says `Reading the city` (`tk_stage`), as a device's does; a start that fails is said the same way. A browser without WebGPU is told so in one sentence after the card; there is no other renderer.

## The pack over HTTP

Every read of the pack is `Source::range(offset, size)` of the kernel.

- **The head is 9.3 MB**: the tables, the vertices and indices of the levels that stay, the lamps' light and the facades. It is all a first frame of the city needs.
- **A block's picture of the ground is a read of its own** (699 KB, 48 of them, 33.5 MB together), after the first frame: two at a time, the block nearest the eye first, also while the title card plays. Until a block's picture has arrived its ground and roofs are one flat colour, the mean of the pack's pictures. A handheld reads these with the head, from the file beside it.
- **A cell's near level is one more read** of its record (227 KiB at most) when the core's slot wants it: up to four at once, the nearest cell first, and a frame hands at most two records and one block's picture to the GPU. Until a cell's record has arrived the core draws it at the mid level.

The source has two forms:

- **The pack's file**, on a server that answers byte ranges: each read is a request with a `Range` header. `serve` does this.
- **The pack cut into pieces of one size** with a manifest that lists them (`<meta name="pocket-pack">` names a `.json`). A read fetches the pieces it lies in, whole and with plain requests, all at once; two reads that need one piece share its request; the last 32 MiB of pieces stay in the page's memory, outside the module's. `dist` writes this form.

`bun tools/wgpu.ts dist` writes `.pocket-build/wgpu/dist` for a host that limits a file to 32 MiB and keeps a file ten minutes in a browser's cache (Pocket Studio's site deployments): `index.html` and `icon.png`; everything else of the site under `app/<build>/`, named by a hash of its contents (the module, PocketJS's UI core, the page's scripts and stylesheets, the handhelds' shells and the player's font, the interface for each device); `pack/<hash>.json` and the pieces, each named by its own hash. Only the page is asked for again at every visit, so a deployment of new code cannot meet an old piece of the pack or half of an old module. The pieces and their manifest are cut by the kernel's tool (`cutPack`). With pieces of 2 MiB the directory is **130 files and 184.3 MB**: 85 pieces and their manifest (177.1 MB), 42 files of the site (7.2 MB), the page and the icon. The largest file is the PS Vita's interface pak, 2.8 MB. `dist` refuses a directory the host would (a file over 32 MiB, more than 4 000 files or 1 GiB, a top-level `play/` or `runtime/`). A checkout that `pocket-studio register` has linked (`.pocket-studio.json`, which Git ignores) gets the project's id and the Studio's origin written into the page (`<meta name="pocket-app">`, `<meta name="pocket-studio">`): the player's door then leads to the game's card. Nothing uploads the directory.

## The interface

The interface is the game's own: `ui/`, compiled for a device by `tools/ui.ts`, the bundle and its pak as that device loads them. The page runs it as a guest in a realm of its own: PocketJS's AppInstance (`app-instance.html`, a hidden frame) on the UI core built for wasm (`pocketjs.wasm`), started without the text worker. The guest opens the same service it opens on a device, `pocket.overlay`, and the lines are carried in the page:

1. `step`: `tk_step` takes what the guest asked for on its last turn (`start`, `menu`, `tour`, `hour`, `drive`, `look`, …), runs the flight and writes the state the interface is shown. The flow from the title into a flight and the menu over it is `tokyo_interface::Session`, as on every device.
2. The guest's turn. **A turn is offered as often as the device's own host offers one**: 30 times a second on the PS Vita, the PSP and the 3DS, 60 on the iPod touch (`turns` in `SHAPES`). The realm is told that rate before the bundle runs (`simHz`, the device's `globalThis.__simHz`), so the guest's clock counts a turn as on the device. A turn is taken on the frames `tk_guest_due` says it is worth one: the line of state it has not seen goes in (`heard`), with the contacts and the buttons held at any moment since a turn was last offered, and what it sends comes back (`say`). The UI core then advances the turn's sixtieths of a second: 2 at 30 turns a second.
3. When the guest's draw hash has changed, its picture is drawn again and handed to the renderer.
4. `draw`: the scene, then the picture over it in a pass of its own, premultiplied.

**The picture comes with its alpha in one drawing.** PocketJS's UI core rasterizes the interface into a buffer cleared to zero and keeps what its ops cover (`renderPremultiplied`); the renderer lays those pixels over the scene as they are (`Overlay::write`). A redraw of the PS Vita's interface, 960 × 544 at two samples a logical pixel, takes 1.6 ms of an M3 Max: 0.85 ms for the drawing, the rest for the copy into the module and the upload. It happens 14 times a second in a flight, when the numbers change. Before the UI core had that render the page drew the interface twice, over black and over white, and took the difference: 3.3 ms a redraw.

A device's second screen is the interface's alone: the 3DS's lower screen is drawn into a canvas of its own from the core's opaque pixels, when its own draw hash changes (`drawHashAuxiliary`): 1.4 ms a redraw, 7 times a second in a flight. Drawn on every turn of the guest, as before that hash, it was 23 times a second.

The settings the interface keeps (`prefs`) are in the browser's `localStorage`, where a device has a file.

## The devices

The page shows one handheld at a time. A device is the renderer's shape (`SHAPES` in `src/app.rs`: the size, the samples a pixel, the triangles a frame may draw, the frames a second, the interface's turns a second, and what its buttons do for the flight), the interface's bundle for it, and the build plan PocketJS wrote for that bundle, from which the page reads the screens, the raster density and which surface takes touch.

| | Scene | Interface | Frames a second | Turns of the interface a second | Touch |
| --- | --- | --- | --- | --- | --- |
| PS Vita | 960 × 544 | `single`, 480 × 272 at two samples a pixel | 60 | 30 | the screen takes taps |
| PSP | 480 × 272 | `single`, 480 × 272 | 30 | 30 | none |
| Nintendo 3DS | 400 × 240 | `dual`: 400 × 240 over it, and a 320 × 240 screen under it | 30 | 30 | the lower screen |
| iPod touch | 480 × 320 | `touch`, 480 × 320 | 60 | 60 | the screen; no buttons |

**The page is PocketJS's Pocket3D player** (`createPlayer` of the kernel): `wgpu/page/index.html` has an empty body, and `main.js` says what the game is and draws into the player's canvas. The player builds the bar (the game's name, the devices as text, the word **Simulated** beside them, the keys and the notices as panels), the device's shell with the screens in it, and the dock that leads to Pocket Studio. This page has no line of its own: the sources of the city are in the interface's About list, on every device.

Picking another device in a flight changes the shell and the screen's shape, starts that device's guest in a new realm and leaves the flight as it is: the new guest is told the whole state on its first turn. The first device is the iPod touch for a browser whose pointer is a finger and the PS Vita otherwise (`?device=`).

**Each device says how its own build differs** (`note` in `DEVICES` of `main.js`, from the table of devices in the repository's README): the mark's panel shows the player's sentence, then that one. A change to what a device's build draws changes its note.

Whatever the device, the pack is the iPod touch's: a device changes how much of that pack a frame draws, not what the city was lowered to.

**The shell is shown with a whole number of the display's pixels to one of the screen's**, the largest at which the whole shell fits, as PocketJS's `integer-fit`; where that would make it less than three quarters of the size that fits (nine tenths in a window narrower than 900 CSS pixels), at the fraction that fits. In a window of 1440 × 900 at two display pixels a point the PSP's and the iPod touch's screens have three display pixels to one; the PS Vita's has 1.74 and the 3DS's 1.91, smoothed.

**A pointer is a finger** on a surface that takes touch: contacts are handed to the guest in the surface's logical pixels with PocketJS's touch hit facts, up to eight at once, and one that lifts before two turns have seen it stays for them.

**The keys are the device's buttons, and so are the shell's own.** A key, a d-pad's arm, a shoulder key or a stick on the picture takes a pointer or a finger; a key that is held is drawn down in its socket and a stick's cap slides, also when the keyboard holds them. The 3DS's shoulder keys are out of sight from the front: their names stand on the hinge's ends.

| | Keys |
| --- | --- |
| The stick (the left one of two) | W A S D |
| The d-pad | the arrows |
| The face buttons at the right, the bottom, the left and the top (○ ✕ □ △, or A B Y X) | Z X C V; Enter and Backspace are the first two |
| The same four by their place, on a device with one stick | L K J I |
| The right stick of a PS Vita | I J K L |
| L, R | Q, E |
| START, SELECT | Space (or Escape), Shift |

What the buttons do is each device's own (`Shape::pad`, from `psp/src/main.rs`, `vita/src/main.rs` and `n3ds/src/main.c`; the README's Controls).

The frame loop counts the display's refreshes: a device of 60 frames a second takes every refresh of a display of 60 and every second one of 120. A frame's ticks are whole when the time since the last frame is a whole number of sixtieths (within a twentieth); on another display the part of a tick left over is owed to the next frame, so the flight keeps its speed.

## The frame

1. `step`: what the interface asked for, then one tick of the flight per sixtieth of a second since the last frame, with the pad the device's buttons make.
2. The guest's turn and, when what it shows has changed, its picture.
3. What has arrived goes to the GPU: up to two cells' records, one block's picture.
4. The shadows: when the sun has moved a degree a sweep is asked for, and a sweep that has arrived is uploaded. In a tab a worker sweeps (`page/sweep.js` runs the module again with the city's heights alone): a sweep takes 4.2 ms of an M3 Max in wasm. The first sweep is the first frame's own.
5. `tk_choose` and `tk_refill`: the frame's draws, and which cells the slots are for; then the reads for the slots that want a cell.
6. One pass into a target with four samples a pixel, resolved into the canvas: the sky's dome, the ground and the roofs, the walls, what is painted, the landmarks' members. Then the interface over it.

What differs from the iPod touch's renderer:

- **A vertex's place is read as four 16-bit numbers.** WebGPU has no vertex format of three; the fourth is the two bytes after the place and is not used.
- **A picture goes to the GPU as `rgba8unorm`.** WebGPU has no texture of 16 bits with three colours. The levels the pack leaves out are made as on the device, by halving in the 16-bit colours.
- **Between two levels of a picture the GPU mixes them.** OpenGL ES reads the ground from the nearer level with two samples along its slant; WebGPU allows several samples only with mixed levels.
- **The matrix and the places of the pictures are records of one buffer**, written once a frame and picked by an offset at each draw.
- **Colours are computed in floats.** The device's `lowp` holds a value up to 2; a wall at night whose light and rooms sum above that is brighter here before the haze.
- **The interface is a pass of its own** over the resolved frame (the kernel's `Overlay`). On the iPod touch every program of the scene reads it, for the SGX's sake.

## Measured

Chrome 154 (headless, WebGPU on the Apple GPU, Metal 3) on an M3 Max, the pack of Shiba at `ipod60`, served from the same machine. Each device is flown on the tour with its interface over the scene.

| | PS Vita | PSP | Nintendo 3DS | iPod touch |
| --- | --- | --- | --- | --- |
| Frames a second over 5 s | 59.96 | 29.99 | 29.99 | 59.96 |
| A frame's cost without the display, over 300 frames | 0.41 ms | 0.31 ms | 0.56 ms | 0.34 ms |
| A redraw of the interface's picture | 1.61 ms | 0.44 ms | 0.38 ms | 1.18 ms |
| of which the UI core's drawing | 0.85 ms | 0.25 ms | 0.23 ms | 0.97 ms |
| The same redraw from two drawings, before | 3.34 ms | 0.89 ms | 0.76 ms | 2.28 ms |
| Redraws a second | 13.8 | 16.4 | 16.6 | 16.4 |
| Turns the guest took a second | 20.0 of 30 | 20.8 of 30 | 20.6 of 30 | 26.2 of 60 |
| A turn of the guest | 0.04 ms | 0.04 ms | 0.04 ms | 0.03 ms |
| The second screen | | | 1.42 ms a redraw, 7.0 a second (before: every turn, 23 a second) | |
| Triangles and draws of the frame measured | 53 400, 356 | 37 400, 313 | 49 000, 352 | 22 400, 254 |

- **The first frame of the city needs 13.8 MB** on the PS Vita's screen: 9.3 MB of the pack, the module (0.51 MB; 0.17 MB gzipped), PocketJS's UI core (0.36 MB), the interface (0.26 MB of script, 2.78 MB of pak; 0.57 MB of pak on a PSP, 0.70 MB on the others) and **the player: 0.13 MB** (the PS Vita's shell, 81 kB in two pictures; the font, 15 kB; 37 kB more of scripts and styles than the page before it). The PSP's shell is 132 kB, the 3DS's 75 kB, the iPod touch's 33 kB; the other three are read after the first frame. On a line of 16 Mbit/s the interface is up at 3.9 s, the city flies at 6.8 s with flat ground, and every picture has arrived at 34 s. With the pictures read with the head, as before, the first frame needed 44 MB. From pieces of 2 MiB: 16.9 MB, 8.4 s, 42 s.
- **Beside the iPod touch's own capture** of one eye in a flight at 21:00 with its interface on the screen (the stick, the keys, the clock, the hint): the mean difference of a colour is 1.0 of 255, and 2.6 % of the pixels differ by more than 16, all of them along the tower's members and the edges of buildings. The interface's pixels are the device's.
- **The eye of an iPod touch's status** (`counts`): 10 362, 12 363, 0 and 1 272 triangles in 231 draws, 11 blocks and 25 regions, 8 647 triangles turned away, on the device and here, with every picture arrived.
- **The tab's scene beside this machine's own** of one view: a mean difference of 0.003 of 255.
- **The site**: the module is 517 723 bytes; 7.25 MB with the UI core (365 434 bytes), the four devices' interfaces, and the player's shells and font (0.37 MB).

`check` drives the real page with real input: on each device the title's list, a flight, the menu opened and closed; the PS Vita's panel under a tap; the 3DS's lower screen under the pointer (its lists, its Menu and Tour keys, the day's bar dragged); the iPod touch's stick, its keys and a finger on the city; another device picked in a flight; a setting kept over a reload; a browser without WebGPU. Then **the player in three windows** (1440 × 900 at two display pixels a point, 1280 × 800 at one, and 390 × 844 at three with a finger for a pointer), each device in each: the shell whole between the bar and the dock, the screens where the shell's picture has them and nothing over them, the mark's sentences under a pointer or a finger, the door's address, a key of the shell held (it goes down, and the title gives way to a flight), the shell's stick (the eye flies, the cap slides), the d-pad under a thumb, the 3DS's lower screen in its shell, the keys as a list, another device picked from the bar, and the notice about the devices' names; and a phone on its side.

Not measured: another browser, another GPU, a phone itself, a network that is not this machine's (the line of 16 Mbit/s is Chrome's own throttle).
