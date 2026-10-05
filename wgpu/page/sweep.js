// The shadows, swept beside the frames: a worker of the page that runs the
// renderer's module again with the city's heights alone (`Sweeps` in
// ../src/web.rs). The page's frames ask for a sweep when the sun has moved a
// degree and read its texels when they arrive.
//
//   in   { city: Uint8Array, heights: Uint8Array }      once: the pack's CITY record and its HMAP
//   out  { ready: true }
//   in   Float32Array [x, y, z, shade]                  towards the sun, and what a shadow leaves of the light
//   out  { texels: Uint8Array, ms }                     1 024 squared texels of 8 bits, in rows
import init, { Sweeps } from "./pkg/tokyo_wgpu.js";

let sweeps;
self.onmessage = async ({ data }) => {
  if (!sweeps) {
    await init();
    sweeps = new Sweeps(data.city, data.heights);
    self.postMessage({ ready: true });
    return;
  }
  const from = performance.now();
  const texels = sweeps.sweep(data[0], data[1], data[2], data[3]);
  self.postMessage({ texels, ms: performance.now() - from }, [texels.buffer]);
};
