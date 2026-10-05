// Triangle counts of an area as the reference meshes it: bun web/scripts/stats.mjs [--area=shiba]
import fs from 'node:fs';
import path from 'node:path';
import { decodeTile } from '../src/shared/tileformat.js';
import { buildTile } from '../src/world/meshing.js';
import { makeSurface } from '../src/shared/decks.js';
import { KIND } from '../src/world/constants.js';

const area = (process.argv.find((a) => a.startsWith('--area=')) ?? '--area=shiba').split('=')[1];
const dir = path.resolve(import.meta.dirname, '../public/tiles', area);
const manifest = JSON.parse(fs.readFileSync(path.join(dir, 'manifest.json'), 'utf8'));
const t = manifest.terrain;
const grid = { x0: t.x0, z0: t.z0, step: t.step, w: t.w, h: t.h, data: new Float32Array(fs.readFileSync(path.join(dir, t.file)).buffer.slice(0)) };
const surface = makeSurface(grid, manifest.decks ?? []);
const sum = { terrain: 0, roads: 0, paint: 0, decals: 0, bldg: 0, photoRoof: 0, models: 0, props: 0 };
const kinds = [0, 0, 0, 0, 0];
const perTile = [];
let lod2 = 0, nb = 0;
const t0 = performance.now();
for (const tl of manifest.tiles) {
  const buf = fs.readFileSync(path.join(dir, tl.file));
  const tile = decodeTile(buf.buffer.slice(buf.byteOffset, buf.byteOffset + buf.byteLength));
  const m = buildTile(tile, grid, manifest.tileSize, surface);
  const c = {
    terrain: m.terrain.index.length / 3, roads: m.roads.position.length / 9, paint: m.paint.position.length / 9, decals: m.decals.position.length / 9,
    bldg: m.buildings.triangles, photoRoof: m.buildings.photo.position.length / 9, models: tl.mesh ? new DataView(fs.readFileSync(path.join(dir, tl.mesh)).buffer).getUint32(0, true) / 3 : 0, props: tile.props.length,
  };
  for (let i = 2; i < m.buildings.aBldg.length; i += 12) kinds[m.buildings.aBldg[i]]++;
  for (const b of tile.buildings) { nb++; if (b.surfaces.length) lod2++; }
  for (const k in c) sum[k] += c[k];
  perTile.push({ x: tl.x, z: tl.z, n: tile.buildings.length, ...c });
}
console.log(`meshed ${manifest.tiles.length} tiles in ${((performance.now() - t0) / 1000).toFixed(1)} s; ${nb} buildings, ${lod2} LOD2`);
console.log('triangles', Object.fromEntries(Object.entries(sum).map(([k, v]) => [k, Math.round(v)])));
console.log('building triangles by kind', Object.fromEntries(Object.entries(KIND).map(([k, v]) => [k, kinds[v]])));
perTile.sort((a, b) => b.bldg - a.bldg);
console.log('densest tiles', perTile.slice(0, 6).map((p) => `${p.x},${p.z}: ${p.n} bldg ${Math.round(p.bldg / 1000)}k+${Math.round(p.photoRoof / 1000)}k roof, roads ${Math.round(p.roads / 1000)}k, paint ${Math.round(p.paint / 1000)}k, models ${Math.round(p.models / 1000)}k`).join(' | '));
