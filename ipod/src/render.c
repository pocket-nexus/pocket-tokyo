// The OpenGL ES 2 renderer: the 3DS's passes (n3ds/src/render.c) on the
// SGX535, from a pack lowered the same way with texels in row order
// (profiles/ipod60.json).
//
// A frame: the sky's dome, then the city in one pass with a 24-bit depth
// buffer: the ground and the roofs, the walls, what is painted, and the
// landmarks' members, which are seen from both sides. The shell gives it a
// target with four samples a pixel (main.c).
//
// What the 3DS lays out over three combiners is a fragment program here:
//
// - The ground and the roofs: the picture from above, the lamps' light and the
//   shadows. The last two are one texture each over the whole city, on the
//   grid of the pack's heights; a thread sweeps the shadows when the sun has
//   moved (main.c). (light x shadow + lamps x night) x picture. By day the
//   program reads no lamps and by night no shadows.
// - Walls: the facade pictures by day and what their windows emit:
//   light x tint x day + rooms x emitted. A wall's light is picked in its
//   vertex program by the sector it faces; each building's rooms come on at
//   their own moment of the dusk.
// - Haze: its share per vertex, as the Vita computes it.
//
// The drawable is the portrait screen: every matrix ends with a quarter turn,
// for a device held with its home button on the right.
#include "render.h"
#define GL_SILENCE_DEPRECATION 1
#include <OpenGL/gl3.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

// OpenGL ES names the desktop's core header leaves out.
#define LUMINANCE 0x1909
#define TEXTURE_MAX_ANISOTROPY 0x84FE
// The eye keeps 30 m from what stands around it.
#define NEAR_PLANE 3.0f
#define FAR_PLANE 12000.0f
// Haze per metre, and the most a point takes of it (the Vita's values).
#define HAZE 0.00022f
#define HAZE_MOST 0.94f

enum { TOP_DAY, TOP_DUSK, TOP_NIGHT, WALL_DAY, WALL_NIGHT, SOLID, SKY, PROGRAMS };
typedef struct {
  GLuint id;
  GLint mvp, haze, fog, pic, map, light, lights, scale;
} Program;

// One source for every program: defines pick the vertex layout and the hour.
static const char *const vertex_source =
  "uniform mat4 uMvp;\n"
  // x: haze per metre, y: the most a point takes
  "uniform vec2 uHaze;\n"
  "attribute vec3 aPos;\n"
  "varying lowp float vHaze;\n"
  "varying lowp vec3 vLight;\n"
  "#ifdef TOP\n"
  // A vertex is its place over a block and how much of the sky it sees. The
  // picture's coordinates, and those of the lamps' light and of the shadows,
  // come from the place: u = x * uPic.x + uPic.y, v = z * uPic.z + uPic.w.
  // (aAo is the vertex's second four bytes: the last two are its own)
  "attribute vec4 aAo;\n"
  "uniform vec4 uPic;\n"
  "uniform vec4 uMap;\n"
  // rgb: the light on what looks up, halved; w: 1 / 255
  "uniform vec4 uLight;\n"
  "varying highp vec2 vPic;\n"
  "varying highp vec2 vMap;\n"
  "#endif\n"
  "#if defined(WALL) || defined(SOLID)\n"
  // aAo.z: what the face sees of the sky; aAo.w: the sector it looks to, which picks its light.
  "attribute vec4 aAo;\n"
  "attribute vec4 aColor;\n"
  "uniform vec3 uLights[17];\n"
  // x, y: texture coordinates per unit; z: 1 / 255; w: how far the night has come
  "uniform vec4 uScale;\n"
  "#endif\n"
  "#ifdef WALL\n"
  "attribute vec2 aUv;\n"
  "varying highp vec2 vUv;\n"
  "varying lowp float vRooms;\n"
  "#endif\n"
  "#ifdef SKY\n"
  "attribute vec4 aColor;\n"
  "#endif\n"
  "void main() {\n"
  "  gl_Position = uMvp * vec4(aPos, 1.0);\n"
  "#ifdef TOP\n"
  "  vPic = aPos.xz * uPic.xz + uPic.yw;\n"
  "  vMap = aPos.xz * uMap.xz + uMap.yw;\n"
  "  vLight = uLight.rgb * (aAo.z * uLight.w);\n"
  "#endif\n"
  "#ifdef WALL\n"
  // Over the sector's five bits, three say how late in the dusk the building's
  // rooms come on; the colour's fourth byte is how bright they are then.
  "  vUv = aUv * uScale.xy;\n"
  "  float late = floor(aAo.w * 0.03125);\n"
  "  vLight = uLights[int(aAo.w - late * 32.0)] * aColor.rgb * (aAo.z * uScale.z * uScale.z);\n"
  "  vRooms = clamp((uScale.w - late * 0.075) * 4.0, 0.0, 1.0) * aColor.a * uScale.z;\n"
  "#endif\n"
  "#ifdef SOLID\n"
  // (alpha 255 marks a lamp: it shines by its own colour once the night has come)
  "  vLight = aColor.rgb * (uLights[int(aAo.w)] * (aAo.z * uScale.z * uScale.z) + step(254.5, aColor.a) * uScale.w * uScale.z);\n"
  "#endif\n"
  "#ifdef SKY\n"
  "  vLight = aColor.rgb;\n"
  "  vHaze = 0.0;\n"
  "#else\n"
  // 1 - exp(-x), near enough.
  "  float x = gl_Position.w * uHaze.x;\n"
  "  vHaze = min(1.0 - 1.0 / (1.0 + x + 0.5 * x * x), uHaze.y);\n"
  "#endif\n"
  "}\n";
// Every colour is computed in `lowp`: the SGX keeps such a vector in one
// register and works on it in one instruction, and a value carried from `lowp`
// to `mediump` and back costs an instruction each way. With the walls' second
// picture read in `mediump`, a night frame of 43 000 triangles took a refresh
// and a quarter. Light travels at half scale, as on the 3DS, and is doubled
// by an addition.
static const char *const fragment_source =
  "precision lowp float;\n"
  // rgb: the haze; a: how far the lamps are on
  "uniform lowp vec4 uFog;\n"
  "varying lowp float vHaze;\n"
  "varying lowp vec3 vLight;\n"
  "#ifdef TOP\n"
  "uniform sampler2D uTex0;\n" // the picture from above
  "uniform sampler2D uTex1;\n" // the lamps' light
  "uniform sampler2D uTex2;\n" // the shadows
  "varying highp vec2 vPic;\n"
  "varying highp vec2 vMap;\n"
  "void main() {\n"
  "#ifdef DAY\n"
  "  lowp vec3 light = vLight * texture2D(uTex2, vMap).r;\n"
  "#endif\n"
  "#ifdef DUSK\n"
  "  lowp vec3 light = vLight * texture2D(uTex2, vMap).r + texture2D(uTex1, vMap).rgb * uFog.a;\n"
  "#endif\n"
  "#ifdef NIGHT\n"
  "  lowp vec3 light = vLight + texture2D(uTex1, vMap).rgb * uFog.a;\n"
  "#endif\n"
  "  lowp vec3 c = texture2D(uTex0, vPic).rgb * light;\n"
  "  gl_FragColor = vec4(mix(c + c, uFog.rgb, vHaze), 1.0);\n"
  "}\n"
  "#endif\n"
  "#ifdef WALL\n"
  "uniform sampler2D uTex0;\n" // the facades by day
  "uniform sampler2D uTex1;\n" // what their windows emit
  "varying highp vec2 vUv;\n"
  "varying lowp float vRooms;\n"
  "void main() {\n"
  "  lowp vec3 c = texture2D(uTex0, vUv).rgb * vLight;\n"
  "#ifdef NIGHT\n"
  "  c = c + c + texture2D(uTex1, vUv).rgb * vRooms;\n"
  "#else\n"
  "  c += c;\n"
  "#endif\n"
  "  gl_FragColor = vec4(mix(c, uFog.rgb, vHaze), 1.0);\n"
  "}\n"
  "#endif\n"
  "#ifdef SOLID\n"
  "void main() { gl_FragColor = vec4(mix(vLight + vLight, uFog.rgb, vHaze), 1.0); }\n"
  "#endif\n"
  "#ifdef SKY\n"
  "void main() { gl_FragColor = vec4(vLight, 1.0); }\n"
  "#endif\n";

static Program programs[PROGRAMS];
static const RenderData *d;
// The levels that stay: top, wall and solid vertices, and their indices.
static GLuint city_vertices[3], city_indices;
static GLuint *block_pictures, facade[2], lamps;
// The shadows: the texture frames read, and the one the next sweep is uploaded into.
static GLuint shadows[2];
static unsigned shadows_shown;
// A slot: a cell's vertices (its three kinds one after another, as the record holds them), indices and picture.
static GLuint slot_vertices[TK_SLOTS], slot_indices[TK_SLOTS], slot_picture[TK_SLOTS];
static unsigned slot_width[TK_SLOTS];
static TkSkyVertex sky_vertices[TK_DOME_VERTS];
static uint16_t sky_indices[TK_DOME_INDICES];
static float sky_hour = -1.0f;
RenderStats render_stats;

static const unsigned vertex_bytes[TK_KINDS] = {8, 16, 12, 12};

static bool link_program(Program *p, const char *define, char *error, size_t capacity) {
  p->id = glCreateProgram();
  for (unsigned i = 0; i < 2; i++) {
    const char *source[2] = {define, i ? fragment_source : vertex_source};
    GLuint shader = glCreateShader(i ? GL_FRAGMENT_SHADER : GL_VERTEX_SHADER);
    glShaderSource(shader, 2, source, NULL);
    glCompileShader(shader);
    GLint ok = 0;
    glGetShaderiv(shader, GL_COMPILE_STATUS, &ok);
    if (!ok) {
      int at = snprintf(error, capacity, "%s shader%s", i ? "fragment" : "vertex", define);
      glGetShaderInfoLog(shader, (GLsizei)(capacity - at), NULL, error + at);
      return false;
    }
    glAttachShader(p->id, shader);
    glDeleteShader(shader);
  }
  glBindAttribLocation(p->id, 0, "aPos");
  glBindAttribLocation(p->id, 1, "aAo");
  glBindAttribLocation(p->id, 2, "aColor");
  glBindAttribLocation(p->id, 3, "aUv");
  glLinkProgram(p->id);
  GLint ok = 0;
  glGetProgramiv(p->id, GL_LINK_STATUS, &ok);
  if (!ok) {
    int at = snprintf(error, capacity, "program%s", define);
    glGetProgramInfoLog(p->id, (GLsizei)(capacity - at), NULL, error + at);
    return false;
  }
  p->mvp = glGetUniformLocation(p->id, "uMvp");
  p->haze = glGetUniformLocation(p->id, "uHaze");
  p->fog = glGetUniformLocation(p->id, "uFog");
  p->pic = glGetUniformLocation(p->id, "uPic");
  p->map = glGetUniformLocation(p->id, "uMap");
  p->light = glGetUniformLocation(p->id, "uLight");
  p->lights = glGetUniformLocation(p->id, "uLights");
  p->scale = glGetUniformLocation(p->id, "uScale");
  glUseProgram(p->id);
  static const char *const units[3] = {"uTex0", "uTex1", "uTex2"};
  for (int i = 0; i < 3; i++) {
    GLint at = glGetUniformLocation(p->id, units[i]);
    if (at >= 0)
      glUniform1i(at, i);
  }
  return true;
}

static GLuint buffer(GLenum target, const void *data, size_t size, GLenum usage) {
  GLuint id;
  glGenBuffers(1, &id);
  glBindBuffer(target, id);
  glBufferData(target, size, data, usage);
  return id;
}

// The levels of a 16-bit picture below `from` (w by h texels), down to one
// texel: each texel the mean of the four it covers. A texture with levels is
// read only when it has them all, and the pack stores the first few.
static void smaller_levels(unsigned level, const uint16_t *from, unsigned w, unsigned h) {
  uint16_t *turn[2] = {malloc((size_t)w * h / 2 + 2), malloc((size_t)w * h / 8 + 2)};
  for (unsigned t = 0; turn[0] && turn[1] && (w > 1 || h > 1); t ^= 1) {
    unsigned nw = w > 1 ? w / 2 : 1, nh = h > 1 ? h / 2 : 1, sx = w > 1, sy = h > 1;
    uint16_t *to = turn[t];
    for (unsigned y = 0; y < nh; y++)
      for (unsigned x = 0; x < nw; x++) {
        const unsigned p[4] = {from[y * 2 * w + x * 2], from[y * 2 * w + x * 2 + sx], from[(y * 2 + sy) * w + x * 2], from[(y * 2 + sy) * w + x * 2 + sx]};
        unsigned r = 0, g = 0, b = 0;
        for (int k = 0; k < 4; k++)
          r += p[k] >> 11, g += (p[k] >> 5) & 63, b += p[k] & 31;
        to[y * nw + x] = (uint16_t)(((r + 2) >> 2) << 11 | ((g + 2) >> 2) << 5 | ((b + 2) >> 2));
      }
    w = nw, h = nh;
    glTexImage2D(GL_TEXTURE_2D, ++level, GL_RGB, w, h, 0, GL_RGB, GL_UNSIGNED_SHORT_5_6_5, to);
    from = to;
  }
  free(turn[0]);
  free(turn[1]);
}

// A texture from 16-bit levels stored largest first, in row order.
static GLuint picture(const uint8_t *src, unsigned w, unsigned h, unsigned levels, GLenum wrap_s, GLenum wrap_t) {
  GLuint id;
  glGenTextures(1, &id);
  glBindTexture(GL_TEXTURE_2D, id);
  const uint8_t *last = src;
  for (unsigned l = 0; l < levels; l++) {
    glTexImage2D(GL_TEXTURE_2D, l, GL_RGB, w >> l, h >> l, 0, GL_RGB, GL_UNSIGNED_SHORT_5_6_5, src);
    last = src;
    src += (size_t)(w >> l) * (h >> l) * 2;
  }
  smaller_levels(levels - 1, (const uint16_t *)last, w >> (levels - 1), h >> (levels - 1));
  glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR_MIPMAP_NEAREST);
  glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
  glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, wrap_s);
  glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, wrap_t);
  return id;
}

bool render_init(const RenderData *data, char *error, size_t capacity) {
  d = data;
  static const char *const defines[PROGRAMS] = {"\n#define TOP\n#define DAY\n", "\n#define TOP\n#define DUSK\n", "\n#define TOP\n#define NIGHT\n", "\n#define WALL\n",
                                                "\n#define WALL\n#define NIGHT\n", "\n#define SOLID\n", "\n#define SKY\n"};
  for (unsigned i = 0; i < PROGRAMS; i++)
    if (!link_program(&programs[i], defines[i], error, capacity))
      return false;
  glPixelStorei(GL_UNPACK_ALIGNMENT, 1);
  for (int k = 0; k < 3; k++)
    city_vertices[k] = buffer(GL_ARRAY_BUFFER, d->vtx[k], d->vtx_bytes[k], GL_STATIC_DRAW);
  city_indices = buffer(GL_ELEMENT_ARRAY_BUFFER, d->idx, d->idx_bytes, GL_STATIC_DRAW);

  bool finer = strstr((const char *)glGetString(GL_EXTENSIONS), "GL_EXT_texture_filter_anisotropic") != NULL;
  block_pictures = calloc(d->block_count, sizeof *block_pictures);
  for (unsigned b = 0; b < d->block_count; b++) {
    const TkPicture *p = &d->pictures[b];
    block_pictures[b] = picture(d->texels + p->offset, p->width, p->height, p->levels, GL_CLAMP_TO_EDGE, GL_CLAMP_TO_EDGE);
    // The ground is seen at a slant: two samples along it keep a level finer.
    if (finer)
      glTexParameterf(GL_TEXTURE_2D, TEXTURE_MAX_ANISOTROPY, 2.0f);
  }
  const TkPicture *f = &d->pictures[d->block_count];
  size_t pair = 0;
  for (unsigned l = 0; l < f->levels; l++)
    pair += (size_t)(f->width >> l) * (f->height >> l) * 2;
  // A facade's picture runs once across a wall and repeats up it.
  for (int k = 0; k < 2; k++)
    facade[k] = picture(d->texels + f->offset + k * pair, f->width, f->height, f->levels, GL_CLAMP_TO_EDGE, GL_REPEAT);
  lamps = picture(d->lamps, MAP_SIDE, MAP_SIDE, 1, GL_CLAMP_TO_EDGE, GL_CLAMP_TO_EDGE);
  // Until the first sweep: everything in the sun.
  uint8_t *sun = malloc(MAP_SIDE * MAP_SIDE);
  if (!sun || !block_pictures) {
    snprintf(error, capacity, "no memory for the pictures");
    return false;
  }
  memset(sun, 255, MAP_SIDE * MAP_SIDE);
  glGenTextures(2, shadows);
  for (int k = 0; k < 2; k++) {
    glBindTexture(GL_TEXTURE_2D, shadows[k]);
    glTexImage2D(GL_TEXTURE_2D, 0, LUMINANCE, MAP_SIDE, MAP_SIDE, 0, LUMINANCE, GL_UNSIGNED_BYTE, sun);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
  }
  free(sun);

  glGenBuffers(TK_SLOTS, slot_vertices);
  glGenBuffers(TK_SLOTS, slot_indices);
  glGenTextures(TK_SLOTS, slot_picture);
  for (unsigned s = 0; s < TK_SLOTS; s++) {
    glBindTexture(GL_TEXTURE_2D, slot_picture[s]);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR_MIPMAP_NEAREST);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
    if (finer)
      glTexParameterf(GL_TEXTURE_2D, TEXTURE_MAX_ANISOTROPY, 2.0f);
  }
  tk_sky_indices(sky_indices);
  if (glGetError() != GL_NO_ERROR) {
    snprintf(error, capacity, "OpenGL refused an upload");
    return false;
  }
  return true;
}

// Where part `k` of a record starts: each part begins on a 16-byte boundary.
static uint32_t part(const TkNearCell *r, unsigned k) {
  uint32_t at = 0;
  for (unsigned i = 0; i < k; i++)
    at += (r->parts[i] + 15) & ~15u;
  return at;
}

void render_cell(unsigned slot, const TkNearCell *r, const uint8_t *bytes) {
  glBindBuffer(GL_ARRAY_BUFFER, slot_vertices[slot]);
  glBufferData(GL_ARRAY_BUFFER, part(r, 3), bytes, GL_DYNAMIC_DRAW);
  glBindBuffer(GL_ELEMENT_ARRAY_BUFFER, slot_indices[slot]);
  glBufferData(GL_ELEMENT_ARRAY_BUFFER, r->parts[3], bytes + part(r, 3), GL_DYNAMIC_DRAW);
  glActiveTexture(GL_TEXTURE0);
  glBindTexture(GL_TEXTURE_2D, slot_picture[slot]);
  const uint8_t *src = bytes + part(r, 4), *last = src;
  unsigned w = r->width;
  for (unsigned l = 0; l < r->levels; l++) {
    // (a slot's texture keeps its storage from one cell to the next)
    if (slot_width[slot] == w)
      glTexSubImage2D(GL_TEXTURE_2D, l, 0, 0, w >> l, w >> l, GL_RGB, GL_UNSIGNED_SHORT_5_6_5, src);
    else
      glTexImage2D(GL_TEXTURE_2D, l, GL_RGB, w >> l, w >> l, 0, GL_RGB, GL_UNSIGNED_SHORT_5_6_5, src);
    last = src;
    src += (size_t)(w >> l) * (w >> l) * 2;
  }
  smaller_levels(r->levels - 1, (const uint16_t *)last, w >> (r->levels - 1), w >> (r->levels - 1));
  slot_width[slot] = w;
}

// A sweep goes into the texture no frame reads, a strip a frame: writing into a texture that a frame still
// on its way to the screen reads makes the driver copy the whole of it first. `last`: frames read it from now.
void render_shadows(const uint8_t *texels, unsigned first, unsigned rows, bool last) {
  glActiveTexture(GL_TEXTURE2);
  glBindTexture(GL_TEXTURE_2D, shadows[shadows_shown ^ 1]);
  glTexSubImage2D(GL_TEXTURE_2D, 0, 0, first, MAP_SIDE, rows, LUMINANCE, GL_UNSIGNED_BYTE, texels + (size_t)first * MAP_SIDE);
  glActiveTexture(GL_TEXTURE0);
  if (last)
    shadows_shown ^= 1;
}

// ---- matrices: sixteen numbers, a column after a column

static void multiply(float *out, const float *a, const float *b) {
  for (int c = 0; c < 4; c++)
    for (int r = 0; r < 4; r++)
      out[c * 4 + r] = a[r] * b[c * 4] + a[4 + r] * b[c * 4 + 1] + a[8 + r] * b[c * 4 + 2] + a[12 + r] * b[c * 4 + 3];
}

// The place of an item's vertices: clip = vp x T(origin + span / 2) x S(span / 65535).
static void place(const Program *p, const float *vp, const float origin[3], const float span[3]) {
  const float half = 32768.0f / 65535.0f;
  float m[16], t[3], s[3];
  for (int i = 0; i < 3; i++) {
    t[i] = origin[i] + span[i] * half;
    s[i] = span[i] / 65535.0f;
  }
  for (int r = 0; r < 4; r++) {
    m[r] = vp[r] * s[0];
    m[4 + r] = vp[4 + r] * s[1];
    m[8 + r] = vp[8 + r] * s[2];
    m[12 + r] = vp[r] * t[0] + vp[4 + r] * t[1] + vp[8 + r] * t[2] + vp[12 + r];
  }
  glUniformMatrix4fv(p->mvp, 1, GL_FALSE, m);
}

static void frame_of(const TkCity *c, uint32_t at, bool top, float origin[3], float span[3]) {
  float m = top ? 0.0f : c->margin;
  origin[0] = c->x0 + (float)(at % c->blocks_x) * c->block - m;
  origin[1] = c->y0;
  origin[2] = c->z0 + (float)(at / c->blocks_x) * c->block - m;
  span[0] = span[2] = c->block + 2.0f * m;
  span[1] = c->y_span;
}

// One kind's draws.
static void draws(unsigned k, const Program *p, const float *vp, const TkItem *items, uint32_t count) {
  const TkCity *c = d->city;
  uint32_t placed = UINT32_MAX, bound = UINT32_MAX;
  GLuint vertices = 0, indices = 0;
  size_t first = SIZE_MAX;
  const unsigned stride = vertex_bytes[k];
  for (uint32_t i = 0; i < count; i++) {
    const TkItem *it = &items[i];
    const TkBatch *b = &d->batches[it->batch];
    bool near = it->cell != UINT32_MAX;
    int slot = -1;
    GLuint vbo, ibo;
    size_t at, index;
    if (near) {
      slot = tk_slot_of(it->cell);
      if (slot < 0)
        continue;
      vbo = slot_vertices[slot];
      ibo = slot_indices[slot];
      at = part(&d->near[it->cell], k) + (size_t)b->vtx_first * stride;
    } else {
      // (a landmark's members are painted geometry: the solids' vertices)
      vbo = city_vertices[k == TK_OPEN ? TK_SOLID : k];
      ibo = city_indices;
      at = (size_t)b->vtx_first * stride;
    }
    index = ((size_t)b->idx_first + it->from) * 2;
    if (placed != it->place) {
      placed = it->place;
      float origin[3], span[3];
      frame_of(c, it->place, k == TK_TOP, origin, span);
      place(p, vp, origin, span);
      if (k == TK_TOP) {
        // The lamps' light and the shadows lie over the grid of heights.
        float side = MAP_SIDE * c->grid_step, a = span[0] / 65535.0f / side, half = 32768.0f / 65535.0f;
        glUniform4f(p->map, a, (origin[0] + span[0] * half - c->grid_x0) / side, a, (origin[2] + span[2] * half - c->grid_z0) / side);
      }
    }
    if (k == TK_TOP) {
      // A cell near the eye has a picture of its own; the others take their block's.
      uint32_t want = near ? 0x80000000u | it->cell : it->place;
      if (bound != want) {
        bound = want;
        const float a = 1.0f / 65535.0f, half = 32768.0f / 65535.0f;
        if (near) {
          float n = (float)c->cells;
          uint32_t in = it->cell % (c->cells * c->cells);
          glBindTexture(GL_TEXTURE_2D, slot_picture[slot]);
          glUniform4f(p->pic, a * n, half * n - (float)(in % c->cells), a * n, half * n - (float)(in / c->cells));
        } else {
          glBindTexture(GL_TEXTURE_2D, block_pictures[it->place]);
          glUniform4f(p->pic, a, half, a, half);
        }
      }
    }
    if (vertices != vbo || first != at) {
      if (vertices != vbo)
        glBindBuffer(GL_ARRAY_BUFFER, vertices = vbo);
      first = at;
      const uint8_t *v = (const uint8_t *)at;
      glVertexAttribPointer(0, 3, GL_SHORT, GL_FALSE, stride, v);
      // (every attribute starts on a 4-byte boundary: the driver copies and converts, at each draw, vertices
      // whose attributes do not. The sky's share and the sector are read with the two bytes before them.)
      glVertexAttribPointer(1, 4, GL_UNSIGNED_BYTE, GL_FALSE, stride, v + 4);
      if (k != TK_TOP)
        glVertexAttribPointer(2, 4, GL_UNSIGNED_BYTE, GL_FALSE, stride, v + 8);
      if (k == TK_WALL)
        glVertexAttribPointer(3, 2, GL_SHORT, GL_FALSE, stride, v + 12);
    }
    if (indices != ibo)
      glBindBuffer(GL_ELEMENT_ARRAY_BUFFER, indices = ibo);
    glDrawElements(GL_TRIANGLES, it->to - it->from, GL_UNSIGNED_SHORT, (const void *)index);
    render_stats.draws++;
    render_stats.tris[k] += (it->to - it->from) / 3;
  }
}

static const Program *use(unsigned which, const TkView *view, float lamps_on) {
  const Program *p = &programs[which];
  glUseProgram(p->id);
  glUniform2f(p->haze, HAZE, view->option & 8 ? 0.0f : HAZE_MOST);
  glUniform4f(p->fog, view->haze[0], view->haze[1], view->haze[2], lamps_on);
  return p;
}

void render_frame(const TkView *view, const TkItem *const lists[TK_KINDS], const uint32_t counts[TK_KINDS]) {
  memset(&render_stats, 0, sizeof render_stats);
  // The frame is the landscape screen turned a quarter: the drawable's x is its y, the drawable's y its -x.
  static const float quarter[16] = {0, -1, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1};
  const float f = 1.0f / tanf(view->fov * (float)M_PI / 360.0f), near = NEAR_PLANE, far = FAR_PLANE;
  const float projection[16] = {f / ((float)WIDTH / HEIGHT), 0, 0, 0, 0, f, 0, 0, 0, 0, (far + near) / (near - far), -1, 0, 0, 2 * far * near / (near - far), 0};
  const float *e = view->eye;
  float l[3] = {view->look[0], view->look[1], view->look[2]}, length = sqrtf(l[0] * l[0] + l[1] * l[1] + l[2] * l[2]);
  for (int i = 0; i < 3; i++)
    l[i] /= length > 1e-6f ? length : 1.0f;
  float r[3] = {-l[2], 0, l[0]};
  length = sqrtf(r[0] * r[0] + r[2] * r[2]);
  if (length < 1e-6f)
    r[0] = 1, r[2] = 0, length = 1;
  r[0] /= length, r[2] /= length;
  const float u[3] = {r[1] * l[2] - r[2] * l[1], r[2] * l[0] - r[0] * l[2], r[0] * l[1] - r[1] * l[0]};
  const float eye[16] = {r[0], u[0], -l[0], 0, r[1], u[1], -l[1], 0, r[2], u[2], -l[2], 0,
                         -(r[0] * e[0] + r[1] * e[1] + r[2] * e[2]), -(u[0] * e[0] + u[1] * e[1] + u[2] * e[2]), l[0] * e[0] + l[1] * e[1] + l[2] * e[2], 1};
  float turned[16], vp[16];
  multiply(turned, quarter, projection);
  multiply(vp, turned, eye);

  glViewport(0, 0, HEIGHT, WIDTH);
  glDisable(GL_SCISSOR_TEST);
  glDisable(GL_BLEND);
  glDepthMask(GL_TRUE);
  glClearColor(view->haze[0], view->haze[1], view->haze[2], 1);
  glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT);
  glFrontFace(GL_CCW);

  // ---- the sky: a dome around the eye, coloured at its vertices for the hour
  if (fabsf(view->hour - sky_hour) > 0.004f) {
    sky_hour = view->hour;
    tk_sky(sky_vertices, 900.0f);
  }
  const float there[16] = {1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, e[0], e[1], e[2], 1};
  float sky[16];
  multiply(sky, vp, there);
  const Program *p = use(SKY, view, 0);
  glUniformMatrix4fv(p->mvp, 1, GL_FALSE, sky);
  glDisable(GL_DEPTH_TEST);
  glDisable(GL_CULL_FACE);
  glBindBuffer(GL_ARRAY_BUFFER, 0);
  glBindBuffer(GL_ELEMENT_ARRAY_BUFFER, 0);
  glEnableVertexAttribArray(0);
  glDisableVertexAttribArray(1);
  glEnableVertexAttribArray(2);
  glDisableVertexAttribArray(3);
  glVertexAttribPointer(0, 3, GL_FLOAT, GL_FALSE, sizeof *sky_vertices, sky_vertices->pos);
  glVertexAttribPointer(2, 4, GL_UNSIGNED_BYTE, GL_TRUE, sizeof *sky_vertices, sky_vertices->color);
  glDrawElements(GL_TRIANGLES, TK_DOME_INDICES, GL_UNSIGNED_SHORT, sky_indices);
  render_stats.draws++;

  // ---- the city
  glEnable(GL_DEPTH_TEST);
  glDepthFunc(GL_LESS);
  if (view->option & 1)
    glDisable(GL_CULL_FACE);
  else {
    glEnable(GL_CULL_FACE);
    glCullFace(view->option & 2 ? GL_FRONT : GL_BACK);
  }
  const float night = view->night;
  // By day no lamp is on; deep in the night no shadow is cast.
  unsigned top = night < 0.004f ? TOP_DAY : night > 0.97f ? TOP_NIGHT : TOP_DUSK;

  // The ground and the roofs: (light x shadow + lamps x night) x picture, doubled.
  glEnableVertexAttribArray(1);
  glDisableVertexAttribArray(2);
  p = use(top, view, 0.8f * night);
  glUniform4f(p->light, view->top[0], view->top[1], view->top[2], 1.0f / 255.0f);
  glActiveTexture(GL_TEXTURE1);
  glBindTexture(GL_TEXTURE_2D, lamps);
  glActiveTexture(GL_TEXTURE2);
  glBindTexture(GL_TEXTURE_2D, shadows[shadows_shown]);
  glActiveTexture(GL_TEXTURE0);
  if (!(view->option & 32))
    draws(TK_TOP, p, vp, lists[TK_TOP], counts[TK_TOP]);

  // Walls: light x tint x the facade by day, doubled, + what the rooms emit.
  glEnableVertexAttribArray(2);
  glEnableVertexAttribArray(3);
  p = use(night < 0.004f ? WALL_DAY : WALL_NIGHT, view, 0);
  // (FACADE_V repeats of the picture up a wall over the signed range)
  glUniform4f(p->scale, 1.0f / 32767.0f, 32.0f / 32767.0f, 1.0f / 255.0f, night);
  glUniform3fv(p->lights, TK_SECTORS + 1, view->lights[0]);
  glBindTexture(GL_TEXTURE_2D, facade[0]);
  glActiveTexture(GL_TEXTURE1);
  glBindTexture(GL_TEXTURE_2D, facade[1]);
  glActiveTexture(GL_TEXTURE0);
  if (!(view->option & 64))
    draws(TK_WALL, p, vp, lists[TK_WALL], counts[TK_WALL]);

  // What is painted.
  glDisableVertexAttribArray(3);
  p = use(SOLID, view, 0);
  // (w: how far a lamp shines by itself, at the half scale light travels at)
  glUniform4f(p->scale, 0, 0, 1.0f / 255.0f, 0.5f * night);
  glUniform3fv(p->lights, TK_SECTORS + 1, view->lights[0]);
  if (!(view->option & 128))
    draws(TK_SOLID, p, vp, lists[TK_SOLID], counts[TK_SOLID]);

  // The landmarks' members: seen from both sides.
  glDisable(GL_CULL_FACE);
  if (!(view->option & 256))
    draws(TK_OPEN, p, vp, lists[TK_OPEN], counts[TK_OPEN]);
  glBindBuffer(GL_ARRAY_BUFFER, 0);
  glBindBuffer(GL_ELEMENT_ARRAY_BUFFER, 0);
}
