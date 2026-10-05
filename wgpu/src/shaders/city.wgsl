// The programs of a frame: the iPod touch's (ipod/src/render.c), one for one.
//
// - The ground and the roofs: the picture from above, the lamps' light and
//   the shadows. (light x shadow + lamps x night) x picture. By day the
//   program reads no lamps and by night no shadows.
// - Walls: the facade pictures by day and what their windows emit:
//   light x tint x day + rooms x emitted. A wall's light is picked in its
//   vertex stage by the sector it faces; each building's rooms come on at
//   their own moment of the dusk.
// - Painted geometry: a colour, lit by the sector its face looks to.
// - The sky: a dome coloured at its vertices.
//
// Light travels at half scale, as on the handhelds, and a fragment doubles
// it. Colours are the display's own values from the pack to the screen:
// nothing here converts them.

struct Frame {
  // x: haze per metre; y: the most a point takes of it
  haze: vec4<f32>,
  // rgb: the haze's colour; a: how far the lamps are on
  fog: vec4<f32>,
  // rgb: the light on what looks up, halved; w: 1 / 255
  top: vec4<f32>,
  // Walls. x, y: texture coordinates per unit; z: 1 / 255; w: how far the night has come
  wall: vec4<f32>,
  // Painted geometry. z: 1 / 255; w: how far a lamp shines by itself, at the half scale light travels at
  solid: vec4<f32>,
  // The light on a wall that looks to each sector, and last on painted faces that look up or down.
  lights: array<vec4<f32>, 17>,
}

// What changes from one place's draws to the next.
struct Place {
  // clip = mvp x the vertex as the pack stores it
  mvp: mat4x4<f32>,
  // The picture's coordinates from the vertex: u = x * pic.x + pic.y, v = z * pic.z + pic.w
  pic: vec4<f32>,
  // The same for the lamps' light and the shadows, which lie over the grid of heights.
  map: vec4<f32>,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var over_city: sampler;
@group(0) @binding(2) var lamps: texture_2d<f32>;
@group(0) @binding(3) var shadows: texture_2d<f32>;
@group(1) @binding(0) var<uniform> place: Place;
// The ground's picture from above, or the facades by day.
@group(2) @binding(0) var picture: texture_2d<f32>;
@group(2) @binding(1) var picture_sampler: sampler;
// What the facades' windows emit.
@group(2) @binding(2) var emitted: texture_2d<f32>;

// 1 - exp(-x), near enough.
fn haze_at(w: f32) -> f32 {
  let x = w * frame.haze.x;
  return min(1.0 - 1.0 / (1.0 + x + 0.5 * x * x), frame.haze.y);
}

// ---- the ground and the roofs

struct TopOut {
  @builtin(position) position: vec4<f32>,
  @location(0) pic: vec2<f32>,
  @location(1) map: vec2<f32>,
  @location(2) light: vec3<f32>,
  @location(3) haze: f32,
}

// A vertex is its place over a block and how much of the sky it sees (the
// third of its second four bytes).
@vertex
fn top_vertex(@location(0) at: vec4<i32>, @location(1) ao: vec4<u32>) -> TopOut {
  var out: TopOut;
  let p = vec3<f32>(at.xyz);
  out.position = place.mvp * vec4<f32>(p, 1.0);
  out.pic = p.xz * place.pic.xz + place.pic.yw;
  out.map = p.xz * place.map.xz + place.map.yw;
  out.light = frame.top.rgb * (f32(ao.z) * frame.top.w);
  out.haze = haze_at(out.position.w);
  return out;
}

fn top(in: TopOut, light: vec3<f32>) -> vec4<f32> {
  let c = textureSample(picture, picture_sampler, in.pic).rgb * light;
  return vec4<f32>(mix(c + c, frame.fog.rgb, in.haze), 1.0);
}

@fragment
fn top_day(in: TopOut) -> @location(0) vec4<f32> {
  return top(in, in.light * textureSample(shadows, over_city, in.map).r);
}

@fragment
fn top_dusk(in: TopOut) -> @location(0) vec4<f32> {
  return top(in, in.light * textureSample(shadows, over_city, in.map).r + textureSample(lamps, over_city, in.map).rgb * frame.fog.a);
}

@fragment
fn top_night(in: TopOut) -> @location(0) vec4<f32> {
  return top(in, in.light + textureSample(lamps, over_city, in.map).rgb * frame.fog.a);
}

// ---- walls

struct WallOut {
  @builtin(position) position: vec4<f32>,
  @location(0) uv: vec2<f32>,
  @location(1) light: vec3<f32>,
  @location(2) haze: f32,
  @location(3) rooms: f32,
}

// ao.z: what the face sees of the sky. ao.w: the sector it looks to in five
// bits, and over them three that say how late in the dusk the building's
// rooms come on; the colour's fourth byte is how bright they are then.
@vertex
fn wall_vertex(@location(0) at: vec4<i32>, @location(1) ao: vec4<u32>, @location(2) colour: vec4<u32>, @location(3) uv: vec2<i32>) -> WallOut {
  var out: WallOut;
  let s = frame.wall;
  out.position = place.mvp * vec4<f32>(vec3<f32>(at.xyz), 1.0);
  out.uv = vec2<f32>(uv) * s.xy;
  let late = f32(ao.w >> 5u);
  out.light = frame.lights[min(ao.w & 31u, 16u)].rgb * vec3<f32>(colour.rgb) * (f32(ao.z) * s.z * s.z);
  out.rooms = clamp((s.w - late * 0.075) * 4.0, 0.0, 1.0) * f32(colour.a) * s.z;
  out.haze = haze_at(out.position.w);
  return out;
}

@fragment
fn wall_day(in: WallOut) -> @location(0) vec4<f32> {
  let c = textureSample(picture, picture_sampler, in.uv).rgb * in.light;
  return vec4<f32>(mix(c + c, frame.fog.rgb, in.haze), 1.0);
}

@fragment
fn wall_night(in: WallOut) -> @location(0) vec4<f32> {
  let c = textureSample(picture, picture_sampler, in.uv).rgb * in.light;
  let lit = c + c + textureSample(emitted, picture_sampler, in.uv).rgb * in.rooms;
  return vec4<f32>(mix(lit, frame.fog.rgb, in.haze), 1.0);
}

// ---- painted geometry

struct SolidOut {
  @builtin(position) position: vec4<f32>,
  @location(0) light: vec3<f32>,
  @location(1) haze: f32,
}

// (alpha 255 marks a lamp: it shines by its own colour once the night has come)
@vertex
fn solid_vertex(@location(0) at: vec4<i32>, @location(1) ao: vec4<u32>, @location(2) colour: vec4<u32>) -> SolidOut {
  var out: SolidOut;
  let s = frame.solid;
  out.position = place.mvp * vec4<f32>(vec3<f32>(at.xyz), 1.0);
  let lamp = step(254.5, f32(colour.a));
  out.light = vec3<f32>(colour.rgb) * (frame.lights[min(ao.w, 16u)].rgb * (f32(ao.z) * s.z * s.z) + lamp * s.w * s.z);
  out.haze = haze_at(out.position.w);
  return out;
}

@fragment
fn solid(in: SolidOut) -> @location(0) vec4<f32> {
  return vec4<f32>(mix(in.light + in.light, frame.fog.rgb, in.haze), 1.0);
}

// ---- the sky

struct SkyOut {
  @builtin(position) position: vec4<f32>,
  @location(0) colour: vec3<f32>,
}

@vertex
fn sky_vertex(@location(0) at: vec3<f32>, @location(1) colour: vec4<f32>) -> SkyOut {
  var out: SkyOut;
  out.position = place.mvp * vec4<f32>(at, 1.0);
  out.colour = colour.rgb;
  return out;
}

@fragment
fn sky(in: SkyOut) -> @location(0) vec4<f32> {
  return vec4<f32>(in.colour, 1.0);
}
