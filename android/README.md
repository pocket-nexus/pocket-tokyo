# Pocket Tokyo on the Redmi 1S

An Android app for the Redmi 1S (`armani`: MIUI V5, Android 4.3, Snapdragon 400, Adreno 305): the city in the panel's own 1280 × 720 pixels, OpenGL ES 3.0, a frame a refresh. The phone has a touch panel and three keys under it, so the city is flown through the interface's touch presentation at 640 × 360 with two pixels a point (`ui/app/main-touch-wide.tsx`): a stick, three keys, a finger on the city to turn the view, the clock as a bar to drag. The back key opens the menu and closes it. The shell draws no 2D of its own.

The package is one `NativeActivity` (`dev.pocketnexus.tokyo`, "Pocket Tokyo"): no Java, and the pack inside it.

| Part | What it is |
| --- | --- |
| `core/` | A Rust static library (`std`, `armv7-linux-androideabi`): the pack, the OpenGL ES 3.0 programs and buffers, what a frame draws (`tokyo_sim::view::select`), the shadows' sweep on a thread, the traffic, the flight and the flow around it (`tokyo_interface::Session`), the governor, the title card's frames, and the interface's channel. Behind the `tk_*` functions of `src/core.h`. |
| `shaders/` | Nine GLSL ES 3.00 sources: the ground and roofs, walls, painted geometry, cars, the sky. |
| `src/main.c` | The shell: the window and its EGL surface, the touches, the title card's presentation, the PocketJS guest and its draw over the city, the control and status files, the GPU's timer. |
| `src/loader.c` | What `NativeActivity` loads. It hands the activity to the engine, a library of its own, so a development run replaces the engine without an install. |
| `src/bionic18.c` | Five functions Rust's standard library links against that Android 4.3's C library lacks. |
| `tools/android.ts` | doctor, cook, build, package, install, replace, launch, drive, capture, bench. It compiles `ui/` for the phone (`tools/ui.ts android`) and builds PocketJS's UI core (OpenGL ES 2 draw-list backend), QuickJS and the guest driver into the engine. |

## What the phone charges for

Measured in this app on the phone, with the driver's own timer for a frame (`gpuTimerMs` in the status) and one view held still: 133 500 triangles in 250 draws, no interface, 1280 × 720, the GPU at 450 MHz.

| | |
| --- | --- |
| **A number that crosses a triangle** | The frame takes the GPU 15.7 ms. With the walls drawn as their facade alone (2 numbers a vertex in place of 10) it takes 11.1 ms; without the building's colour (4 numbers fewer) 14.1 ms; without the haze (1 fewer) 15.1 ms. The ground as its picture alone (2 in place of 6): 14.2 ms. That is **0.4 to 0.6 ms for each number 58 000 triangles interpolate**. The frame before these programs carried 12 to 16 numbers on every vertex and took 20.3 ms for 150 000 triangles. `flat` saves nothing: 15.67 ms with it on the building's colour, 15.44 ms without. |
| **The frame by what is drawn** | The walls 6.5 ms (58 600 triangles), the ground and roofs 4.2 ms (57 100), painted geometry and the landmark 1.1 ms (17 800), the sky 0.7 ms, an empty frame 2.4 ms (at 200 MHz: the clock follows the load). Reading the shadows costs the ground 0.5 ms and the walls 0.4 ms. |
| **Order** | Nearest place first, farthest first and the pack's own order: 20.3, 19.9 and 21.4 ms for the frame before these programs. A pass into the depth buffer alone before it: 33.6 ms. |
| **The window's size** | The same frame in a buffer of 960 × 540, which the display processor scales to the panel: 11.5 ms. A minute of the tour by day in each size, on two cores: at 1280 × 720 the governor's scale ends at 0.28 with 10 late frames, at 1120 × 630 at 0.67 with 16, at 960 × 540 at 0.76 with 25. A smaller buffer buys distance and no fewer late frames. The app keeps the panel's pixels; `bun tools/android.ts boot "width=960 height=540"` is the smaller buffer for a development run. |
| **The hour** | With the lamps' light and the rooms' light read beside the shadows, 74 400 triangles take 14.7 ms where the day's programs take 12.7 ms. So a program reads the shadows or the lights: by day shadows, by night the lamps and the rooms. Only the walls have a set with both, for the dusk. |
| **An index of 32 bits** | One vertex array over the whole pack with 32-bit indices cost the render thread 5 to 30 ns an index at every draw: 147 µs for a block's ground. The pack keeps 16-bit indices from each batch's first vertex, and each batch has a vertex array that starts there. |
| **Draws merged through a dynamic index buffer** | No GPU time saved, and 2.9 ms of copying with 7 ms of `glBufferData` on the render thread. A draw's cost is its change of uniforms, and a place changes them once. |
| **Memory** | The driver keeps its own copy of every buffer: 111 MB of vertices and indices were 191 MB of GPU memory and 157 MB of the process's heap. |
| **The timer** | `EXT_disjoint_timer_query` around the whole frame leaves out 2.4 ms of it: the kernel counts the GPU busy 91 % of the time (`gpubusy`) when the timer reads 12.7 ms. A refresh of 16.7 ms is 14.2 ms of the timer. |
| **Heat** | Above 60 °C at the SoC the system leaves the phone two of its four cores, at 1.0 GHz. The shadows' sweep is a quarter of a second of one core: at the frame's priority a minute of the tour had 29 late frames by day; below it (`setpriority` 19), 10. Under the tour the GPU's clock read 450 MHz in every status; it falls to 320 and 200 MHz under a light frame. |

**The window has no samples of its own.** With two a pixel from the EGL config (`boot "samples=2"`), a held frame of 81 100 triangles under the interface went from 14.6 ms of the GPU and a frame a refresh to 20.2 ms and a frame every 23.5 ms; at the governor's shortest distances (69 100 triangles), from 13.2 ms to 18.4 ms and a frame every 21.9 ms. The panel has 312 pixels an inch.

## Measured

`bun tools/android.ts bench --seconds 150`: from the title into the tour, the interface over it, the status read every 5 s. Build `3718ab794741`, the SoC at 57 to 59 °C.

| | |
| --- | --- |
| Frames | 8 937 in 150 s, **73 late (0.82 %)**, worst 45.4 ms, mean 16.79 ms |
| Where the late frames are | By the status' count since the launch (85, of which 73 in the 150 s): 9 by the first reading, 7 over the next 45 s of day, 14 over the 20 s in which the lights come on (17:13 to 17:50), **43 over the 10 s after that** (to 18:07: the walls read the shadows and their rooms, and the timer reads 14.3 to 15.9 ms at the shortest distances), none over the next 50 s of night, 12 over the last 25 s |
| Triangles a frame | 46 300 to 93 500, mean 66 800; 83 to 166 draws, mean 135 |
| The GPU's timer | 11.0 to 15.9 ms, mean 12.3 ms |
| The governor's scale | 0.25 to 0.80, mean 0.36: the near level to 75 to 240 m, the mid level to 325 to 1 040 m |
| The render thread | the flight 0.9 ms, the guest's turn 0.2 ms, the interface's draw 1.8 ms, the city's draw 3 to 4 ms by day and 6 to 9 ms by night, when the wait in `eglSwapBuffers` is shorter by as much |
| Memory | 310 MB resident: 92.6 MB of vertices and indices and 48.2 MB of pictures on the GPU, and the driver's copies |

A late frame's parts go to the log in a development run. Of those read: the GPU over 14 ms; a wait of 19 to 29 ms in `eglSwapBuffers` with the GPU at 12 to 13 ms; a collection of the guest's heap (25.6 ms); a draw of the interface of 7 to 11 ms. While these ran the phone had 72 MB free, 270 MB in swap and 16 % of its time waiting for the disk.

## The pack

`bun tools/android.ts cook` (or `bun tools/tokyo.ts cook --profile redmi1s60`) lowers the city as for the Vita: the near level is the reference's own triangles by cell, the mid and far levels are prisms, the ground's pictures are 1 024² a block, every vertex has its normal, the indices are 16 bits from a batch's first vertex. What differs (`Target::Gles3` in `crates/tokyo-cook`):

- **Pictures are ETC2 blocks** (`flag::ETC2`): this GPU reads no S3TC. The ground and the facades' lights are the blocks ETC1 also has (`etc1.rs` encodes them); the facades by day carry their mask as EAC alpha before each block.
- **A landmark is a model of its own** (`flag::LANDMARKS`, the `LAND` table), drawn at the level of detail of its distance and in no cell's batches: the reference's own triangles with the lamps within 450 m (20 024 triangles for Tokyo Tower), the frame without its bracing to 1 100 m (2 440), the outline beyond (1 272). The Vita draws the reference's triangles at every distance.

The pack is 145.7 MB: 77 MB of vertices, 17 MB of indices, 36 MB of ground pictures.

## The launch

The Pocket3D title card plays first, before the guest starts and before the pack is read: 144 ticks, 2.4 s. The core draws each tick's frame into memory (`tk_card`, over `pocket3d_title::draw`, the 624 × 192 art on the plum ground) and the shell uploads it to a texture, which is deleted when the card ends. The card follows the clock, so a slow frame skips a tick and the card keeps its length. `bun tools/android.ts title` launches the app and brings back the held frame as the app drew it.

The guest boots next (`tokyo.js`, `tokyo.pak`), so the step "Reading the city" is the interface's own screen; the pack is then read a step a frame (2 MB of geometry, or a picture), with the loading screen drawn between two steps. The package stores the pack as it is (`aapt -0 pack`) and the core reads it in place through the package's file descriptor.

## The frame

1. `tk_step`: what the interface asked for since the last frame (`start`, `tour`, `hour`, `drive`, `look`, …), then one tick of the flight and the traffic per display refresh.
2. The guest's turn, on the frames `tk_guest_due` says it is worth one: the contacts go in (PocketJS's contact latch, the panel's pixels turned to the interface's 640 × 360), commands are left for the next `tk_step`.
3. `tk_draw`, straight into the window's buffer, which is cleared first so that no tile is read back: the walls, painted geometry and the landmark, the ground and roofs, the cars, and the sky last, over what nothing else covered.
4. The interface over it (`ui_gl_render_over`), then `eglSwapBuffers`.

The programs:

- **Ground and roofs**: picture × (ambient + sun × lit) by day, picture × (ambient + moon + the lamps' light) by night. The fragment stage finds the height grid from the picture's own coordinates and reads there the shadow's height, which it compares with the point's own, or the lamps' light. Six numbers cross a triangle by day, five by night. The lamps' light comes on over two seconds when the night's set starts to draw, and goes out the same way.
- **Walls**: facade × (1 + tint × mask) × light, plus what the rooms emit from the first of the dusk. The shadow's height is read in the vertex stage, a little way out from the wall, and what crosses the triangle is the point's height over it: a shadow's edge still crosses a wall at its own height. Nine numbers by day and by night, ten through the dusk.
- **Painted geometry, the landmark, cars**: lit, shadowed and hazed vertex by vertex. A colour crosses the triangle and the fragment stage writes it.
- **Shadows** are the Vita's: `tokyo_sim::shadow::sweep` on a thread whenever the sun has moved a tenth of a degree, into a texture of half floats over the 2 m height grid with two smaller levels under it. A finished sweep goes to the texture no queued frame reads, 96 rows a frame. By night (`NIGHT` in `core/src/lib.rs`: the sun's light is out and the moon's has begun) no program reads them and the sweeps rest.

That is eight programs: two for the ground, three for the walls, one each for painted geometry, the cars and the sky.

**The governor follows the GPU's timer.** The distances of the near and mid levels are 300 m and 1 300 m times a scale between 0.25 and 1. The scale comes in while the timer's mean over ten frames is above 13 ms, in faster when one frame is above 14 ms, and goes out while the mean is under 11 ms and the latest frame under 11.5 ms. A frame shown late brings it in only when the GPU was within 0.5 ms of 13 ms: a minute of the tour had as many late frames at a third of the distances, so most are the system's. The timer wraps the interface's draw too, so a list over the city brings the scale in by itself.

## Development loop

```
bun tools/android.ts doctor
bun tools/android.ts cook
bun tools/android.ts install          # the package: 147 MB; MIUI's two questions on the phone are answered by the tool
bun tools/android.ts native [--pack]  # replace the engine and the interface (and the pack) in the app's data: no install
bun tools/android.ts boot "samples=2" # words the next development launch reads: samples= width= height= title=0
bun tools/android.ts launch
bun tools/android.ts ctl "ui=fly hour=19 rate=0 view=707,325,924,0,150,0"
bun tools/android.ts ctl "touch=86,274;584,210"   # a thumb on the stick, one on UP
bun tools/android.ts capture --out a.png
bun tools/android.ts bench --seconds 150
bun tools/android.ts reset [--dev]    # stop the app and remove the settings it kept (and the development copies)
```

`native` puts `libtokyo-engine.so`, `tokyo.js`, `tokyo.pak` (and `city.pack`) into `files/dev` of the app's data through `run-as`, which the package allows (`android:debuggable`); the loader and the shell take a file from there before the package's own. `install` removes `files/dev`, so an installed package runs as installed.

`ctl` words go to the flow (`mode=title|flight|menu`, `ui=tour|fly|menu|resume|title`), to the flight (`tour= restart= at= hour= rate= budget= reach=a,b near= mid= sectors= view=x,y,z,tx,ty,tz[,fov] view=off fly=…`), to the renderer (`band=A,B haze= lamps= top= wall= solid= sky= shadow= unordered= tiny= cull= winding=`) and to the shell: `touch=X,Y[;X,Y]` holds fingers on the panel in the interface's pixels and `touch=off` lifts them, `tap=X,Y` is one finger for a few turns, `screen=1` writes the frame as drawn, `interface=0` leaves the interface out, `profile=1` waits for the GPU in every frame, `mark=SECONDS` measures.

The status (`files/status.json`, written twice a second by a thread of its own) carries the frames shown and how many refreshes went without a new one, the render thread's milliseconds by what it did, the GPU's own time for a frame (`gpuTimerMs`, from `EXT_disjoint_timer_query`: it leaves the frame's pace alone, where waiting for the GPU does not), the GPU's clock and the SoC's temperature. A development run writes each late frame to the log with what it was made of (`adb logcat -s PocketTokyo`).
