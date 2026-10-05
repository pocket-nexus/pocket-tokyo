/* The PICA200 renderer of Pocket Tokyo. */
#ifndef TOKYO_RENDER_H
#define TOKYO_RENDER_H
#include <3ds.h>
#include <citro3d.h>
#include <stdbool.h>
#include <stddef.h>

#include "core.h"

/* Side of the textures over the grid of heights: the lamps' light and the shadows. */
#define MAP_SIDE 1024

typedef struct {
  const TkCity *city;
  const TkBatch *batches;
  const TkNearCell *near;
  /* Top, wall and solid vertices and the indices of the levels that stay in memory (linear memory). */
  const uint8_t *vtx[3];
  const uint16_t *idx;
  /* One picture per block, then the facades; their texels (linear memory). */
  const TkPicture *pictures;
  uint32_t picture_count, block_count;
  const uint8_t *texels;
  /* The lamps' light, MAP_SIDE squared texels of 16 bits (linear memory). */
  const uint8_t *lamps;
  /* Bytes of the largest cell's record. */
  uint32_t slot_bytes;
} RenderData;

typedef struct {
  uint32_t draws, tris[TK_KINDS];
} RenderStats;
extern RenderStats render_stats;

bool render_init(const RenderData *d, char *error, size_t n);
void render_frame(C3D_RenderTarget *target, const TkView *view, const TkItem *const lists[TK_KINDS], const uint32_t counts[TK_KINDS]);
/* A slot's memory, for the reading thread: the cell's record goes there whole. */
uint8_t *render_slot(unsigned slot);
/* Hands a record that has arrived in a slot to the GPU: its picture into the slot's texture. */
void render_slot_arrived(unsigned slot, const TkNearCell *record);
/* The shadows' texture: MAP_SIDE squared texels of 8 bits, written in place. */
uint8_t *render_shadows(void);
#endif
