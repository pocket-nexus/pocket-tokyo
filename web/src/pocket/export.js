// CityIR export. The page runs the city's own mesher and materials and hands what they produce to the
// compiler (tools/export.ts receives it): per tile, the meshes as the client would draw them and a picture of
// the tile from straight above in its unlit colours (once more, small, with the structures that stand over the
// ground); for the whole area, the structures, the model library and the facades drawn flat, by day and by night.
//
//   export.html?area=shiba&sink=http://127.0.0.1:5291[&only=facades|tiles|global][&tiles=0_0,1_0][&top=1024]
import * as THREE from 'three';
import { decodeTile, PROP } from '../shared/tileformat.js';
import { sampleGrid } from '../shared/terrain.js';
import { makeSurface, makeCover } from '../shared/decks.js';
import { makeProjection } from '../shared/geo.js';
import { buildTile } from '../world/meshing.js';
import { KIND, CAT, WALL } from '../world/constants.js';
import { createMaterials, shared } from '../world/materials.js';
import { loadTextures } from '../world/textures.js';
import { loadOrtho } from '../world/ortho.js';
import { installLampLight } from '../world/lamplight.js';
import { Props, PROP_PLACEMENT } from '../world/props.js';
import { buildRailways } from '../world/rails.js';
import { buildFlyovers } from '../world/flyovers.js';
import { buildStructures } from '../world/structures.js';
import { parkedVehicles } from '../world/traffic.js';
import { Bundle } from './bundle.js';
import { FACADE, bakeFacades } from './facades.js';

installLampLight(); // (before any material is compiled)

const params = new URLSearchParams(location.search);
const AREA = params.get('area') || 'shiba';
const SINK = params.get('sink');
const ONLY = params.get('only');
const TOP = Number(params.get('top')) || 1024; // texels along a tile's picture from above
const base = `tiles/${AREA}`;

const logEl = document.getElementById('log');
const log = (...a) => { const line = a.join(' '); console.log(line); logEl.textContent = line + '\n' + logEl.textContent.slice(0, 4000); };
const put = async (name, body) => {
  if (!SINK) return;
  const res = await fetch(`${SINK}/${name}`, { method: 'PUT', body });
  if (!res.ok) throw new Error(`${name}: HTTP ${res.status}`);
};

const renderer = new THREE.WebGLRenderer({ antialias: false, preserveDrawingBuffer: true });
renderer.setSize(512, 512);
renderer.toneMapping = THREE.NoToneMapping;
document.body.appendChild(renderer.domElement);

// PNG of RGBA rows that start at the top.
async function png(pixels, w, h) {
  const canvas = new OffscreenCanvas(w, h), g = canvas.getContext('2d');
  g.putImageData(new ImageData(new Uint8ClampedArray(pixels.buffer, pixels.byteOffset, pixels.byteLength), w, h), 0, 0);
  return canvas.convertToBlob({ type: 'image/png' });
}

// What a render target holds, top row first, averaged down by `factor`.
function readTarget(target, factor = 1) {
  const W = target.width, H = target.height, raw = new Uint8Array(W * H * 4);
  renderer.readRenderTargetPixels(target, 0, 0, W, H, raw);
  const w = W / factor, h = H / factor, out = new Uint8Array(w * h * 4), n = factor * factor;
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) {
    let r = 0, g = 0, b = 0, a = 0;
    for (let j = 0; j < factor; j++) for (let i = 0; i < factor; i++) {
      const o = ((H - 1 - (y * factor + j)) * W + x * factor + i) * 4;
      r += raw[o]; g += raw[o + 1]; b += raw[o + 2]; a += raw[o + 3];
    }
    const o = (y * w + x) * 4;
    out[o] = Math.round(r / n); out[o + 1] = Math.round(g / n); out[o + 2] = Math.round(b / n); out[o + 3] = Math.round(a / n);
  }
  return out;
}

function geometry(arrays, attrs) {
  const g = new THREE.BufferGeometry();
  for (const [name, size] of attrs) if (arrays[name]?.length) g.setAttribute(name, new THREE.BufferAttribute(arrays[name], size));
  if (arrays.index) g.setIndex(new THREE.BufferAttribute(arrays.index, 1));
  return g;
}

// ---------------------------------------------------------------- the area
log(`area ${AREA}: textures`);
const textures = await loadTextures(renderer);
const materials = createMaterials(textures);
const manifest = await (await fetch(`${base}/manifest.json`)).json();
const t = manifest.terrain;
const grid = { x0: t.x0, z0: t.z0, step: t.step, w: t.w, h: t.h, data: new Float32Array(await (await fetch(`${base}/${t.file}`)).arrayBuffer()) };
const decks = manifest.decks ?? [];
const surface = makeSurface(grid, decks), cover = makeCover(grid, decks);
const ground = (x, z) => sampleGrid(grid, x, z);
const size = manifest.tileSize;
const proj = makeProjection(manifest.origin.lon, manifest.origin.lat);
shared.uNight.value = 0; shared.uDark.value = 0; shared.uGlintOn.value = 0; shared.uCityGlass.value = 0; shared.uMirrorOn.value = 0;

// ---------------------------------------------------------------- facades
if (!ONLY || ONLY === 'facades') {
  log('facades');
  const out = bakeFacades(renderer, materials.facade, readTarget);
  await put('facade.day.png', await png(out.day, FACADE.width, FACADE.height));
  await put('facade.night.png', await png(out.night, FACADE.width, FACADE.height));
  await put('facade.json', JSON.stringify(FACADE));
}

// ---------------------------------------------------------------- structures of the whole area
// Railways, elevated roads, footbridges and platforms: recorded for the compiler, and drawn into the far
// picture of each tile.
const structures = [];
if (!ONLY || ONLY === 'global' || ONLY === 'tiles') {
  log('railways, flyovers, structures');
  const railways = await buildRailways(`${base}/${manifest.rails}`, ground, cover);
  railways.userData.trains.group.removeFromParent(); // the trains move: the device has its own
  structures.push(['rails', railways], ['flyovers', await buildFlyovers(`${base}/${manifest.roads}`, ground)]);
  if (manifest.structures) structures.push(['structures', await buildStructures(`${base}/${manifest.structures}`, ground)]);
}
if (!ONLY || ONLY === 'global') {
  const bundle = new Bundle({ parts: [] });
  const dump = (name, root) => {
    root.updateMatrixWorld(true);
    root.traverse((o) => {
      if (!o.isMesh || o.isInstancedMesh || !o.geometry?.attributes.position) return;
      const g = o.geometry.index ? o.geometry.toNonIndexed() : o.geometry, id = `${name}.${bundle.meta.parts.length}`;
      const m = o.material, c = m.color ?? { r: 1, g: 1, b: 1 };
      bundle.meta.parts.push({ id, name, object: o.name, triangles: g.attributes.position.count / 3, color: [c.r, c.g, c.b], vertexColors: !!m.vertexColors, map: !!m.map, transparent: !!m.transparent, basic: !!m.isMeshBasicMaterial });
      bundle.add(`${id}.position`, g.attributes.position.array);
      if (g.attributes.color) bundle.add(`${id}.color`, g.attributes.color.array);
    });
  };
  for (const [name, root] of structures) dump(name, root);
  await put('global.cir', bundle.blob());
  // the height everything stands on, decks included, on the terrain's own grid
  const stand = new Float32Array(grid.w * grid.h);
  for (let j = 0; j < grid.h; j++) for (let i = 0; i < grid.w; i++) stand[j * grid.w + i] = surface(grid.x0 + i * grid.step, grid.z0 + j * grid.step);
  await put('surface.bin', stand);
}

// ---------------------------------------------------------------- tiles
// PLATEAU's own models of a tile, as the tile worker reads them (format: encodeMesh in tools/pipeline/meshes.mjs).
async function models(url) {
  const res = await fetch(url);
  if (!res.ok) return null;
  const buf = await res.arrayBuffer(), n = new DataView(buf).getUint32(0, true);
  return { position: new Float32Array(buf.slice(4, 4 + n * 12)), rgb: new Uint8Array(buf, 4 + n * 12, n * 3) };
}

// The triangles of `arrays` whose vertices pass `keep(vertex index)`.
function pick(arrays, sizes, keep) {
  const n = arrays.position.length / 3, map = [];
  for (let i = 0; i < n; i += 3) if (keep(i)) map.push(i, i + 1, i + 2);
  const out = {};
  for (const [name, k] of sizes) {
    const src = arrays[name], dst = new Float32Array(map.length * k);
    map.forEach((v, j) => { for (let c = 0; c < k; c++) dst[j * k + c] = src[v * k + c]; });
    out[name] = dst;
  }
  return out;
}

if (!ONLY || ONLY === 'tiles') {
  log('aerial photo');
  if (params.get('ortho') !== '0') await loadOrtho(`ortho/${AREA}`, proj, manifest.bounds, renderer);
  const props = new Props();
  const scene = new THREE.Scene();
  scene.add(new THREE.AmbientLight(0xffffff, Math.PI)); // a lit material then shows its own colour
  const target = new THREE.WebGLRenderTarget(TOP * 2, TOP * 2, { samples: 4, depthBuffer: true });
  target.texture.colorSpace = THREE.SRGBColorSpace;
  const FAR = 256, far = new THREE.WebGLRenderTarget(FAR * 2, FAR * 2, { samples: 4, depthBuffer: true });
  far.texture.colorSpace = THREE.SRGBColorSpace;
  const camera = new THREE.OrthographicCamera(0, 1, 1, 0, 1, 6000);
  const loadTexture = (file) => new Promise((resolve) => new THREE.TextureLoader().load(`${base}/${file}`, (map) => { map.colorSpace = THREE.SRGBColorSpace; map.anisotropy = 8; resolve(map); }, undefined, () => resolve(null)));
  const wanted = params.get('tiles')?.split(',');
  const list = manifest.tiles.filter((tl) => !wanted || wanted.includes(`${tl.x}_${tl.z}`));
  let done = 0;
  for (const tl of list) {
    const tile = decodeTile(await (await fetch(`${base}/${tl.file}`)).arrayBuffer());
    const mesh = buildTile(tile, grid, size, surface);
    const own = tl.mesh ? await models(`${base}/${tl.mesh}`) : null;
    const b = mesh.buildings;

    // -- the record
    const bundle = new Bundle({ x: tl.x, z: tl.z, size, buildings: tile.buildings.length, signs: mesh.signs, top: TOP });
    bundle.add('terrain.position', mesh.terrain.position).add('terrain.index', mesh.terrain.index);
    // water is drawn by the device; the rest of the ground is in the picture from above
    const water = [];
    for (let i = 0; i < mesh.roads.aLayer.length; i += 3) if (mesh.roads.aLayer[i] > 3.5) for (let k = 0; k < 9; k++) water.push(mesh.roads.position[i * 3 + k]);
    bundle.add('water.position', new Float32Array(water));
    bundle.add('bldg.position', b.position).add('bldg.normal', b.normal).add('bldg.color', b.color).add('bldg.facade', b.aFacade).add('bldg.kind', b.aBldg).add('bldg.ends', b.ends);
    bundle.add('roof.position', b.photo.position);
    if (own) bundle.add('models.position', own.position).add('models.rgb', own.rgb);
    // footprints, for the simple levels of detail
    const info = [], rings = [], points = [];
    tile.buildings.forEach((bd) => {
      info.push(bd.base, bd.height, bd.usage, bd.storeys, bd.flags, rings.length / 3, 0);
      bd.polygons.forEach((poly, p) => poly.forEach((ring, r) => { rings.push(points.length / 2, ring.length / 2, p * 2 + (r ? 1 : 0)); for (const v of ring) points.push(v); }));
      info[info.length - 1] = rings.length / 3 - info[info.length - 2];
    });
    bundle.add('foot.info', new Float32Array(info)).add('foot.rings', new Uint32Array(rings)).add('foot.points', new Float32Array(points));
    bundle.add('props', mesh.props).add('wires', mesh.wires);
    // the height everything stands on at each prop
    const at = new Float32Array(mesh.props.length / 6);
    for (let i = 0; i < at.length; i++) at[i] = surface(mesh.props[i * 6 + 3], mesh.props[i * 6 + 4]);
    bundle.add('props.ground', at);
    await put(`tile_${tl.x}_${tl.z}.cir`, bundle.blob());

    // -- the picture from above: the ground and every roof, without walls and without the lattice tower
    const group = new THREE.Group(), own3 = [];
    const add = (geo, material, order = 0) => { const m = new THREE.Mesh(geo, material); m.renderOrder = order; group.add(m); own3.push(geo); return m; };
    add(geometry(mesh.terrain, [['position', 3], ['normal', 3]]), materials.terrain);
    if (mesh.roads.position.length) add(geometry(mesh.roads, [['position', 3], ['normal', 3], ['color', 3], ['aLayer', 1]]), materials.road);
    if (mesh.paint.position.length) add(geometry(mesh.paint, [['position', 3], ['normal', 3], ['color', 3], ['aLayer', 1]]), materials.paint);
    if (mesh.decals.position.length) add(geometry(mesh.decals, [['position', 3], ['normal', 3], ['uv', 2]]), props.mats.decal, 2);
    if (b.position.length) {
      const tops = pick({ position: b.position, normal: b.normal, color: b.color, aFacade: b.aFacade, aBldg: b.aBldg, aPhoto: b.aPhoto },
        [['position', 3], ['normal', 3], ['color', 3], ['aFacade', 4], ['aBldg', 4], ['aPhoto', 2]], (i) => b.aBldg[i * 4 + 2] !== KIND.WALL && b.aBldg[i * 4 + 2] !== KIND.LATTICE);
      if (tops.position.length) add(geometry(tops, [['position', 3], ['normal', 3], ['color', 3], ['aFacade', 4], ['aBldg', 4], ['aPhoto', 2]]), materials.facade);
    }
    // Water in its own plain colour: the device has no mirror to show in it.
    if (water.length) {
      const lifted = Float32Array.from(water, (v, i) => (i % 3 === 1 ? v + 0.05 : v));
      const material = new THREE.MeshBasicMaterial({ color: new THREE.Color().setRGB(0.13, 0.25, 0.33, THREE.SRGBColorSpace) });
      add(geometry({ position: lifted }, [['position', 3]]), material);
      own3.push(material);
    }
    // Trees as seen from above: their crowns belong to the picture; near the eye the device stands a tree on each.
    const trees = mesh.props.length ? props.build(mesh.props, new Float32Array(0), surface) : null;
    if (trees) { trees.group.remove(trees.near); trees.far.visible = true; group.add(trees.group); }
    let atlas = null;
    if (b.photo.position.length) {
      atlas = tl.atlas ? await loadTexture(tl.atlas) : null;
      const material = new THREE.MeshStandardMaterial(atlas ? { map: atlas, roughness: 0.9, metalness: 0 } : { color: 0x777776, roughness: 0.9, metalness: 0 });
      add(geometry(b.photo, [['position', 3], ['normal', 3], ['uv', 2]]), material);
      own3.push(material);
    }
    scene.add(group);
    const x0 = tl.x * size, z0 = tl.z * size;
    camera.left = x0; camera.right = x0 + size; camera.top = -z0; camera.bottom = -(z0 + size);
    camera.position.set(0, 3000, 0); camera.up.set(0, 0, -1); camera.lookAt(0, 0, 0);
    camera.updateProjectionMatrix(); camera.updateMatrixWorld();
    renderer.setRenderTarget(target);
    renderer.setClearColor(0x6f6e68, 1);
    renderer.clear();
    renderer.render(scene, camera);
    renderer.setRenderTarget(null);
    await put(`top_${tl.x}_${tl.z}.png`, await png(readTarget(target, 2), TOP, TOP));
    // -- the same with what stands over the ground: the far level of detail draws these as part of the picture
    if (own) {
      const normal = new Float32Array(own.position.length), color = new Float32Array(own.position.length);
      const lin = (v) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
      for (let i = 0; i < color.length; i++) { color[i] = lin(own.rgb[i] / 255); normal[i] = i % 3 === 1 ? 1 : 0; }
      add(geometry({ position: own.position, normal, color }, [['position', 3], ['normal', 3], ['color', 3]]), materials.models);
    }
    for (const [, root] of structures) scene.add(root);
    renderer.setRenderTarget(far);
    renderer.clear();
    renderer.render(scene, camera);
    renderer.setRenderTarget(null);
    for (const [, root] of structures) scene.remove(root);
    await put(`far_${tl.x}_${tl.z}.png`, await png(readTarget(far, 2), FAR, FAR));
    scene.remove(group);
    trees?.group.traverse((o) => { if (o.isInstancedMesh) o.dispose(); });
    for (const o of own3) o.dispose();
    atlas?.dispose();
    done++;
    if (done % 10 === 0 || done === list.length) log(`tiles ${done}/${list.length}`);
    document.title = `export tiles ${done}/${list.length}`;
  }

  // -- the model library: what stands at a prop, by kind and variant
  log('models');
  const lib = new Bundle({ placement: PROP_PLACEMENT, trees: props.trees.map((tr) => ({ height: tr.height, radius: tr.radius })), models: [] });
  const model = (name, geo, color = null) => {
    const g = geo.index ? geo.toNonIndexed() : geo;
    lib.meta.models.push({ name, color });
    lib.add(`${name}.position`, g.attributes.position.array);
    if (g.attributes.color) lib.add(`${name}.color`, g.attributes.color.array);
    if (g.attributes.aGlow) lib.add(`${name}.glow`, g.attributes.aGlow.array);
  };
  model('pole.0', props.models.pole[0]); model('pole.1', props.models.pole[1]);
  model('light', props.models.light); model('signal', props.models.signal); model('vending', props.models.vending); model('blob', props.models.blob);
  for (const [kind, list2] of Object.entries(props.furniture)) list2.forEach((geo, v) => model(`prop.${kind}.${v}`, geo));
  props.trees.forEach((tr, i) => { model(`tree.${i}.branches`, tr.branches); model(`tree.${i}.leaves`, tr.leaves); });
  const kit = parkedVehicles();
  kit.models.forEach((geo, v) => model(`vehicle.${v}`, geo));
  lib.meta.vehicleColors = kit.colors;
  lib.meta.kinds = PROP;
  await put('models.cir', lib.blob());
}

await put('done', 'ok');
log('done');
document.title = 'export done';
