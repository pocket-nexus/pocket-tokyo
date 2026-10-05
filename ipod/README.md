# Pocket Tokyo on the iPod touch 4

480 × 320 with four samples a pixel at 60 frames a second, OpenGL ES 2 on the SGX535, iOS 6. The device has a touch panel and no pad, so the city is flown through the interface's touch presentation (`ui/app/presentations/touch.tsx`): a stick, three keys, a finger on the city to turn the view, the clock as a bar to drag. The shell draws no 2D of its own.

| Part | What it is |
| --- | --- |
| `core/` | A Cargo manifest only. Its `[lib] path` is the 3DS core's source (`n3ds/core/src/lib.rs`), built for `armv7-apple-ios`: the flight, the clock and the light, what a frame draws, the slots of the cells, the shadows' sweep, the flow around the flight (`tokyo_interface::Session`) and the interface's channel, behind the `tk_*` functions of `n3ds/src/core.h`. What differs for this device is under `target_vendor = "apple"`: the screen's shape, row order for the shadows' texture, the allocator, `tk_card`, and how often the numbers in flight go to the interface. |
| `src/render.c` | The renderer: the 3DS's passes as OpenGL ES 2 programs, each of which also reads the interface's picture at its own pixel. |
| `src/main.c` | The shell: UIKit through the Objective-C runtime, the render thread, the pack's tables, the thread that reads cells and sweeps shadows, the title card, the guest's turn and its texture, touches, the settings' file, the control and status files. |
| `tools/ipod.ts` | cook, build, package, deploy, launch, drive, capture, bench. It compiles `ui/` for the device (`tools/ui.ts`) and builds PocketJS's UI core (OpenGL ES 2 backend), QuickJS and the guest driver into the app. |

## The pack

`bun tools/ipod.ts cook` (or `bun tools/tokyo.ts cook --profile ipod60`) lowers the city as for the 3DS: prisms at every level, each cell's near level one record, walls and painted geometry ordered by 16 sectors, the 3DS's vertex layouts. What differs:

- **Pictures are 16-bit texels in row order**, the image's first row first (`UNSIGNED_SHORT_5_6_5`): a block's ground at 512² (1 m a texel), a cell's at 256² (0.5 m, three levels in its record), the facades by day and what their windows emit at 1 024 × 128, the lamps' light at 1 024². The device makes the levels below the stored ones.
- **A wall vertex's sector byte carries three more bits**: how late in the dusk the building's rooms come on.
- **The landmark's lamps are in its models** (`landmarks.lamps` in the profile): 118 on Tokyo Tower.

The pack is 177 MB: 34 MB of block pictures, 134 MB of cell records (227 KiB the largest), the rest tables and the levels that stay. The process holds about 65 MB while it flies.

## The launch

The Pocket3D title card plays first, before the guest starts and before the pack is read: 144 ticks, 2.4 s. PocketJS's drawers write a console's frame buffer, and this device has none a CPU writes, so the core draws each tick's frame into memory (`tk_card`, over `pocket3d_title::draw`) and the shell uploads it to a texture. A frame that differs from the last is uploaded; the held frames in the middle are not. The card follows the clock, so a slow frame skips a tick and the card keeps its length. `bun tools/ipod.ts title --out a.png` launches the app and brings back the held frame as the device presented it.

The guest boots next (`tokyo.js`, `tokyo.pak` in the bundle), so the step "Reading the city" is the interface's own screen (`tk_stage`); the pack is read once that screen has had two frames.

## The frame

1. `tk_step`: what the interface asked for since the last frame (`start`, `tour`, `hour`, `drive`, `look`, …), then one tick of the flight per display refresh. The pad handed in is empty.
2. The guest's turn, on the frames `tk_guest_due` says it is worth one: the contacts go in (eight slots through PocketJS's contact latch, the panel's points turned to the interface's 480 × 320), the flight's state is read, commands are left for the next `tk_step`. When what the interface shows has changed (its draw hash), and not on two frames in a row, it is drawn into a texture. There are two textures, used in turn.
3. What the reading thread has ready goes to the GPU: one cell's record (its vertices and indices into the slot's buffers, its picture into the slot's texture) and one strip of 128 rows of the shadows.
4. `tk_choose` and `tk_refill`: the frame's draws, and which cells the slots are for.
5. The scene, into a target with four samples a pixel: the haze over the whole target as two triangles, the sky's dome, the ground and the roofs, the walls, what is painted, the landmarks. The drawable is the portrait screen, untransformed and opaque, and every matrix ends with a quarter turn.
6. The samples are resolved into the screen's buffer, then the present.

The programs:

- **Ground and roofs**: (light × shadow + lamps × night) × picture. The lamps' light and the shadows are one texture each over the whole city. Three programs: by day no lamp is read, deep in the night no shadow.
- **Walls**: light × tint × the facade, and by night plus what the windows emit × how far that building's rooms are on. The light is one of 17 uniforms, picked by the vertex's sector.
- **Painted geometry and landmarks**: light × colour; a vertex with alpha 255 is a lamp, and adds its own colour × the night.
- **Haze**: the Vita's formula, per vertex.
- **The interface**: every program ends with one more read, of the interface's texture at the pixel's own place (a projective read from a varying), and writes colour × (1 − its alpha) + its colour. The two triangles of haze drawn first give a program, and so the interface, to the pixels nothing else covers (past the city's edge, under the dome's lowest ring); the GPU shades a pixel for the last opaque triangle on it, so they cost what they alone show.

The thread that reads cells and sweeps the shadows runs below the render thread's priority. A cell's record goes into one staging block and waits there for the render thread; a sweep of the shadows (0.2 to 0.5 s) starts when the sun has moved a degree and the last one has been uploaded.

## What limits the frame

Measured on an iPod touch 4 (A4, SGX535, iOS 6.1.6), each window by the device itself (`mark=SECONDS`).

| | |
| --- | --- |
| The GPU, a frame drawn alone (`option=512`: `glFinish` after the scene), four samples | 10.8 ms at 12 600 triangles, 13.7 ms at 20 300, 18.0 ms at 30 200, 21.1 ms at 39 000: **5.9 ms + 0.39 ms per 1 000 triangles** |
| The same with one sample (`option=1536`) | 11.1 ms at 12 600, 16.1 ms at 30 200, 18.7 ms at 39 000 |
| A flight, four samples: consecutive frames overlap in the GPU | 34 400 triangles: no late frame by day, 2 % at dusk and by night. 44 300: every other frame late |
| A flight, one sample (`option=1024`) | 51 900 triangles: 0 to 8 late frames in 481 at any hour. 62 600 by day: 3 late. 58 900 at dusk: every other frame late |
| The CPU | flight 0.06 ms, choosing the draws 2.1 ms, the scene's commands 2.4 to 3.1 ms for 240 to 330 draws (about 10 µs a draw), arrivals 0.05 ms |

Four samples cost a third of the triangles a refresh allows. The lattice of a tower and the edges of 14 000 buildings are what the picture is made of at 480 × 320, so the build takes the samples; without the interface that left a budget of 32 000 triangles.

### The interface

A layer over the whole screen is what this GPU has least room for. Three ways of laying the interface's texture over the scene, each on the tour at a budget of 32 000 triangles:

| | Late frames |
| --- | --- |
| A blended quad as the scene's last draw, inside the four-sample target | 38 % (150 s) |
| The samples resolved into a texture, then one opaque pass that reads the scene and the interface | 49 % (60 s) |
| **A read in every program of the scene** | 12 % (60 s) |

The third is the build's. What it still costs is paid in triangles: one view of 30 800 triangles that missed no refresh before misses a third of them by day and half by night with the read, and holds at 24 800 (7 late frames in 474 by night). On the tour, 26 000 triangles a frame leave 1.1 % of the frames late and 24 000 leave 0.2 to 0.3 %. **The budget is 24 000: the interface takes 8 000 triangles of the 32 000.** A 16-bit texture for the interface did not change the night's figure.

The guest itself, on the tour:

| | |
| --- | --- |
| A turn | 1.0 ms, 13 a second (`tokyo_interface::Pace`); every frame while a finger is down |
| A redraw of the texture (the check of the draw hash, then the draw's commands) | 5.1 ms of the CPU, 11 a second: the numbers in flight go to the interface every fifth frame (`NUMBERS_EVERY` in the core, 12 lines a second) |
| Memory | the process holds 72 MB with the guest, 64 MB without |

Three things that cost the frame rate on this GPU, each measured before and after:

- **A vertex attribute that does not start on a 4-byte boundary.** A ground vertex is three shorts and two bytes; with the bytes read as an attribute at offset 6, the driver copied and converted vertices at every draw: 242 draws took 34.7 ms of the CPU. Reading four bytes from offset 4 (the last two are the vertex's own) took the same draws to 3.3 ms.
- **A fragment colour that is not `lowp`.** The SGX keeps a `lowp` vector in one register and works on it in one instruction; a value carried to `mediump` and back costs an instruction each way. With `mediump` intermediates, a night frame of 43 000 triangles (one sample) missed 13 % of its refreshes and a dusk frame 25 to 33 %; with every colour in `lowp` the same frames miss none. Light travels at half scale and is doubled by an addition.
- **Writing into a texture a queued frame reads**, which makes a tile-based driver copy the texture first. With the shadows uploaded a strip a frame into the texture in use, and the reading thread at the render thread's priority, the render thread's uploads took 21 ms at the 95th percentile. The shadows now have two textures (a sweep goes into the one no frame reads, and frames read it from its last strip on) and the reading thread runs below the render thread: 0.8 ms at the 95th percentile. The two changes were measured together.

The tour (`bun tools/ipod.ts bench`: the flow in `flight`, the tour at the controls, the instruments over it), 150 s from its start at 15:30 into the night, the governor on:

| | Frames | Late frames | Average frame | Worst frame | Triangles | Draws |
| --- | --- | --- | --- | --- | --- | --- |
| Without the interface, budget 32 000 | 8 995 | 6 (0.07 %) | 16.68 ms | 38.0 ms | 19 400 to 34 100, mean 29 300 | mean 233, most 330 |
| **With the interface, budget 24 000** | 8 965 | 37 (0.41 %) | 16.73 ms | 39.8 ms | 16 600 to 27 500, mean 22 200 | mean 205, most 309 |

Windows of 20 s: behind the title 3 late frames in 1 199; under the menu 3 in 1 199; a flight with a thumb held on the stick (a turn of the guest every frame) 16 in 1 183. Reading a cell takes the thread 8 to 10 ms (826 cells in 6.9 s of reading over one long run).

## Development loop

```
bun tools/ipod.ts cook
bun tools/ipod.ts deploy            # first install (MobileInstallation); the .ipa is 177 MB
bun tools/ipod.ts native [--pack]   # replace the executable and the interface (and the pack)
bun tools/ipod.ts launch
bun tools/ipod.ts ctl "ui=fly hour=19 rate=0 view=230,120,-200,0,150,0"
bun tools/ipod.ts ctl "touch=86,190;424,170"   # a thumb on the stick, one on UP
bun tools/ipod.ts capture --out a.png
bun tools/ipod.ts bench --seconds 150
bun tools/ipod.ts reset             # stop the app and remove the settings it kept
```

`ctl` words go to the flow (`mode=title|flight|menu`, `ui=tour|fly|menu|resume|title`: what the interface would ask), to the flight (`tour= restart= at= hour= rate= budget= reach=a,b near= mid= sectors= clearance= view=x,y,z,tx,ty,tz[,fov] view=off fly=… option=`) and to the shell: `touch=X,Y[;X,Y]` holds fingers on the panel in the interface's pixels and `touch=off` lifts them, `tap=X,Y` is one finger for a few turns, `screen=1` writes the presented frame, `mark=SECONDS` measures. `option` is a sum: 1 no culling, 2 the other winding, 8 no haze, 32 / 64 / 128 / 256 skip the ground, the walls, the painted geometry, the landmarks, 512 time the GPU, 1024 one sample a pixel.

`launch` and `bench` wait while another tool drives the device: control files or captures written in another app's container in the last three minutes.

## Not in this build

- An overlay that costs what it covers. The interface is read at every pixel; in a flight it covers about a quarter of the screen. Blending it only over the cells that hold something needs to know those cells without reading the texture back.
- The traffic, water and the night's glow, as on the PSP and the 3DS. Sound: the city has none on any device.
