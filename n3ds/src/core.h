/* The C interface of the Rust core (../core/src/lib.rs) and the pack's
 * records the renderer reads (crates/tokyo-pack). tk_sizes() checks at start
 * that both sides agree. */
#ifndef TOKYO_CORE_H
#define TOKYO_CORE_H
#include <stdint.h>

#define TK_SECTORS 16
#define TK_KINDS 4
#define TK_SLOTS 40
#define TK_DOME_VERTS 129
#define TK_DOME_INDICES 720
enum { TK_TOP = 0, TK_WALL = 1, TK_SOLID = 2, TK_OPEN = 3 };
enum { TK_FREE = 0, TK_WANTED = 1, TK_READY = 2, TK_RETIRED = 3 };
/* camera::btn and flight::key */
enum { TK_FAST = 1, TK_UP = 2, TK_DOWN = 4, TK_TOUR = 1 << 8, TK_STATS = 1 << 9, TK_LATER = 1 << 10, TK_EARLIER = 1 << 11, TK_FASTER = 1 << 12, TK_SLOWER = 1 << 13 };

typedef struct {
  float block;
  uint32_t blocks_x, blocks_z;
  float x0, z0;
  uint32_t cells;
  float y0, y_span, margin;
  float grid_x0, grid_z0, grid_step;
  uint32_t grid_w, grid_h;
  float height_step;
  float view[6];
  float hour;
  uint32_t region_blocks, flags;
} TkCity;

typedef struct {
  uint32_t kind, vtx_first, vtx_count, idx_first, idx_count;
  float min[3], max[3];
  uint32_t spans;
} TkBatch;

typedef struct {
  uint32_t offset, size;
  /* top vertices, wall vertices, solid vertices, indices, picture: each starts on a 16-byte boundary */
  uint32_t parts[5];
  uint16_t width, levels;
} TkNearCell;

typedef struct {
  uint32_t offset, size;
  uint16_t width, height;
  uint32_t levels;
} TkPicture;

typedef struct {
  uint32_t batch, place, cell, from, to;
  uint8_t region, sector, pad[2];
} TkItem;

typedef struct {
  volatile uint32_t state;
  uint32_t cell, offset, size;
  volatile uint32_t rank;
} TkSlot;

typedef struct {
  const TkCity *city;
  const void *regions;
  uint32_t region_count;
  const void *blocks;
  uint32_t block_count;
  const void *cells;
  uint32_t cell_count;
  const TkBatch *batches;
  uint32_t batch_count;
  const uint32_t *spans;
  uint32_t span_count;
  const float *tour;
  uint32_t tour_count;
  const uint16_t *heights;
  const TkNearCell *near;
  uint32_t near_offset;
  const uint8_t *meta;
  uint32_t meta_len;
  const void *landmarks;
  uint32_t landmark_count;
} TkPack;

typedef struct {
  uint32_t buttons, keys;
  float lx, ly, rx, ry;
} TkPad;

typedef struct {
  float eye[3], look[3], fov;
  float hour, night;
  float haze[3];
  float top[3];
  float lights[TK_SECTORS + 1][3];
  float shade;
  float sun[3];
  float near, mid;
  uint32_t tour_on, stats, pace, option;
} TkView;

typedef struct {
  float frame, worst;
  uint32_t late, frames;
  float cpu, gpu;
  uint32_t draws, tris[TK_KINDS];
} TkPerf;

typedef struct {
  float pos[3];
  uint8_t color[4];
} TkSkyVertex;

void tk_sizes(uint32_t out[8]);
const char *tk_init(const TkPack *pack, uint32_t budget);
void tk_control(const char *text, uint32_t len);
void tk_step(const TkPad *pad, uint32_t ticks);
void tk_view(TkView *out);
void tk_sky(TkSkyVertex *out, float radius);
uint32_t tk_sky_indices(uint16_t *out);
TkSlot *tk_slots(void);
void tk_choose(const TkItem **lists, uint32_t *lengths);
int32_t tk_slot_of(uint32_t cell);
void tk_refill(void);
void tk_drew(uint32_t tris);
void tk_shadows(const float *sun, float shade, uint16_t *swept, uint8_t *texture, uint32_t side);
uint32_t tk_title(char *out, uint32_t cap);
uint32_t tk_status(char *out, uint32_t cap, const TkPerf *perf, const char *extra, uint32_t extra_len);
float tk_ground(float x, float z);

/* ---- the interface (ui/, a PocketJS guest the host runs: guest.c). The flow around the flight is the
 * core's (tokyo_interface::Session); the guest's service channel is answered there (svcwire.h). */
/* TkPad.keys: the button that opens the menu. The interface hears it by itself; with no interface on the
 * screen it hands the eye to the tour and takes it back. */
#define TK_MENU (1u << 16)
/* tk_stage: what the interface shows while there is no flight. */
#define TK_STAGE_LOADING 0u
#define TK_STAGE_ERROR 1u
/* Before the flight exists: the loading step, or why the start failed. The first tk_step replaces it with the flow. */
void tk_stage(uint32_t stage, const char *message, uint32_t len);
/* The preferences read from storage at the start, and what the interface asked to have stored since the last
 * call (NUL-terminated; 0 when there is nothing). */
void tk_prefs_stored(const char *text, uint32_t len);
uint32_t tk_prefs_take(char *out, uint32_t cap);
/* Whether the guest's next turn is worth taking: it has something scheduled, the flight has news for it, or a
 * button it listens to changed (`buttons`: PocketJS's bits, as held since the last offer; `touching`: a contact). */
uint32_t tk_guest_due(uint32_t buttons, uint32_t touching);
/* A guest holds the interface's channel. */
uint32_t tk_interface_open(void);
/* Once a frame: the statistics line the interface shows while its setting is on. */
void tk_report(const TkPerf *perf);
/* Words of a control text that are the flow's: mode=title|flight|menu, ui=tour|fly|menu|resume|title. */
void tk_remote(const char *text, uint32_t len);
#ifdef __APPLE__
/* The iPod touch (ipod/core builds the same source): a frame of the Pocket3D title card as RGBA rows.
 * 0: the card is over; 1: drawn; 2: the frame is the one drawn at tick `shown`. */
uint32_t tk_card(uint8_t *pixels, uint32_t width, uint32_t height, uint32_t tick, uint32_t shown);
#endif
#endif
