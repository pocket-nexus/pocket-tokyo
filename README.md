# Pocket Tokyo

Tokyo, flown over on a PS Vita, a PSP and a Nintendo 3DS. The city is the real one: 14 753 buildings of the Shiba district around Tokyo Tower, from Japan's open 3D city model (Project PLATEAU), OpenStreetMap and the national elevation survey. The sun crosses the sky, the shadows turn with it, the windows and the street lamps come on at dusk, and on the Vita the traffic runs on the real road graph. **960 × 544 with 4× multisampling at 60 frames per second on the Vita; 30 frames per second on the PSP and the 3DS.**

This repository is private until its owner says otherwise. Compiled city data keeps the terms of its sources ([Attribution](#attribution)).

| | Screen | Renderer | Measured |
| --- | --- | --- | --- |
| PS Vita | 960 × 544, 4× MSAA, shadows that follow the clock, night glow | GXM, programs compiled on the device | 150 s of the tour from 15:36 to 20:04, with the traffic: 9 010 frames, **0 late**, worst frame 16.9 ms, 90 000 to 198 000 triangles a frame (mean 162 000), 225 draws |
| PSP | 480 × 272, shadows that follow the clock | GE, fixed function, one display list a frame | 150 s of the tour (PSPLINK, 333 MHz): 4 500 frames, **0 late**, worst frame 35.5 ms, 22 400 to 43 900 triangles a frame (mean 37 500), 1 253 draws |
| Nintendo 3DS | 400 × 240 on the upper screen, shadows that follow the clock; the clock and the frame in numbers on the lower one | PICA200: four vertex programs, three combiner stages | Old 3DS, one view of the build before the landmark models: 30 frames a second, 44 000 triangles, 289 draws, 10.7 ms of CPU, 18.6 ms of GPU. The tour is not benched on the console yet |

The city is modelled once as Three.js content that runs in a browser, and a compiler lowers it to what one console draws:

- **`web/`** is the model: [Procedural Tokyo](https://github.com/jeantimex/tokyo) by Yong Su (MIT), with its pipeline from public records to tiles (`web/tools/pipeline`) and its three.js client. `web/src/pocket/` adds the export page.
- **`crates/tokyo-cook`** is the city compiler: CityIR in, one pack and a compile receipt out, for a device profile (`profiles/vita60.json`, `psp30.json`, `n3ds30.json`).
- **`crates/tokyo-pack`** is the pack: tables, vertex layouts and sections shared by the compiler and the runtimes.
- **`crates/tokyo-sim`** is what moves, the same on every device: the camera and its tour, the clock and the sun, the sweep that turns heights into shadows, the traffic; and what a frame draws: the cells, blocks and regions in view at their levels of detail.
- **`vita/`**, **`psp/`** and **`n3ds/`** draw a pack.

PocketJS (pinned in `vendor/pocketjs`) supplies the device toolchains, the dev host, the GXM kernel and packaging. It also supplies what every Pocket3D game shows: the title card at launch and **the app icon in the console's launcher** (`vendor/pocketjs/engine/pocket3d/icon/`: 144 × 80 for the XMB, 128 × 128 for the Vita's bubble, 48 × 48 and 24 × 24 for the 3DS). This repository holds no icon file; `psp/assets/pic1.png` and the Vita's LiveArea pictures are captures of this game.

## From records to a frame

```
bun tools/tokyo.ts fetch   --area shiba     # PLATEAU, OpenStreetMap, GSI → web/data/raw (3 GB)
bun tools/tokyo.ts tiles   --area shiba     # → web/public/tiles/shiba (the reference's own compiler)
bun tools/tokyo.ts export  --area shiba     # the city as the reference draws it → .pocket-build/city/shiba/ir (CityIR)
bun tools/tokyo.ts cook    --area shiba     # CityIR → .pocket-build/city/shiba/vita60/city.pack + receipt.json
bun tools/tokyo.ts native                   # build, and replace the binary Pocket Devkit runs
bun tools/tokyo.ts bench                    # frame timings over the tour → .pocket-build/validation/vita/…

bun tools/tokyo.ts cook --profile psp30     # the PSP's pack
bun tools/psp.ts run                        # build, stage on the PSPLINK share, start
bun tools/psp.ts bench                      # → .pocket-build/validation/psp/…
bun tools/psp.ts emu                        # the same program in PPSSPPHeadless: a frame and its status

bun tools/tokyo.ts cook --profile n3ds30    # the 3DS's pack
bun tools/n3ds.ts install                   # build the .3dsx (the pack in its ROMFS), send it over the paired wire, start
bun tools/n3ds.ts bench
bun tools/n3ds.ts emu                       # the same .3dsx in Azahar
```

The export takes 28 s and the compile 3 to 6 s for the 180 tiles of Shiba; `bun run dev` serves the reference itself.

## CityIR: the export runs the reference

`web/export.html` loads the reference's mesher and materials in headless Chrome and hands the compiler what they produce (`tools/export.ts` receives it). Nothing of the reference's look is written a second time.

- **Per tile, the geometry**: the triangles `buildTile` makes for the buildings, with the attributes its facade material reads (window bay, height above the base, storey height, category), the footprints, PLATEAU's own models, the props.
- **Per tile, a picture from above** (`top_x_z.png`, 0.25 m a texel): the ground, the road paint, every roof under its aerial photo, tree crowns and parked cars, drawn by the reference's own materials under a light that leaves each surface its own colour, through a camera that looks straight down. A second, small picture (`far_x_z.png`) also shows the elevated roads and railways.
- **The facade material, drawn flat** (`facade.day.png`, `facade.night.png`): eight kinds of wall, each eight bays by eight storeys. The day picture is the wall in a neutral grey with a mask of what is wall and what is window; the night picture is what the rooms and the shop signs emit.

## The compiler

A device draws three kinds of surface, each with one program.

- **Tops**: the ground and every roof. Their texture is the picture from above and their texture coordinates are their own x and z, so a vertex is 12 bytes and the 11 million triangles of road surface the reference drapes over the terrain are no triangles at all: they are in the picture.
- **Walls**: the facade pictures, tinted by the building's colour. A wall is cut only where it is longer than the picture's eight bays; storeys repeat upwards.
- **Solids**: painted geometry, for the lattice tower, structures and trees.

The city is cut three ways, and each cut is a level of detail:

| Level | Place | Geometry |
| --- | --- | --- |
| near | a cell, 128 m | the reference's own triangles: PLATEAU's roof shapes and wall planes, structures within 15 cm, a crown on every tree |
| mid | a block, 512 m, with one picture of 1 024² | **prisms** |
| far | a region, 1 024 m, with one picture of 512² | prisms from a coarser grid, of buildings over 14 m; structures are in the picture |

**Prisms.** Every roof of the area is drawn into one grid of heights, 1 m a cell. Cells next to each other within 4 m of height form a terrace, whoever's roof they are; a terrace's outline is straightened and becomes walls under the facade pictures and a flat top under the picture from above. Two houses of one height with a shared wall become one prism. The mid level has a quarter of the near level's triangles and the far level a twentieth.

**Order in the index buffer is the culling structure.** A mid or far batch has its indices ordered by cell, and its walls within a cell by the sector of the compass they face (16 sectors). The pack stores where each group starts. A frame draws, of each cell in view, the ground and roofs, and of its walls only the arc of sectors that can face the eye: a third of the walls in view are never sent to the GPU.

**The city as heights.** The same raster, at 2 m, goes into the pack (`HMAP`), with the light of 9 128 street lamps as a second picture over the same grid (`LAMP`).

## The frame on the Vita

- **Shadows** come from `HMAP`. For the light's direction, `tokyo_sim::shadow::sweep` writes into each cell the height below which a point over it is in shadow: the cell's own top, or what the cell one step towards the light passes on, lowered by the light's slope. A thread of its own sweeps the 2 048 × 1 536 cells whenever the sun has moved a tenth of a degree (410 ms a sweep); the result is a 16-bit texture. Every fragment, of any program, compares its own height with one texel. The step towards the light lands between two cells, so a shadow's edge softens with its distance from what casts it.
- **The scene is drawn straight into the display's multisampled surface.** There is no off-screen target and no pass that copies the frame. At night a quarter-size chain takes the frame the display is showing, keeps what is bright and blurs it, and adds it over the next frame.
- **The level of detail follows a triangle budget** (200 000) and the GPU: the near and the mid distance shrink when a frame draws more, and when a frame has to wait more than 6 ms for the GPU to finish the frame before last; they grow back slowly, two seconds after such a wait. The GPU can run most of a refresh behind and still show every frame on time, and a view that fills more pixels than its triangles say (low over the park at dusk, 175 000 triangles) is what puts it there. While a list is up the budget is 150 000.
- **The traffic** is 800 cars on 6 777 lanes; the ones in view are built into one draw each frame.
- **The interface** (`ui/`, the `single` presentation) is drawn by PocketJS's Vita host library into the display scene, after the city and the night's glow: 480 × 272 logical, rastered at two samples a pixel. The guest turns 30 times a second and its list is drawn every frame. Its vertices come from vita2d's pool, which each frame takes one half of in turn, so a frame's draws stay valid until the GPU has used them. A tap on the panel reaches a list row through the guest's own hit test. The guest's collector walks its whole heap, 35 ms, and does not start by itself: it runs when the load ends and when the title comes back. The flight is `tokyo_sim::flight::Flight` and the flow around it `tokyo_interface::Session`, as on the handhelds; the bundle is read from the USB share in a development build and from the package otherwise, and what the interface asks to keep is `ux0:data/pocket-tokyo/interface.json`.
- **A clip is a pass over the whole screen's stencil here.** vita2d makes a clip of two stencil writes, in the multisampled scene the city is drawn in. The compass fades its marks at the ends of its window and the map's picture is transparent beside the map, so the flight's instruments use none; the panel of a list uses one, and the lower budget pays for it.
- **The interface's glyph pages and map take 15 MB of video memory** (vita2d keeps a texture at 32 bits a texel; the pages for 24 px and 36 px type are 1 024 texels square at two samples a pixel). That leaves the city 58 MB: 40 MB with ground pictures of 512 texels, the default, and 65 MB with 1 024 (`"ground": 1024` in `boot.json`), which no longer fit. Textures stay in video memory: with the two shadow targets in GPU-mapped main memory a night tour had 26 late frames in 30 s.
- **The Pocket3D title card** plays first.

Measured on the console (clocks at 444 / 222 MHz):

| | |
| --- | --- |
| Scene pass | 3.0 ms + 0.053 ms per 1 000 triangles |
| A draw call | 2 µs of GPU time, 4 µs of CPU time |
| GXM's parameter buffer at its default 16 MB | overflows near 150 000 triangles in view; the frame then takes three times as long. `vita/build.rs` raises it to 32 MB |
| Shadow lookup | 1.1 ms a frame with its own texture coordinate, 2.9 ms read from the `.zw` of a shared one |
| Off-screen scene, bloom chain and composite | 3.75 ms; drawing into the display surface removes it |
| The interface | 1.0 ms a turn (script, layout, two ticks of the UI core) at 30 turns a second; 0.75 ms of CPU a frame to draw its list; 35 ms a collection of its heap; a list built at its first opening is one turn of 50 to 80 ms |
| The tour with the interface | 150 s from 15:36 to 20:04: 9 000 frames, 0 late, worst frame 17.7 ms, 86 800 to 201 000 triangles (mean 159 000). Before it, with ground pictures of 512 texels as now: 9 010 frames, 0 late, worst 16.9 ms, mean 161 300 |
| The flight's instruments on the GPU | with them the tour's heaviest stretch at dusk had 6 late frames in 9 000 under the triangle budget alone; none with the governor that follows the GPU |
| One clip in the instruments | a night tour's first 30 s: 8 to 11 late frames with the compass clipped, 2 to 4 with its marks faded instead (both before the governor followed the GPU) |

## A pack for a handheld

The PSP has 24 MB and no programs; the 3DS has vertex programs, three texture units and a 32 MiB limit on what the wire installs. Their packs differ from the Vita's in what follows from that.

- **The near level is prisms too**, from a raster of 2 m, and each cell's near level is one record: its vertices, its indices and a picture of its own ground (0.5 m a texel on the PSP, 1 m on the 3DS). A device keeps the records of the cells before the eye in 40 slots; a thread reads them while the frame thread waits for the GPU. A cell that has not arrived is drawn at the mid level. The rest of the pack stays in memory: 10 MB on the PSP.
- **No vertex has a normal.** Walls and painted geometry are ordered by the sector of the compass they face (16), at every level. The PSP draws one sector per draw with the GE's ambient colour as its light; the 3DS keeps the sector in the vertex and picks one of 17 lights in its vertex program. A ground vertex is its position and what it sees of the sky: 8 bytes.
- **Day and night.** PSP: every picture is 8-bit indices with a day palette and a night palette, clustered over both colours of a texel at once; the palette the GE reads is their mix by the hour. 3DS: the ground in ETC1 by day, and the night as light: the lamps' light is one texture over the whole city, added by a combiner stage; the facades have a second picture of what their windows emit.
- **Shadows.** Both sweep the pack's heights (4 m a cell) for the sun's direction on a thread of low priority, when the sun has moved a degree. PSP: ground pictures keep to 128 colours, the upper half of each palette is the lower half in shadow, and the thread sets the top bit of the indices where the ground is in shadow. 3DS: the result is an 8-bit texture over the whole city, multiplied in by a combiner stage.

## Landmarks

A landmark is a building the reference builds member by member from a model of its own, where the city's data has only its outline; `web/src/world/tower.js` (a steel lattice tower) is one. A model reports what it is made of through `web/src/world/landmark.js`: beams with a rank (0 carries the outline, 1 the frame, 2 is bracing), boxes, lamps. The export records the members (`landmarks.cir`), and the compiler makes a handheld's model from them at three levels of detail, chosen by the landmark's distance alone:

| Level | Members | Tokyo Tower |
| --- | --- | --- |
| near, to 450 m | all 1 412: a beam is a ribbon of two triangles seen from both sides, an outline beam two ribbons crossed | 3 388 triangles |
| mid, to 1 100 m | the outline and the frame, no thinner than 1.6 m | 1 732 |
| far | the outline, no thinner than 3.2 m | 564 |

A model for another landmark reports its members the same way and needs nothing else in the compiler or on a device. The Vita draws the reference's own triangles of the tower.

## The frame on the PSP

- One display list a frame. The draws are chosen (`tokyo_sim::view::select`) while the GE draws the frame before; the list is written while the GE draws it.
- **A list costs 1.5 ms plus 0.63 ms per 1 000 triangles**, with or without lighting, fog, texture or clipping. At 30 frames a second that is 45 000 triangles; the budget is 42 000.
- Two ranges of the 16-bit depth buffer: the cells near the eye, then the mid and far levels. A landmark is drawn in the range its distance puts it in, or in both.
- Reading a cell (126 KiB at most) takes 8 ms over USB; a sweep of the shadows takes 320 ms and writing them into the pictures 1.7 s, both in the time the frame thread waits.

## The frame on the 3DS

- One pass: the depth buffer has 24 bits. The near plane is at 12 m: the haze is a table of 128 steps over the depth buffer's own values, and a nearer plane leaves the whole city to its first step.
- Ground: (light × shadow + lamps × night) × picture, in three combiner stages over three textures. Walls: light × tint × facade by day, plus night × what the windows emit.
- Every texture is in linear memory, and every vertex program writes all three texture coordinates: a unit that stays bound is read at them.

## Controls

Vita: left stick flies, right stick looks, L and R go down and up, ✕ flies faster. Left and right on the pad turn the clock; up and down set how fast it runs. START returns to the tour; SELECT shows the frame counters.

PSP: the stick flies ahead and turns, △ and ✕ look up and down, L and R go down and up, □ flies faster; the pad, START and SELECT as on the Vita. 3DS: the Circle Pad flies ahead and turns, X and B look up and down, L and R go down and up, Y flies faster; L + R + START leaves.

On the handhelds the eye keeps 30 m above what stands under it and around it. On the tour it starts to rise two seconds before a tower and comes down after it.

## Attribution

3D city model, roof photos, bridges and street furniture: Project PLATEAU (MLIT Japan). Elevation and aerial photos: Geospatial Information Authority of Japan (GSI). Road network, railways and places: © OpenStreetMap contributors (ODbL); a compiled area is a derived database under the ODbL. Textures: Poly Haven (CC0). Trees: ez-tree (MIT). The model in `web/` is [Procedural Tokyo](https://github.com/jeantimex/tokyo) by Yong Su, under the MIT License (`web/LICENSE`).

## License

The code is under the [MIT License](LICENSE); the code in `web/` keeps its own notice. The licence covers the code only: the city data keeps the terms of its sources listed above. Pocket3D's title card, app icon and device kernels come from PocketJS under their own licence.
