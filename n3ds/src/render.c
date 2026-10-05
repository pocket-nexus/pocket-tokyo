/* The PICA200 renderer.
 *
 * A frame: the sky's dome, then the city in one pass (the depth buffer has 24
 * bits): the ground and the roofs, the walls, what is painted, and the
 * landmarks' members, which are seen from both sides.
 *
 * The PICA200 has vertex programs and a fixed chain of combiners after them.
 * What the Vita's fragment programs do is laid out over the three texture
 * units:
 *
 * - The ground and the roofs: the picture from above (ETC1), the lamps'
 *   light and the shadows. The last two are one texture each over the whole
 *   city, on the grid of the pack's heights; a thread writes the shadows'
 *   when the sun has moved. The combiners compute
 *   (light x shadow + lamps x night) x picture.
 * - Walls: the facade pictures by day and what their windows emit:
 *   light x tint x day + night x emitted. A wall's light is picked in its
 *   vertex program by the sector it faces.
 * - Haze is the fog table.
 */
#include "render.h"

#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "sky_shbin.h"
#include "solid_shbin.h"
#include "top_shbin.h"
#include "wall_shbin.h"

typedef struct {
  DVLB_s *dvlb;
  shaderProgram_s program;
  C3D_AttrInfo attr;
  int mvp, a, b, c;
} Program;

static Program top_prog, wall_prog, solid_prog, sky_prog;
static const RenderData *d;
static C3D_Tex *block_tex, facade_tex[2], lamp_tex, shadow_tex, cell_tex[TK_SLOTS];
static uint8_t *slot_mem[TK_SLOTS];
static C3D_FogLut fog_lut;
static TkSkyVertex *sky_vb;
static uint16_t *sky_ib;
RenderStats render_stats;

static const unsigned vertex_bytes[TK_KINDS] = {8, 16, 12, 12};
/* The eye keeps 30 m from what stands around it; a near plane this far out leaves the depth buffer's values
 * (and with them the haze table's steps) to the city. */
#define NEAR_PLANE 12.0f
#define FAR_PLANE 12000.0f

static bool program_init(Program *p, const u8 *shbin, u32 size, const char *a, const char *b, const char *c) {
  p->dvlb = DVLB_ParseFile((u32 *)shbin, size);
  if (!p->dvlb)
    return false;
  shaderProgramInit(&p->program);
  shaderProgramSetVsh(&p->program, &p->dvlb->DVLE[0]);
  p->mvp = shaderInstanceGetUniformLocation(p->program.vertexShader, "mvp");
  p->a = a ? shaderInstanceGetUniformLocation(p->program.vertexShader, a) : -1;
  p->b = b ? shaderInstanceGetUniformLocation(p->program.vertexShader, b) : -1;
  p->c = c ? shaderInstanceGetUniformLocation(p->program.vertexShader, c) : -1;
  AttrInfo_Init(&p->attr);
  return p->mvp >= 0 && (!a || p->a >= 0) && (!b || p->b >= 0) && (!c || p->c >= 0);
}

static void use(Program *p) {
  C3D_BindProgram(&p->program);
  C3D_SetAttrInfo(&p->attr);
}

static void buffer(const void *data, unsigned stride, int count, u64 permutation) {
  C3D_BufInfo *b = C3D_GetBufInfo();
  BufInfo_Init(b);
  BufInfo_Add(b, data, stride, count, permutation);
}

static u32 rgb(const float c[3], float scale) {
  u32 v = 0;
  for (int i = 0; i < 3; i++) {
    float x = c[i] * scale;
    x = x < 0 ? 0 : x > 1 ? 1 : x;
    v |= (u32)(x * 255.0f + 0.5f) << (8 * i);
  }
  return v | 0xff000000;
}

static u32 level_bytes(GPU_TEXCOLOR format, unsigned w, unsigned h) { return format == GPU_ETC1 ? w * h / 2 : format == GPU_L8 ? w * h : w * h * 2; }

/* A texture from levels stored largest first. `vram`: in video memory, which the pack's own copy then leaves. */
static bool texture(C3D_Tex *t, const uint8_t *src, unsigned w, unsigned h, unsigned levels, GPU_TEXCOLOR format, bool vram) {
  C3D_TexInitParams params = {.width = w, .height = h, .maxLevel = levels - 1, .format = format, .type = GPU_TEX_2D, .onVram = vram};
  if (!C3D_TexInitWithParams(t, NULL, params)) {
    /* Video memory is 6 MB: the rest goes beside the vertices. */
    params.onVram = false;
    if (!C3D_TexInitWithParams(t, NULL, params))
      return false;
  }
  for (unsigned l = 0; l < levels && src; l++) {
    C3D_TexLoadImage(t, src, GPU_TEXFACE_2D, l);
    src += level_bytes(format, w >> l, h >> l);
  }
  C3D_TexSetFilter(t, GPU_LINEAR, GPU_LINEAR);
  C3D_TexSetFilterMipmap(t, GPU_NEAREST);
  C3D_TexSetWrap(t, GPU_CLAMP_TO_EDGE, GPU_CLAMP_TO_EDGE);
  return true;
}

bool render_init(const RenderData *data, char *error, size_t n) {
  d = data;
  if (!program_init(&top_prog, top_shbin, top_shbin_size, "pic", "map", "light") || !program_init(&wall_prog, wall_shbin, wall_shbin_size, "scale", "lights", NULL) ||
      !program_init(&solid_prog, solid_shbin, solid_shbin_size, "scale", "lights", NULL) || !program_init(&sky_prog, sky_shbin, sky_shbin_size, NULL, NULL, NULL)) {
    snprintf(error, n, "a vertex program did not load");
    return false;
  }
  AttrInfo_AddLoader(&top_prog.attr, 0, GPU_SHORT, 3);
  AttrInfo_AddLoader(&top_prog.attr, 1, GPU_UNSIGNED_BYTE, 2);
  AttrInfo_AddLoader(&wall_prog.attr, 0, GPU_SHORT, 3);
  AttrInfo_AddLoader(&wall_prog.attr, 1, GPU_UNSIGNED_BYTE, 2);
  AttrInfo_AddLoader(&wall_prog.attr, 2, GPU_UNSIGNED_BYTE, 4);
  AttrInfo_AddLoader(&wall_prog.attr, 3, GPU_SHORT, 2);
  AttrInfo_AddLoader(&solid_prog.attr, 0, GPU_SHORT, 3);
  AttrInfo_AddLoader(&solid_prog.attr, 1, GPU_UNSIGNED_BYTE, 2);
  AttrInfo_AddLoader(&solid_prog.attr, 2, GPU_UNSIGNED_BYTE, 4);
  AttrInfo_AddLoader(&sky_prog.attr, 0, GPU_FLOAT, 3);
  AttrInfo_AddLoader(&sky_prog.attr, 1, GPU_UNSIGNED_BYTE, 4);

  /* Every texture lies beside the vertices, in linear memory: a thread writes some of them in place. */
  block_tex = calloc(d->block_count, sizeof *block_tex);
  for (unsigned b = 0; b < d->block_count; b++) {
    const TkPicture *p = &d->pictures[b];
    if (!block_tex || !texture(&block_tex[b], d->texels + p->offset, p->width, p->height, p->levels, GPU_ETC1, false)) {
      snprintf(error, n, "no memory for the picture of block %u", b);
      return false;
    }
  }
  const TkPicture *f = &d->pictures[d->block_count];
  u32 pair = 0;
  for (unsigned l = 0; l < f->levels; l++)
    pair += level_bytes(GPU_RGB565, f->width >> l, f->height >> l);
  for (int k = 0; k < 2; k++) {
    if (!texture(&facade_tex[k], d->texels + f->offset + k * pair, f->width, f->height, f->levels, GPU_RGB565, false)) {
      snprintf(error, n, "no memory for the facades");
      return false;
    }
    C3D_TexSetWrap(&facade_tex[k], GPU_CLAMP_TO_EDGE, GPU_REPEAT);
  }
  if (!texture(&lamp_tex, d->lamps, MAP_SIDE, MAP_SIDE, 1, GPU_RGB565, false) || !texture(&shadow_tex, NULL, MAP_SIDE, MAP_SIDE, 1, GPU_L8, false)) {
    snprintf(error, n, "no memory for the lamps' light and the shadows");
    return false;
  }
  /* Until the first sweep: everything in the sun. */
  memset(shadow_tex.data, 255, MAP_SIDE * MAP_SIDE);
  GSPGPU_FlushDataCache(shadow_tex.data, MAP_SIDE * MAP_SIDE);
  for (unsigned s = 0; s < TK_SLOTS; s++) {
    slot_mem[s] = linearAlloc(d->slot_bytes);
    if (!slot_mem[s] || !texture(&cell_tex[s], NULL, 128, 128, 3, GPU_ETC1, false)) {
      snprintf(error, n, "no memory for cell slot %u", s);
      return false;
    }
  }
  sky_vb = linearAlloc(TK_DOME_VERTS * sizeof *sky_vb);
  sky_ib = linearAlloc(TK_DOME_INDICES * 2);
  if (!sky_vb || !sky_ib) {
    snprintf(error, n, "no memory for the sky");
    return false;
  }
  tk_sky_indices(sky_ib);
  GSPGPU_FlushDataCache(sky_ib, TK_DOME_INDICES * 2);
  /* Haze: what is left of a colour at a distance, exp(-distance x density), in a table of 128 steps over the
   * depth buffer's own values. Those crowd towards the eye: the first step alone reaches from 128 near planes
   * (1.5 km) to the far plane, and inside a step the table is followed in a straight line, which there is a
   * line in 1 / distance. Its far end is set for the kilometres in between rather than for the horizon. */
  float left[129], table[256];
  for (int i = 0; i <= 128; i++) {
    float distance = FAR_PLANE * NEAR_PLANE / ((float)i / 128.0f * (FAR_PLANE - NEAR_PLANE) + NEAR_PLANE);
    left[i] = i == 0 ? 0.2f : expf(-0.00022f * distance);
  }
  for (int i = 0; i < 128; i++) {
    table[i] = left[i];
    table[128 + i] = left[i + 1] - left[i];
  }
  FogLut_FromArray(&fog_lut, table);
  return true;
}

uint8_t *render_slot(unsigned slot) { return slot_mem[slot]; }

static u32 part(const TkNearCell *r, unsigned k) {
  u32 at = 0;
  for (unsigned i = 0; i < k; i++)
    at += (r->parts[i] + 15) & ~15u;
  return at;
}

void render_slot_arrived(unsigned slot, const TkNearCell *r) {
  const uint8_t *src = slot_mem[slot] + part(r, 4);
  C3D_Tex *t = &cell_tex[slot];
  for (unsigned l = 0; l < r->levels && l < 3; l++) {
    u32 size = 0;
    void *to = C3D_Tex2DGetImagePtr(t, l, &size);
    memcpy(to, src, size);
    GSPGPU_FlushDataCache(to, size);
    src += size;
  }
  GSPGPU_FlushDataCache(slot_mem[slot], part(r, 4));
}

uint8_t *render_shadows(void) { return shadow_tex.data; }

static void stage(int i, GPU_COMBINEFUNC f, GPU_TEVSRC a, GPU_TEVSRC b, GPU_TEVSRC c, u32 constant, bool doubled) {
  C3D_TexEnv *e = C3D_GetTexEnv(i);
  C3D_TexEnvInit(e);
  C3D_TexEnvSrc(e, C3D_RGB, a, b, c);
  C3D_TexEnvFunc(e, C3D_RGB, f);
  C3D_TexEnvColor(e, constant);
  if (doubled)
    C3D_TexEnvScale(e, C3D_RGB, GPU_TEVSCALE_2);
  /* The alpha of the stage before, or the vertex's at the first. */
  C3D_TexEnvSrc(e, C3D_Alpha, i == 0 ? GPU_PRIMARY_COLOR : GPU_PREVIOUS, 0, 0);
  C3D_TexEnvFunc(e, C3D_Alpha, GPU_REPLACE);
}

static void pass(int i) { C3D_TexEnvInit(C3D_GetTexEnv(i)); }

/* The place of an item's vertices: clip = vp x T(origin + span / 2) x S(span / 65535). */
static void place(Program *p, const C3D_Mtx *vp, const float origin[3], const float span[3]) {
  C3D_Mtx m = *vp;
  const float half = 32768.0f / 65535.0f;
  Mtx_Translate(&m, origin[0] + span[0] * half, origin[1] + span[1] * half, origin[2] + span[2] * half, true);
  Mtx_Scale(&m, span[0] / 65535.0f, span[1] / 65535.0f, span[2] / 65535.0f);
  C3D_FVUnifMtx4x4(GPU_VERTEX_SHADER, p->mvp, &m);
}

static void frame_of(const TkCity *c, u32 at, bool top, float origin[3], float span[3]) {
  float m = top ? 0.0f : c->margin;
  origin[0] = c->x0 + (float)(at % c->blocks_x) * c->block - m;
  origin[1] = c->y0;
  origin[2] = c->z0 + (float)(at / c->blocks_x) * c->block - m;
  span[0] = span[2] = c->block + 2.0f * m;
  span[1] = c->y_span;
}

static void lights(Program *p, const TkView *view) {
  C3D_FVec *l = C3D_FVUnifWritePtr(GPU_VERTEX_SHADER, p->b, TK_SECTORS + 1);
  for (int k = 0; k <= TK_SECTORS; k++)
    l[k] = FVec4_New(view->lights[k][0], view->lights[k][1], view->lights[k][2], 1.0f);
}

/* One kind's draws. */
static void draws(unsigned k, Program *p, const C3D_Mtx *vp, const TkItem *items, uint32_t count) {
  const TkCity *c = d->city;
  u32 placed = UINT32_MAX, bound = UINT32_MAX;
  const uint8_t *vertices = NULL;
  for (uint32_t i = 0; i < count; i++) {
    const TkItem *it = &items[i];
    const TkBatch *b = &d->batches[it->batch];
    const uint8_t *vtx;
    const uint16_t *idx;
    bool near = it->cell != UINT32_MAX;
    int slot = -1;
    if (near) {
      slot = tk_slot_of(it->cell);
      if (slot < 0)
        continue;
      const TkNearCell *r = &d->near[it->cell];
      vtx = slot_mem[slot] + part(r, k) + b->vtx_first * vertex_bytes[k];
      idx = (const uint16_t *)(slot_mem[slot] + part(r, 3)) + b->idx_first;
    } else {
      /* (a landmark's members are painted geometry: the solids' vertices) */
      vtx = d->vtx[k == TK_OPEN ? TK_SOLID : k] + b->vtx_first * vertex_bytes[k];
      idx = d->idx + b->idx_first;
    }
    float origin[3], span[3];
    if (placed != it->place) {
      placed = it->place;
      frame_of(c, it->place, k == TK_TOP, origin, span);
      place(p, vp, origin, span);
      if (k == TK_TOP) {
        /* The lamps' light and the shadows lie over the grid of heights, the last row first. */
        float side = MAP_SIDE * c->grid_step, a = span[0] / 65535.0f / side, half = 32768.0f / 65535.0f;
        C3D_FVUnifSet(GPU_VERTEX_SHADER, p->b, a, (origin[0] + span[0] * half - c->grid_x0) / side, -a, 1.0f - (origin[2] + span[2] * half - c->grid_z0) / side);
      }
    }
    if (k == TK_TOP) {
      /* A cell near the eye has a picture of its own; the others take their block's. */
      u32 want = near ? 0x80000000u | it->cell : it->place;
      if (bound != want) {
        bound = want;
        const float a = 1.0f / 65535.0f, half = 32768.0f / 65535.0f;
        if (near) {
          float n = (float)c->cells;
          u32 in = it->cell % (c->cells * c->cells);
          C3D_TexBind(0, &cell_tex[slot]);
          C3D_FVUnifSet(GPU_VERTEX_SHADER, p->a, a * n, half * n - (float)(in % c->cells), -a * n, 1.0f - half * n + (float)(in / c->cells));
        } else {
          C3D_TexBind(0, &block_tex[it->place]);
          C3D_FVUnifSet(GPU_VERTEX_SHADER, p->a, a, half, -a, 1.0f - half);
        }
      }
    }
    if (vertices != vtx) {
      vertices = vtx;
      if (k == TK_TOP)
        buffer(vtx, 8, 2, 0x10);
      else if (k == TK_SOLID || k == TK_OPEN)
        buffer(vtx, 12, 3, 0x210);
      else
        buffer(vtx, 16, 4, 0x3210);
    }
    C3D_DrawElements(GPU_TRIANGLES, it->to - it->from, C3D_UNSIGNED_SHORT, idx + it->from);
    render_stats.draws++;
    render_stats.tris[k] += (it->to - it->from) / 3;
  }
}

void render_frame(C3D_RenderTarget *target, const TkView *view, const TkItem *const lists[TK_KINDS], const uint32_t counts[TK_KINDS]) {
  memset(&render_stats, 0, sizeof render_stats);
  u32 haze = rgb(view->haze, 1.0f);
  C3D_RenderTargetClear(target, C3D_CLEAR_ALL, ((haze & 0xff) << 24) | ((haze & 0xff00) << 8) | ((haze & 0xff0000) >> 8) | 0xff, 0);
  C3D_FrameDrawOn(target);
  C3D_Mtx proj, look, vp;
  Mtx_PerspTilt(&proj, C3D_AngleFromDegrees(view->fov), C3D_AspectRatioTop, NEAR_PLANE, FAR_PLANE, false);
  Mtx_LookAt(&look, FVec3_New(view->eye[0], view->eye[1], view->eye[2]), FVec3_New(view->eye[0] + view->look[0], view->eye[1] + view->look[1], view->eye[2] + view->look[2]), FVec3_New(0, 1, 0), false);
  Mtx_Multiply(&vp, &proj, &look);
  C3D_DepthMap(true, -1.0f, 0.0f);
  C3D_AlphaTest(false, GPU_ALWAYS, 0);
  C3D_AlphaBlend(GPU_BLEND_ADD, GPU_BLEND_ADD, GPU_ONE, GPU_ZERO, GPU_ONE, GPU_ZERO);

  /* ---- the sky */
  tk_sky(sky_vb, 900.0f);
  GSPGPU_FlushDataCache(sky_vb, TK_DOME_VERTS * sizeof *sky_vb);
  C3D_Mtx sky = vp;
  Mtx_Translate(&sky, view->eye[0], view->eye[1], view->eye[2], true);
  use(&sky_prog);
  C3D_FVUnifMtx4x4(GPU_VERTEX_SHADER, sky_prog.mvp, &sky);
  buffer(sky_vb, sizeof *sky_vb, 2, 0x10);
  /* (a texture unit is never unbound: citro3d reads the texture it is handed. A stage that names no texture
   * leaves the units alone.) */
  stage(0, GPU_REPLACE, GPU_PRIMARY_COLOR, 0, 0, 0, false);
  pass(1);
  pass(2);
  C3D_FogGasMode(GPU_NO_FOG, GPU_PLAIN_DENSITY, false);
  C3D_CullFace(GPU_CULL_NONE);
  C3D_DepthTest(false, GPU_ALWAYS, GPU_WRITE_COLOR);
  C3D_DrawElements(GPU_TRIANGLES, TK_DOME_INDICES, C3D_UNSIGNED_SHORT, sky_ib);
  render_stats.draws++;

  /* ---- the city */
  C3D_DepthTest(true, GPU_GREATER, GPU_WRITE_ALL);
  C3D_CullFace(view->option & 1 ? GPU_CULL_NONE : view->option & 2 ? GPU_CULL_FRONT_CCW : GPU_CULL_BACK_CCW);
  if (!(view->option & 8)) {
    C3D_FogGasMode(GPU_FOG, GPU_PLAIN_DENSITY, false);
    C3D_FogColor(haze & 0xffffff);
    C3D_FogLutBind(&fog_lut);
  }
  float night = view->night;
  u32 lamps = rgb((float[3]){night, night, night}, 0.8f), windows = rgb((float[3]){night, night, night}, 1.0f);

  /* The ground and the roofs: (light x shadow + lamps x night) x picture, doubled. */
  use(&top_prog);
  C3D_FVUnifSet(GPU_VERTEX_SHADER, top_prog.c, view->top[0], view->top[1], view->top[2], 1.0f / 255.0f);
  C3D_TexBind(1, &lamp_tex);
  C3D_TexBind(2, &shadow_tex);
  stage(0, GPU_MODULATE, GPU_PRIMARY_COLOR, GPU_TEXTURE2, 0, 0, false);
  stage(1, GPU_MULTIPLY_ADD, GPU_TEXTURE1, GPU_CONSTANT, GPU_PREVIOUS, lamps, false);
  stage(2, GPU_MODULATE, GPU_PREVIOUS, GPU_TEXTURE0, 0, 0, true);
  if (view->option & 16) {
    stage(0, GPU_MODULATE, GPU_PRIMARY_COLOR, GPU_TEXTURE0, 0, 0, true);
    pass(1);
    pass(2);
  }
  if (!(view->option & 32))
    draws(TK_TOP, &top_prog, &vp, lists[TK_TOP], counts[TK_TOP]);

  /* Walls: light x tint x the facade by day, doubled, + night x what the windows emit. */
  use(&wall_prog);
  C3D_FVUnifSet(GPU_VERTEX_SHADER, wall_prog.a, 1.0f / 32767.0f, -32.0f / 32767.0f, 1.0f / 255.0f, 1.0f);
  lights(&wall_prog, view);
  C3D_TexBind(0, &facade_tex[0]);
  C3D_TexBind(1, &facade_tex[1]);
  stage(0, GPU_MODULATE, GPU_PRIMARY_COLOR, GPU_TEXTURE0, 0, 0, true);
  stage(1, GPU_MULTIPLY_ADD, GPU_TEXTURE1, GPU_CONSTANT, GPU_PREVIOUS, windows, false);
  pass(2);
  if (!(view->option & 64))
    draws(TK_WALL, &wall_prog, &vp, lists[TK_WALL], counts[TK_WALL]);

  /* What is painted. */
  use(&solid_prog);
  /* (x: how far the night has come for what shines by itself, at the half scale light travels at) */
  C3D_FVUnifSet(GPU_VERTEX_SHADER, solid_prog.a, 0.5f * night, 0.0f, 1.0f / 255.0f, 1.0f);
  lights(&solid_prog, view);
  stage(0, GPU_REPLACE, GPU_PRIMARY_COLOR, 0, 0, 0, true);
  pass(1);
  if (!(view->option & 128))
    draws(TK_SOLID, &solid_prog, &vp, lists[TK_SOLID], counts[TK_SOLID]);

  /* The landmarks' members: seen from both sides. */
  C3D_CullFace(GPU_CULL_NONE);
  if (!(view->option & 256))
    draws(TK_OPEN, &solid_prog, &vp, lists[TK_OPEN], counts[TK_OPEN]);
}
