# Pocket Tokyo

Tokyo, flown over on a PS Vita. The city is the real one: 14 753 buildings of the Shiba district around Tokyo Tower, from Japan's open 3D city model (Project PLATEAU), OpenStreetMap and the national elevation survey. The sun crosses the sky, the shadows turn with it, the windows and the street lamps come on at dusk, and the traffic runs on the real road graph. **960 × 544, 4× multisampling, 60 frames per second.**

This repository is private until its owner says otherwise. Compiled city data keeps the terms of its sources ([Attribution](#attribution)).

| | Screen | Renderer | Measured |
| --- | --- | --- | --- |
| PS Vita | 960 × 544, 4× MSAA, shadows that follow the clock, night glow | GXM, programs compiled on the device | 150 s of the tour from 15:36 to 20:04, with the traffic: 9 010 frames, **0 late**, worst frame 16.9 ms, 90 000 to 198 000 triangles a frame (mean 162 000), 225 draws |

The city is modelled once as Three.js content that runs in a browser, and a compiler lowers it to what one console draws:

- **`web/`** is the model: [Procedural Tokyo](https://github.com/jeantimex/tokyo) by Yong Su (MIT), with its pipeline from public records to tiles (`web/tools/pipeline`) and its three.js client. `web/src/pocket/` adds the export page.
- **`crates/tokyo-cook`** is the city compiler: CityIR in, one pack and a compile receipt out, for a device profile (`profiles/vita60.json`).
- **`crates/tokyo-pack`** is the pack: tables, vertex layouts and sections shared by the compiler and the runtimes.
- **`crates/tokyo-sim`** is what moves, the same on every device: the camera and its tour, the clock and the sun, the sweep that turns heights into shadows, the traffic.
- **`vita/`** draws the pack.

PocketJS (pinned in `vendor/pocketjs`) supplies the device toolchains, the dev host, the GXM kernel and packaging.

## From records to a frame

```
bun tools/tokyo.ts fetch   --area shiba     # PLATEAU, OpenStreetMap, GSI → web/data/raw (3 GB)
bun tools/tokyo.ts tiles   --area shiba     # → web/public/tiles/shiba (the reference's own compiler)
bun tools/tokyo.ts export  --area shiba     # the city as the reference draws it → .pocket-build/city/shiba/ir (CityIR)
bun tools/tokyo.ts cook    --area shiba     # CityIR → .pocket-build/city/shiba/vita60/city.pack + receipt.json
bun tools/tokyo.ts native                   # build, and replace the binary Pocket Devkit runs
bun tools/tokyo.ts bench                    # frame timings over the tour → .pocket-build/validation/vita/…
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
- **The level of detail follows a triangle budget** (200 000): the near and the mid distance shrink when a frame draws more and grow back slowly.
- **The traffic** is 800 cars on 6 777 lanes; the ones in view are built into one draw each frame.
- **The Pocket3D title card** plays first.

Measured on the console (clocks at 444 / 222 MHz):

| | |
| --- | --- |
| Scene pass | 3.0 ms + 0.053 ms per 1 000 triangles |
| A draw call | 2 µs of GPU time, 4 µs of CPU time |
| GXM's parameter buffer at its default 16 MB | overflows near 150 000 triangles in view; the frame then takes three times as long. `vita/build.rs` raises it to 32 MB |
| Shadow lookup | 1.1 ms a frame with its own texture coordinate, 2.9 ms read from the `.zw` of a shared one |
| Off-screen scene, bloom chain and composite | 3.75 ms; drawing into the display surface removes it |

## Controls

Left stick flies, right stick looks, L and R go down and up, ✕ flies faster. Left and right on the pad turn the clock; up and down set how fast it runs. START returns to the tour; SELECT shows the frame counters.

## Attribution

3D city model, roof photos, bridges and street furniture: Project PLATEAU (MLIT Japan). Elevation and aerial photos: Geospatial Information Authority of Japan (GSI). Road network, railways and places: © OpenStreetMap contributors (ODbL); a compiled area is a derived database under the ODbL. Textures: Poly Haven (CC0). Trees: ez-tree (MIT). The model in `web/` is [Procedural Tokyo](https://github.com/jeantimex/tokyo) by Yong Su, under the MIT License (`web/LICENSE`).

## License

The code is under the [MIT License](LICENSE); the code in `web/` keeps its own notice. The licence covers the code only: the city data keeps the terms of its sources listed above. Pocket3D's title card and device kernels come from PocketJS under their own licence.
