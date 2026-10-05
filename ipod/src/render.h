// The OpenGL ES 2 renderer of the iPod touch build.
#ifndef TOKYO_IPOD_RENDER_H
#define TOKYO_IPOD_RENDER_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "../../n3ds/src/core.h"

// The screen as it is held (landscape) in pixels; the drawable is the same
// screen on its side, HEIGHT wide and WIDTH high.
enum { WIDTH = 480, HEIGHT = 320 };
// Side of the textures over the grid of heights: the lamps' light and the shadows.
#define MAP_SIDE 1024

// What the renderer takes from the pack. The tables stay in memory; the
// vertices, indices and texels are copied to the GPU by render_init and may
// go afterwards.
typedef struct {
  const TkCity *city;
  const TkBatch *batches;
  const TkNearCell *near;
  // Top, wall and solid vertices and the indices of the levels that stay in memory.
  const uint8_t *vtx[3];
  uint32_t vtx_bytes[3];
  const uint16_t *idx;
  uint32_t idx_bytes;
  // One picture per block, then the facades; their texels.
  const TkPicture *pictures;
  uint32_t picture_count, block_count;
  const uint8_t *texels;
  // The lamps' light, MAP_SIDE squared texels of 16 bits.
  const uint8_t *lamps;
} RenderData;

typedef struct {
  uint32_t draws, tris[TK_KINDS];
} RenderStats;
extern RenderStats render_stats;

// The GL context is current for all of these.
bool render_init(const RenderData *data, char *error, size_t capacity);
// One frame into the bound framebuffer, a quarter turn round.
void render_frame(const TkView *view, const TkItem *const lists[TK_KINDS], const uint32_t counts[TK_KINDS]);
// A cell's record that has arrived for a slot: its vertices, indices and picture go to the GPU.
void render_cell(unsigned slot, const TkNearCell *record, const uint8_t *bytes);
// Rows of a sweep of the shadows (MAP_SIDE texels of 8 bits each), from row `first`; with `last`, frames
// read the sweep from now on.
void render_shadows(const uint8_t *texels, unsigned first, unsigned rows, bool last);

#endif
