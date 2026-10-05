// The facades drawn flat: every kind of wall the facade material can show, laid out as columns of one picture
// that a device samples instead of running the material.
//
// A column holds `bays` window bays across and `floors` storeys up, with a margin of the neighbouring bays on
// either side. Storeys repeat upwards (the picture tiles in v); across, a wall longer than `bays` is cut by the
// compiler. The `mixed` and `commercial` columns start at the ground: their lowest row is a row of shops, and
// a building taller than `floors` storeys continues in a column that repeats.
//
// Two pictures come out, both sRGB:
//   day    rgb: the wall drawn with a grey of 0.5 (linear) as its own colour; alpha: how much of the texel is
//          wall (takes the building's colour) and not window (keeps its own)
//   night  rgb: half of what the windows and shop signs emit when every light is on
import * as THREE from 'three';
import { KIND, CAT, WALL } from '../world/constants.js';
import { shared } from '../world/materials.js';

const FLOOR = 3.2;
export const FACADE = {
  width: 4096, height: 512, column: 512, bays: 8, floors: 8, margin: 0.25, floorHeight: FLOOR,
  // the grey the walls were drawn with, as stored: a device multiplies a wall texel by colour / wallGrey
  wallGrey: 0.7354,
  nightScale: 2,
  columns: [
    { name: 'house', cat: CAT.HOUSE, layer: WALL.SIDING, bay: 3.4, height: 64, row: 6, seed: 0.3 },
    { name: 'apartment', cat: CAT.APARTMENT, layer: WALL.TILE, bay: 3.3, height: 64, row: 6, seed: 0.3 },
    // a whole low building: a row of shops under seven storeys (the compiler does not repeat these two upwards)
    { name: 'mixed', cat: CAT.MIXED, layer: WALL.TILE, bay: 3.2, height: 64, row: 0, seed: 0.3 },
    { name: 'commercial', cat: CAT.COMMERCIAL, layer: WALL.CONCRETE, bay: 3.0, height: 64, row: 0, seed: 0.4 },
    { name: 'office', cat: CAT.COMMERCIAL, layer: WALL.CONCRETE, bay: 3.0, height: 64, row: 6, seed: 0.4 },
    { name: 'glass', cat: CAT.GLASS, layer: WALL.CONCRETE, bay: 1.5, height: 64, row: 6, seed: 0.4 },
    // (a seed whose tower is not one of those lit in a single colour)
    { name: 'tower', cat: CAT.GLASS, layer: WALL.CONCRETE, bay: 1.5, height: 160, row: 20, seed: 0.39 },
    { name: 'plain', cat: CAT.COMMERCIAL, layer: WALL.CONCRETE, bay: 3.0, height: 64, row: 6, seed: 0.3, plain: true },
  ],
};

const quantize = (seed) => Math.round(seed * 4096) / 4096;

// A wall in the plane z = 0 facing +z: bays -2 .. bays + 2 across, storeys r0 .. r1 up.
function wall(c, r0, r1, seed, grey) {
  const u0 = -2, u1 = FACADE.bays + 2, x0 = u0 * c.bay, x1 = u1 * c.bay, y0 = r0 * FLOOR, y1 = r1 * FLOOR;
  const corners = [[x0, y0, u0], [x1, y0, u1], [x1, y1, u1], [x0, y0, u0], [x1, y1, u1], [x0, y1, u0]];
  const g = new THREE.BufferGeometry(), n = corners.length;
  const position = new Float32Array(n * 3), normal = new Float32Array(n * 3), color = new Float32Array(n * 3).fill(grey);
  const aFacade = new Float32Array(n * 4), aBldg = new Float32Array(n * 4), aPhoto = new Float32Array(n * 2).fill(-1);
  corners.forEach(([x, y, u], i) => {
    position.set([x, y, 0], i * 3); normal.set([0, 0, 1], i * 3);
    aFacade.set([c.plain ? 0 : u, y, FLOOR, quantize(seed)], i * 4);
    aBldg.set([c.height, c.cat + 8 * c.layer, c.plain ? KIND.SOLID : KIND.WALL, c.plain ? 0 : c.bay], i * 4);
  });
  for (const [name, a, k] of [['position', position, 3], ['normal', normal, 3], ['color', color, 3], ['aFacade', aFacade, 4], ['aBldg', aBldg, 4], ['aPhoto', aPhoto, 2]]) g.setAttribute(name, new THREE.BufferAttribute(a, k));
  return g;
}

const toLinear = new Float32Array(256).map((_, i) => { const c = i / 255; return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4; });
const toSrgb = (c) => Math.round(255 * (c <= 0.0031308 ? c * 12.92 : 1.055 * Math.min(1, c) ** (1 / 2.4) - 0.055));

export function bakeFacades(renderer, material, readTarget) {
  const S = 2, W = FACADE.width * S, H = FACADE.height * S, col = FACADE.column * S;
  const target = new THREE.WebGLRenderTarget(W, H, { samples: 4 });
  target.texture.colorSpace = THREE.SRGBColorSpace;
  const camera = new THREE.OrthographicCamera(0, 1, 1, 0, 1, 12000);
  camera.position.set(0, 0, 6000);
  const lit = new THREE.Scene(), dark = new THREE.Scene(), ambient = new THREE.AmbientLight(0xffffff, Math.PI);
  lit.add(ambient);

  // One pass over every column: `grey` is the walls' own colour, `scene` decides whether anything but emitted light shows.
  const pass = (scene, grey, night) => {
    shared.uNight.value = night;
    target.scissorTest = false;
    renderer.setRenderTarget(target);
    renderer.setClearColor(0x000000, 1);
    renderer.clear();
    target.scissorTest = true; // (each column clears and draws its own rectangle)
    FACADE.columns.forEach((c, ci) => {
      const r0 = c.row, r1 = c.row + FACADE.floors;
      const geo = wall(c, r0 - 2, r1 + 2, c.seed, grey);
      const mesh = new THREE.Mesh(geo, material);
      scene.add(mesh);
      camera.left = -FACADE.margin * c.bay; camera.right = (FACADE.bays + FACADE.margin) * c.bay;
      camera.bottom = r0 * FLOOR; camera.top = r1 * FLOOR;
      camera.updateProjectionMatrix();
      target.viewport.set(ci * col, 0, col, H); target.scissor.set(ci * col, 0, col, H);
      renderer.setRenderTarget(target);
      renderer.render(scene, camera);
      scene.remove(mesh);
      geo.dispose();
    });
    target.scissorTest = false;
    target.viewport.set(0, 0, W, H); target.scissor.set(0, 0, W, H);
    renderer.setRenderTarget(null);
    return readTarget(target, S);
  };

  const a = pass(lit, 0.5, 0), b = pass(lit, 0, 0);
  const e0 = pass(dark, 0, 0), e1 = pass(dark, 0, 0.5);
  shared.uNight.value = 0;
  target.dispose();
  const n = FACADE.width * FACADE.height, day = new Uint8Array(n * 4), night = new Uint8Array(n * 4);
  const lum = (px, o) => 0.2126 * toLinear[px[o]] + 0.7152 * toLinear[px[o + 1]] + 0.0722 * toLinear[px[o + 2]];
  for (let i = 0; i < n; i++) {
    const o = i * 4, la = lum(a, o), lb = lum(b, o);
    day[o] = a[o]; day[o + 1] = a[o + 1]; day[o + 2] = a[o + 2];
    day[o + 3] = la > 1e-4 ? Math.round(255 * Math.max(0, Math.min(1, 1 - lb / la))) : 0;
    // e1 holds half the night's light and half of the daylight seen through the glass; e0 holds all of the latter
    for (let c = 0; c < 3; c++) night[o + c] = toSrgb(Math.max(0, toLinear[e1[o + c]] - 0.5 * toLinear[e0[o + c]]));
    night[o + 3] = 255;
  }
  return { day, night };
}
