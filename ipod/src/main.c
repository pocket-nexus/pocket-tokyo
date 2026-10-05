// Pocket Tokyo on the iPod touch 4 (iOS 6): the shell around the renderer
// (render.c), the Rust core (../core, the 3DS core's source), which says
// where the eye is, what the light is and what a frame draws, and the
// interface, a PocketJS guest (ui/, QuickJS) drawn over the city. The device
// has a touch panel and no pad: the stick, the keys and the view all come from
// the interface's touch presentation as `drive` and `look` commands, which the
// core's flow applies to the flight. This shell draws no 2D of its own.
//
// The same city as the Vita build, lowered by the city compiler for this
// machine (profiles/ipod60.json): 480 x 320 with four samples a pixel at 60
// frames a second, OpenGL ES 2 on the SGX535.
//
// The device builds from the macOS SDK's C headers, so UIKit is reached
// through the Objective-C runtime. The main thread owns UIKit and receives
// touches; one render thread owns the GL context, the flight and the guest,
// and they share only `shared` under its lock. A third thread, below the render thread's priority, reads
// the cells' near levels from the pack and sweeps the shadows.
//
// The EAGL layer is the portrait screen at 320x480, opaque and untransformed,
// with nothing over it: Core Animation shows the frame as it is instead of
// compositing it with the same GPU. The interface is drawn into a texture
// when what it shows changes, and every program of the scene reads that
// texture at its own pixel. Everything is drawn a quarter turn round, for a device
// held with its home button on the right.
//
// Development loop (tools/ipod.ts): `tmp/control.txt` holds a nonce and words
// for this shell, for the flow (tk_remote) and for the flight
// (tokyo_sim::flight::Flight::control); `tmp/status.json` reports the run.
#include "contact_latch.h"
#include "pocket_runtime.h"
#include "render.h"
#include "svcwire.h"
#define GL_SILENCE_DEPRECATION 1
#include <OpenGL/gl3.h>
#include <fcntl.h>
#include <mach/mach.h>
#include <mach/mach_time.h>
#include <math.h>
#include <objc/message.h>
#include <objc/runtime.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>
#ifndef TOKYO_BUILD
#define TOKYO_BUILD "development"
#endif
// Triangles a frame may draw: the distances of the levels of detail follow it.
// 32 000 hold a refresh a frame without the interface; reading it at every
// pixel and redrawing it 11 times a second take 8 000 of them.
#ifndef BUDGET
#define BUDGET 24000
#endif

// Guest turns a second at most: one per frame, so the stick and the keys reach
// the flight the frame after a thumb moves. A turn advances the UI core by the
// display refreshes since the last one.
#ifndef TURN_HZ
#define TURN_HZ 60
#endif
#define TAG(a, b, c, d) ((uint32_t)(a) | ((uint32_t)(b) << 8) | ((uint32_t)(c) << 16) | ((uint32_t)(d) << 24))
typedef struct {
  float x, y;
} Spot; // CGPoint: CGFloat is a float on this device
typedef struct {
  float x, y, width, height;
} Frame; // CGRect
extern int UIApplicationMain(int, char **, id, id);
extern uint64_t ui_draw_hash(void);
extern int32_t ui_gl_render_over(int32_t, int32_t, int32_t, int32_t, int32_t, int32_t);
extern void glDiscardFramebufferEXT(GLenum, GLsizei, const GLenum *);
// APPLE_framebuffer_multisample
extern void glRenderbufferStorageMultisampleAPPLE(GLenum, GLsizei, GLenum, GLsizei, GLsizei);
extern void glResolveMultisampleFramebufferAPPLE(void);
#define READ_FRAMEBUFFER_APPLE 0x8CA8
#define DRAW_FRAMEBUFFER_APPLE 0x8CA9
#define RGBA8_OES 0x8058
// Samples a pixel of the scene is drawn with.
#ifndef SAMPLES
#define SAMPLES 4
#endif
enum { WINDOW = 120 };

static struct {
  PocketContactLatch touches;
  bool active, parked;
} shared = {.active = true};
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t changed = PTHREAD_COND_INITIALIZER;
static char bundle[1024], tmp[1024], documents[1024];
static id context, view;
static GLuint framebuffer, colorbuffer;
// The scene's own target, with SAMPLES samples a pixel; it is resolved into `framebuffer`.
static GLuint scene_target;

static SEL sel(const char *name) { return sel_registerName(name); }
static id cls(const char *name) { return (id)objc_getClass(name); }
static id send(id object, const char *name) { return ((id(*)(id, SEL))objc_msgSend)(object, sel(name)); }
static id send_id(id object, const char *name, id value) { return ((id(*)(id, SEL, id))objc_msgSend)(object, sel(name), value); }
static void send_int(id object, const char *name, int value) { ((void (*)(id, SEL, int))objc_msgSend)(object, sel(name), value); }
static id string(const char *text) {
  return ((id(*)(id, SEL, const char *))objc_msgSend)(cls("NSString"), sel("stringWithUTF8String:"), text);
}
static double now(void) {
  static mach_timebase_info_data_t rate;
  if (!rate.denom)
    mach_timebase_info(&rate);
  return (double)mach_absolute_time() * rate.numer / rate.denom * 1e-9;
}
// Replaces the file in one step: the host never reads half a status.
static void write_file(const char *directory, const char *name, const void *data, size_t size) {
  char path[1100], staging[1110];
  snprintf(path, sizeof path, "%s/%s", directory, name);
  snprintf(staging, sizeof staging, "%s.new", path);
  FILE *file = fopen(staging, "wb");
  if (!file)
    return;
  fwrite(data, 1, size, file);
  fclose(file);
  rename(staging, path);
}

// A whole file, with a NUL after it; `before` goes in front.
static char *read_file(const char *directory, const char *name, const char *before, size_t *size) {
  char path[1100];
  snprintf(path, sizeof path, "%s/%s", directory, name);
  FILE *file = fopen(path, "rb");
  if (!file)
    return NULL;
  fseek(file, 0, SEEK_END);
  size_t length = ftell(file), lead = strlen(before);
  rewind(file);
  char *data = malloc(lead + length + 1);
  memcpy(data, before, lead);
  *size = lead + fread(data + lead, 1, length, file);
  data[*size] = 0;
  fclose(file);
  return data;
}

// ---- the pack

typedef struct {
  uint32_t tag, offset, size, zero;
} Section;

static char pack_path[1100];
static Section sections[32];
static uint32_t section_count;
static off_t pack_bytes;
static const TkNearCell *near_cells;
static uint32_t slot_bytes;

static const Section *section(uint32_t tag) {
  for (uint32_t i = 0; i < section_count; i++)
    if (sections[i].tag == tag)
      return &sections[i];
  return NULL;
}
// A table of the pack in memory of its own: it stays for the life of the program.
static void *table(const uint8_t *pack, uint32_t tag, uint32_t *size) {
  const Section *s = section(tag);
  if (!s)
    return NULL;
  void *copy = malloc(s->size ? s->size : 1);
  if (copy)
    memcpy(copy, pack + s->offset, s->size);
  if (size)
    *size = s->size;
  return copy;
}

static uint16_t *swept;
static uint8_t *shadow_texels, *staging;
// Rows of the grid of heights: the part of the shadows' texture a sweep writes.
static uint32_t grid_rows;

// Reads the pack's tables, hands what the GPU draws from to the renderer and
// starts the flight. The pack is mapped for the load only: the GPU and the
// tables keep their own copies, and the cells are read from the file as the
// eye comes near them.
static bool load(char *error, size_t capacity) {
  uint32_t sizes[8];
  tk_sizes(sizes);
  if (sizes[0] != sizeof(TkCity) || sizes[1] != sizeof(TkBatch) || sizes[2] != sizeof(TkNearCell) || sizes[3] != sizeof(TkItem) || sizes[4] != sizeof(TkView) ||
      sizes[5] != sizeof(TkSlot) || sizes[6] != TK_SLOTS || sizes[7] != TK_DOME_VERTS) {
    snprintf(error, capacity, "core.h does not match the core library");
    return false;
  }
  snprintf(pack_path, sizeof pack_path, "%s/city.pack", bundle);
  int file = open(pack_path, O_RDONLY);
  struct stat info;
  if (file < 0 || fstat(file, &info) || info.st_size < 16) {
    snprintf(error, capacity, "city.pack is missing");
    return false;
  }
  pack_bytes = info.st_size;
  const uint8_t *pack = mmap(NULL, info.st_size, PROT_READ, MAP_PRIVATE, file, 0);
  close(file);
  const uint32_t *head = (const uint32_t *)pack;
  if (pack == MAP_FAILED || head[0] != TAG('T', 'K', 'P', 'K') || head[1] != 3 || head[2] > 32) {
    snprintf(error, capacity, "city.pack is not a version 3 pack");
    return false;
  }
  section_count = head[2];
  memcpy(sections, pack + 16, section_count * sizeof *sections);
  for (uint32_t i = 0; i < section_count; i++)
    if (sections[i].offset + (uint64_t)sections[i].size > (uint64_t)info.st_size) {
      snprintf(error, capacity, "city.pack is cut short");
      return false;
    }
  static TkPack p;
  static RenderData data;
  uint32_t size = 0, pictures = 0;
  p.city = table(pack, TAG('C', 'I', 'T', 'Y'), &size);
  p.regions = table(pack, TAG('R', 'E', 'G', 'N'), &size);
  p.region_count = size / 24;
  p.blocks = table(pack, TAG('B', 'L', 'C', 'K'), &size);
  p.block_count = size / 24;
  p.cells = table(pack, TAG('C', 'E', 'L', 'L'), &size);
  p.cell_count = size / 32;
  p.batches = table(pack, TAG('B', 'T', 'C', 'H'), &size);
  p.batch_count = size / sizeof(TkBatch);
  p.spans = table(pack, TAG('S', 'P', 'A', 'N'), &size);
  p.span_count = size / 4;
  p.tour = table(pack, TAG('T', 'O', 'U', 'R'), &size);
  p.tour_count = size / 24;
  p.heights = table(pack, TAG('H', 'M', 'A', 'P'), NULL);
  p.near = near_cells = table(pack, TAG('N', 'C', 'E', 'L'), NULL);
  p.meta = table(pack, TAG('M', 'E', 'T', 'A'), &p.meta_len);
  p.landmarks = table(pack, TAG('L', 'A', 'N', 'D'), &size);
  p.landmark_count = p.landmarks ? size / 64 : 0;
  data.pictures = table(pack, TAG('H', 'P', 'I', 'C'), &pictures);
  const Section *near = section(TAG('N', 'E', 'A', 'R')), *texels = section(TAG('H', 'T', 'E', 'X')), *lamps = section(TAG('L', 'A', 'M', 'P')),
                *indices = section(TAG('I', 'D', 'X', '0'));
  static const uint32_t vertices[3] = {TAG('V', 'T', 'O', 'P'), TAG('V', 'W', 'A', 'L'), TAG('V', 'S', 'O', 'L')};
  bool whole = p.city && p.regions && p.blocks && p.cells && p.batches && p.spans && p.tour && p.heights && p.near && p.meta && data.pictures && near && texels && lamps && indices;
  for (int k = 0; k < 3 && whole; k++) {
    const Section *s = section(vertices[k]);
    whole = s != NULL;
    if (s)
      data.vtx[k] = pack + s->offset, data.vtx_bytes[k] = s->size;
  }
  if (!whole || lamps->size != MAP_SIDE * MAP_SIDE * 2 || p.city->grid_w > MAP_SIDE || p.city->grid_h > MAP_SIDE) {
    snprintf(error, capacity, "city.pack is not an iPod touch pack: cook with --profile ipod60");
    munmap((void *)pack, info.st_size);
    return false;
  }
  p.near_offset = near->offset;
  grid_rows = p.city->grid_h;
  data.city = p.city;
  data.batches = p.batches;
  data.near = p.near;
  data.idx = (const uint16_t *)(pack + indices->offset);
  data.idx_bytes = indices->size;
  data.picture_count = pictures / sizeof(TkPicture);
  data.block_count = p.block_count;
  data.texels = pack + texels->offset;
  data.lamps = pack + lamps->offset;
  for (uint32_t i = 0; i < p.cell_count; i++)
    if (near_cells[i].size > slot_bytes)
      slot_bytes = near_cells[i].size;
  bool ok = render_init(&data, error, capacity);
  munmap((void *)pack, info.st_size);
  if (!ok)
    return false;
  swept = malloc(p.city->grid_w * p.city->grid_h * 2);
  shadow_texels = malloc(MAP_SIDE * MAP_SIDE);
  staging = malloc(slot_bytes ? slot_bytes : 1);
  const char *refusal = tk_init(&p, BUDGET);
  if (refusal || !swept || !shadow_texels || !staging) {
    snprintf(error, capacity, "%s", refusal ? refusal : "no memory for the shadows");
    return false;
  }
  memset(shadow_texels, 255, MAP_SIDE * MAP_SIDE);
  // (a frame is shown for one refresh here: the flight's `pace` says so to whoever reads it)
  tk_control("pace=1", 6);
  return true;
}

// ---- the thread that reads cells and sweeps shadows
//
// It runs below the render thread's priority, in the time a frame waits for
// the display. A cell's near level is one record of the pack, read whole into
// the staging block; the render thread hands it to the GPU at the start of
// its next frame and sets the slot ready. When no cell is wanted and the sun
// has moved a degree, the shadows are swept anew, and the render thread
// uploads the texture a strip a frame.
static volatile float want_sun[3], want_shade;
static volatile int staged = -1;
static volatile bool shadows_swept;
static volatile uint32_t cells_read, sweeps, read_ms_total, sweep_ms_last;

static void *worker(void *unused) {
  (void)unused;
  int file = open(pack_path, O_RDONLY);
  TkSlot *slots = tk_slots();
  float done_sun[3] = {0, 0, 0}, done_shade = -1.0f;
  while (file >= 0) {
    if (staged >= 0) {
      usleep(2000);
      continue;
    }
    int pick = -1;
    for (int i = 0; i < TK_SLOTS; i++)
      if (slots[i].state == TK_WANTED && (pick < 0 || slots[i].rank < slots[pick].rank))
        pick = i;
    if (pick >= 0) {
      TkSlot *s = &slots[pick];
      double from = now();
      if (pread(file, staging, s->size, s->offset) == (ssize_t)s->size) {
        cells_read++;
        read_ms_total += (uint32_t)((now() - from) * 1000);
        __sync_synchronize();
        staged = pick;
      } else
        usleep(200000);
      continue;
    }
    float sun[3] = {want_sun[0], want_sun[1], want_sun[2]}, shade = want_shade;
    bool lit = sun[0] != 0.0f || sun[1] != 0.0f || sun[2] != 0.0f;
    // (the cosine of one degree; and the shade is written into the texture, so a change of it counts too)
    if (lit && !shadows_swept && (sun[0] * done_sun[0] + sun[1] * done_sun[1] + sun[2] * done_sun[2] < 0.99985f || fabsf(shade - done_shade) > 0.03f)) {
      double from = now();
      tk_shadows(sun, shade, swept, shadow_texels, MAP_SIDE);
      memcpy(done_sun, sun, sizeof sun);
      done_shade = shade;
      sweeps++;
      sweep_ms_last = (uint32_t)((now() - from) * 1000);
      __sync_synchronize();
      shadows_swept = true;
      continue;
    }
    usleep(5000);
  }
  return NULL;
}

// What the thread has ready goes to the GPU: one cell, and a strip of the shadows.
static void arrivals(void) {
  enum { STRIP = 128 };
  static uint32_t row;
  if (staged >= 0) {
    TkSlot *s = &tk_slots()[staged];
    render_cell(staged, &near_cells[s->cell], staging);
    s->state = TK_READY;
    __sync_synchronize();
    staged = -1;
  }
  if (shadows_swept) {
    uint32_t rows = grid_rows - row < STRIP ? grid_rows - row : STRIP;
    render_shadows(shadow_texels, row, rows, row + rows >= grid_rows);
    row += rows;
    if (row >= grid_rows) {
      row = 0;
      shadows_swept = false;
    }
  }
}

// ---- render thread

enum { LOADING, RUNNING, FAILED };
// Milliseconds per presented frame: the frame, its work, the flight, the
// guest's turn, the interface's redraw (with the check that says whether one
// is due), choosing the draws, what arrived for the GPU, the scene's commands (with the samples
// resolved, which waits for the GPU to take the frame before), the GPU's own
// work when a run asks for it to be timed, handing the frame over, the present.
enum { INTERVAL, WORK, SIM, GUEST, REDRAW, CHOOSE, ARRIVALS, SCENE, GPU, KICK, PRESENT, TIMINGS };
static float timing[TIMINGS][WINDOW];
static int stage = LOADING;
static bool guest;
static unsigned frames, late, turns, redraws;
static char command[40], captured[40], failure[512];
// The interface is drawn into one of two textures in turn: the frame the GPU
// still works on reads the other, so a redraw never waits for it.
static GLuint interface_target[2], interface_texture[2], overlay;
static unsigned interface_shown;
static TkPerf perf;
// Contacts a command holds on the panel, for driving the interface from the
// host; `tap` lifts them again after that many turns.
static struct {
  int count;
  float at[4][2];
} fingers, fingered;
static int tap;
// A measurement the device takes by itself: `mark=SECONDS` starts one three
// seconds later, and the host reads it when it is done. Asking the device
// anything over SSH costs its one core a few frames, so nothing is asked
// while the window is open.
static struct {
  double from, until, seconds;
  unsigned frames, late, min_tris, max_tris, max_draws, turns, redraws;
  uint64_t tris, draws;
  float worst;
  // (the guest's turns, and the redraws of the interface's texture alone)
  double sums[TIMINGS], turn_ms, redraw_ms;
  bool done;
} measured;

// The status record on its way to the file.
static char report[6144];
static int report_size;
static pthread_mutex_t report_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t report_ready = PTHREAD_COND_INITIALIZER;

static int compare(const void *a, const void *b) { return (*(const float *)a > *(const float *)b) - (*(const float *)a < *(const float *)b); }
static int summary(char *out, size_t capacity, const char *name, const float *a) {
  float sorted[WINDOW], sum = 0;
  unsigned n = frames < WINDOW ? frames : WINDOW;
  for (unsigned i = 0; i < n; i++)
    sum += sorted[i] = a[i];
  qsort(sorted, n, sizeof *sorted, compare);
  return snprintf(out, capacity, "\"%s\":{\"mean\":%.3f,\"p95\":%.3f,\"max\":%.3f},", name, n ? sum / n : 0, n ? sorted[n * 95 / 100] : 0,
                  n ? sorted[n - 1] : 0);
}
static void status(void) {
  static const char *const names[TIMINGS] = {"intervalMs", "workMs", "simMs", "guestMs", "interfaceDrawMs", "chooseMs", "arrivalsMs", "sceneMs", "finishMs", "kickMs", "presentMs"};
  static const char *const stages[] = {"loading", "running", "failed"};
  static char text[6144], extra[4096], error[2 * sizeof failure];
  unsigned e = 0;
  for (const char *c = failure; *c && e < sizeof error - 2; c++) {
    if (*c == '"' || *c == '\\')
      error[e++] = '\\';
    error[e++] = *c < ' ' ? ' ' : *c;
  }
  error[e] = 0;
  task_basic_info_data_t task;
  mach_msg_type_number_t count = TASK_BASIC_INFO_COUNT;
  if (task_info(mach_task_self(), TASK_BASIC_INFO, (task_info_t)&task, &count) != KERN_SUCCESS)
    task.resident_size = 0;
  int at = snprintf(extra, sizeof extra, "\"build\":\"" TOKYO_BUILD "\",\"fps\":%.3f,", perf.frame > 0 ? 1000 / perf.frame : 0);
  // The last 120 presented frames.
  for (unsigned i = 0; i < TIMINGS; i++)
    at += summary(extra + at, sizeof extra - at, names[i], timing[i]);
  at += snprintf(extra + at, sizeof extra - at,
                 "\"packBytes\":%llu,\"slotBytes\":%u,\"read\":{\"cells\":%u,\"ms\":%u},\"shadows\":{\"sweeps\":%u,\"ms\":%u},"
                 "\"guest\":{\"running\":%s,\"error\":\"%s\",\"turnHz\":%d,\"turns\":%u,\"redraws\":%u},"
                 "\"residentBytes\":%u,\"glError\":%u,\"lastCommand\":\"%s\",\"capture\":\"%s\"",
                 (unsigned long long)pack_bytes, slot_bytes, cells_read, read_ms_total, sweeps, sweep_ms_last, guest ? "true" : "false",
                 guest ? "" : pocket_runtime_error(), TURN_HZ, turns, redraws, (unsigned)task.resident_size, glGetError(), command, captured);
  at += snprintf(extra + at, sizeof extra - at,
                 ",\"window\":{\"done\":%s,\"seconds\":%.3f,\"frames\":%u,\"late\":%u,\"worstMs\":%.3f,\"minTris\":%u,\"maxTris\":%u,\"meanTris\":%.0f,\"maxDraws\":%u,"
                 "\"meanDraws\":%.0f,\"turns\":%u,\"redraws\":%u,\"turnMs\":%.3f,\"redrawMs\":%.3f,\"meanMs\":{",
                 measured.done ? "true" : "false", measured.seconds, measured.frames, measured.late, measured.worst, measured.frames ? measured.min_tris : 0,
                 measured.max_tris, measured.frames ? (double)measured.tris / measured.frames : 0, measured.max_draws,
                 measured.frames ? (double)measured.draws / measured.frames : 0, measured.turns, measured.redraws,
                 measured.turns ? measured.turn_ms / measured.turns : 0, measured.redraws ? measured.redraw_ms / measured.redraws : 0);
  for (unsigned i = 0; i < TIMINGS; i++)
    at += snprintf(extra + at, sizeof extra - at, "%s\"%.*s\":%.3f", i ? "," : "", (int)strlen(names[i]) - 2, names[i], measured.frames ? measured.sums[i] / measured.frames : 0);
  at += snprintf(extra + at, sizeof extra - at, "}}");
  if (stage == RUNNING)
    at = tk_status(text, sizeof text, &perf, extra, at);
  else
    at = snprintf(text, sizeof text, "{\"target\":\"ipod\",\"stage\":\"%s\",\"error\":\"%s\",%s}", stages[stage], error, extra);
  // A file write can take longer than the frame has left: another thread does it.
  pthread_mutex_lock(&report_lock);
  memcpy(report, text, report_size = at);
  pthread_cond_signal(&report_ready);
  pthread_mutex_unlock(&report_lock);
}
static void *reporter(void *unused) {
  (void)unused;
  static char text[sizeof report];
  for (;;) {
    pthread_mutex_lock(&report_lock);
    while (!report_size)
      pthread_cond_wait(&report_ready, &report_lock);
    int size = report_size;
    memcpy(text, report, size);
    report_size = 0;
    pthread_mutex_unlock(&report_lock);
    write_file(tmp, "status.json", text, size);
  }
  return NULL;
}

// The interface's own drawable, and the pass that shows a landscape picture
// by itself: the title card's frames, and the interface while the pack is read.
static void cover(void) {
  glGenTextures(2, interface_texture);
  glGenFramebuffers(2, interface_target);
  for (unsigned i = 0; i < 2; i++) {
    glBindTexture(GL_TEXTURE_2D, interface_texture[i]);
    glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA, WIDTH, HEIGHT, 0, GL_RGBA, GL_UNSIGNED_BYTE, NULL);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
    glBindFramebuffer(GL_FRAMEBUFFER, interface_target[i]);
    glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, interface_texture[i], 0);
  }
  static const char *const sources[2] = {
    // The picture is landscape: its x runs down the portrait drawable.
    "attribute vec2 aPos; varying highp vec2 vAt;\n"
    "void main() { gl_Position = vec4(aPos, 0.0, 1.0); vAt = vec2(0.5 - aPos.y * 0.5, 0.5 + aPos.x * 0.5); }\n",
    "precision lowp float; uniform sampler2D uPicture; varying highp vec2 vAt;\n"
    "void main() { gl_FragColor = texture2D(uPicture, vAt); }\n"};
  overlay = glCreateProgram();
  for (unsigned i = 0; i < 2; i++) {
    GLuint shader = glCreateShader(i ? GL_FRAGMENT_SHADER : GL_VERTEX_SHADER);
    glShaderSource(shader, 1, &sources[i], NULL);
    glCompileShader(shader);
    glAttachShader(overlay, shader);
    glDeleteShader(shader);
  }
  glBindAttribLocation(overlay, 0, "aPos");
  glLinkProgram(overlay);
}

// The Pocket3D title card, first at every launch and before anything of the
// city's is read or shown. The frames are PocketJS's (the core draws each
// tick's, `tk_card`); they reach the screen through a texture. The card
// follows the clock, so a slow frame skips a tick and the card still takes its
// 2.4 seconds. A development run that left `tmp/title.want` gets the card's
// held frame in `tmp/title.rgba`, as presented.
static void title(void) {
  static uint8_t pixels[WIDTH * HEIGHT * 4], row[WIDTH * 4];
  static const GLfloat corners[] = {-1, -1, 1, -1, -1, 1, 1, 1};
  char want[1200];
  snprintf(want, sizeof want, "%s/title.want", tmp);
  bool wanted = !access(want, F_OK);
  GLuint texture;
  glGenTextures(1, &texture);
  glBindTexture(GL_TEXTURE_2D, texture);
  glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
  glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
  glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
  glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
  glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA, WIDTH, HEIGHT, 0, GL_RGBA, GL_UNSIGNED_BYTE, NULL);
  double start = now();
  uint32_t shown = UINT32_MAX;
  for (;;) {
    pthread_mutex_lock(&lock);
    bool visible = shared.active;
    pthread_mutex_unlock(&lock);
    uint32_t tick = (uint32_t)((now() - start) * 60.0);
    if (!visible) {
      // Background GL kills the process; the card waits where it was.
      glFinish();
      usleep(50000);
      start = now() - tick / 60.0;
      continue;
    }
    uint32_t drawn = tk_card(pixels, WIDTH, HEIGHT, tick, shown);
    if (!drawn)
      break;
    if (drawn == 1) {
      shown = tick;
      // A texture's first row is its bottom one.
      for (unsigned y = 0; y < HEIGHT / 2; y++) {
        uint8_t *a = pixels + y * WIDTH * 4, *b = pixels + (HEIGHT - 1 - y) * WIDTH * 4;
        memcpy(row, a, sizeof row);
        memcpy(a, b, sizeof row);
        memcpy(b, row, sizeof row);
      }
      glTexSubImage2D(GL_TEXTURE_2D, 0, 0, 0, WIDTH, HEIGHT, GL_RGBA, GL_UNSIGNED_BYTE, pixels);
    }
    glBindFramebuffer(GL_FRAMEBUFFER, framebuffer);
    glViewport(0, 0, HEIGHT, WIDTH);
    glDisable(GL_DEPTH_TEST);
    glDisable(GL_BLEND);
    glUseProgram(overlay);
    glEnableVertexAttribArray(0);
    glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, 0, corners);
    glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
    if (wanted && tick >= 72) {
      static uint8_t presented[WIDTH * HEIGHT * 4];
      glReadPixels(0, 0, HEIGHT, WIDTH, GL_RGBA, GL_UNSIGNED_BYTE, presented);
      write_file(tmp, "title.rgba", presented, sizeof presented);
      unlink(want);
      wanted = false;
    }
    glBindRenderbuffer(GL_RENDERBUFFER, colorbuffer);
    ((BOOL(*)(id, SEL, unsigned))objc_msgSend)(context, sel("presentRenderbuffer:"), GL_RENDERBUFFER);
  }
  glDeleteTextures(1, &texture);
}

// A word of a control message that is this shell's:
//   touch=X,Y[;X,Y…]  fingers held on the panel, in the interface's pixels; touch=off lifts them
//   tap=X,Y           one finger down for a few turns
//   screen=1          writes the next presented frame to tmp/screen.rgba
//   mark=SECONDS      measures that long, starting in three seconds (`window` in the status)
static bool screen;
static void host_word(const char *word) {
  if (!strncmp(word, "touch=", 6) || !strncmp(word, "tap=", 4)) {
    tap = word[1] == 'a' ? 6 : 0;
    fingers.count = 0;
    for (const char *at = strchr(word, '=') + 1; fingers.count < 4 && sscanf(at, "%f,%f", &fingers.at[fingers.count][0], &fingers.at[fingers.count][1]) == 2;) {
      fingers.count++;
      at = strchr(at, ';');
      if (!at++)
        break;
    }
  } else if (!strcmp(word, "screen=1"))
    screen = true;
  else if (!strncmp(word, "mark=", 5)) {
    memset(&measured, 0, sizeof measured);
    measured.from = now() + 3;
    measured.until = measured.from + atof(word + 5);
  }
}

static void *render(void *unused) {
  (void)unused;
  send_id(cls("EAGLContext"), "setCurrentContext:", context);
  pthread_t writer;
  pthread_create(&writer, NULL, reporter, NULL);
  cover();
  title();
  // The interface is up before the pack is read, so the load shows through it.
  static const char reading[] = "Reading the city";
  tk_stage(TK_STAGE_LOADING, reading, sizeof reading - 1);
  size_t script_size, pak_size, prefs_size;
  char rate[40];
  snprintf(rate, sizeof rate, "globalThis.__simHz=%d;", TURN_HZ);
  char *script = read_file(bundle, "tokyo.js", rate, &script_size), *pak = read_file(bundle, "tokyo.pak", "", &pak_size);
  char *prefs = read_file(documents, "interface.json", "", &prefs_size);
  if (prefs)
    tk_prefs_stored(prefs, prefs_size);
  free(prefs);
  guest = script && pak && pocket_runtime_boot(script, script_size, (const uint8_t *)pak, pak_size, WIDTH, HEIGHT) && pocket_runtime_gl_initialize();
  if (!guest)
    snprintf(failure, sizeof failure, "interface: %s", script && pak ? pocket_runtime_error() : "tokyo.js or tokyo.pak is missing");

  char path[1200], words[1024];
  double previous = now(), started = previous, reported = 0;
  unsigned owed = 0; // display refreshes since the guest's last turn
  float average = 16.7f;
  uint64_t drawn_hash = 0;
  snprintf(path, sizeof path, "%s/control.txt", tmp);
  unlink(path);
  for (;;) {
    id pool = send(send(cls("NSAutoreleasePool"), "alloc"), "init");
    pthread_mutex_lock(&lock);
    while (!shared.active) {
      // Background GL kills the process: finish, then wait to be resumed.
      glFinish();
      shared.parked = true;
      pthread_cond_broadcast(&changed);
      pthread_cond_wait(&changed, &lock);
      previous = started = now();
    }
    shared.parked = false;
    pthread_mutex_unlock(&lock);

    // Commands from the host (tools/ipod.ts ctl): a nonce on the first line,
    // then words for this shell and for the flight. The file is replaced in
    // one step and acknowledged by its nonce in the status.
    FILE *file = fopen(path, "rb");
    if (file) {
      words[fread(words, 1, sizeof words - 1, file)] = 0;
      fclose(file);
      unlink(path);
      char *text = strchr(words, '\n');
      if (text) {
        *text++ = 0;
        snprintf(command, sizeof command, "%s", words);
        text[strcspn(text, "\r\n")] = 0;
        // (the flow's words, `mode=` and `ui=`, and the flight's)
        tk_remote(text, strlen(text));
        if (stage == RUNNING)
          tk_control(text, strlen(text));
        char *save = NULL;
        for (char *word = strtok_r(text, " ", &save); word; word = strtok_r(NULL, " ", &save))
          host_word(word);
      }
      reported = 0;
    }

    double start = now();
    float dt = fminf((float)(start - started), 0.1f);
    started = start;
    // One tick of the flight per display refresh since the last frame: a late frame catches up.
    unsigned ticks = (unsigned)(dt * 60 + 0.5f);
    ticks = ticks < 1 ? 1 : ticks > 3 ? 3 : ticks;
    unsigned slot = frames % WINDOW;

    // The pack loads once the interface has had two frames to say so.
    if (stage == LOADING && frames >= 2) {
      if (load(failure, sizeof failure)) {
        stage = RUNNING;
        pthread_t reader;
        pthread_attr_t attributes;
        struct sched_param priority;
        pthread_attr_init(&attributes);
        pthread_attr_getschedparam(&attributes, &priority);
        // (below this thread: it takes the time a frame waits for the display)
        int least = sched_get_priority_min(SCHED_OTHER);
        priority.sched_priority = priority.sched_priority - 12 > least ? priority.sched_priority - 12 : least;
        pthread_attr_setinheritsched(&attributes, PTHREAD_EXPLICIT_SCHED);
        pthread_attr_setschedpolicy(&attributes, SCHED_OTHER);
        pthread_attr_setschedparam(&attributes, &priority);
        pthread_create(&reader, &attributes, worker, NULL);
      } else {
        stage = FAILED;
        tk_stage(TK_STAGE_ERROR, failure, strlen(failure));
      }
      start = started = now();
    }

    // The flight, and what this frame draws.
    TkView seen = {0};
    const TkItem *lists[TK_KINDS] = {0};
    uint32_t counts[TK_KINDS] = {0};
    timing[SIM][slot] = timing[CHOOSE][slot] = timing[ARRIVALS][slot] = 0;
    if (stage == RUNNING) {
      // What the interface asked for since the last frame, then the flight. The pad stays empty: the stick,
      // the keys and the drags are the interface's commands.
      const TkPad nobody = {0};
      tk_step(&nobody, ticks);
      tk_report(&perf);
      static char keep[4096];
      uint32_t n = tk_prefs_take(keep, sizeof keep);
      if (n)
        write_file(documents, "interface.json", keep, n);
      tk_view(&seen);
      want_shade = seen.shade;
      for (int k = 0; k < 3; k++)
        want_sun[k] = seen.sun[k];
    }
    double stepped = now();
    timing[SIM][slot] = stage == RUNNING ? (float)(stepped - start) * 1000 : 0;

    // The interface's turn, when it is worth one: what the fingers are doing
    // goes in, the flight's state is read and commands are left for the next
    // tk_step. It is redrawn into its texture when what it shows has changed.
    timing[GUEST][slot] = timing[REDRAW][slot] = 0;
    bool turned = false, redrawn = false;
    owed += ticks;
    if (guest && owed >= 60 / TURN_HZ) {
      PocketRuntimeContactsInput input;
      pthread_mutex_lock(&lock);
      if (tap && !--tap)
        fingers.count = 0;
      for (int i = 0; i < 4; i++) {
        if (i < fingers.count)
          pocket_contact_event(&shared.touches, i < fingered.count ? POCKET_TOUCH_MOVE : POCKET_TOUCH_DOWN, -1 - i, fingers.at[i][0], fingers.at[i][1], WIDTH, HEIGHT);
        else if (i < fingered.count)
          pocket_contact_event(&shared.touches, POCKET_TOUCH_UP, -1 - i, fingered.at[i][0], fingered.at[i][1], WIDTH, HEIGHT);
      }
      bool touching = fingers.count || fingered.count;
      fingered = fingers;
      pocket_contacts_sample(&shared.touches, &input, WIDTH, HEIGHT, WIDTH, HEIGHT, pocket_runtime_hit_test_bounds);
      pthread_mutex_unlock(&lock);
      // (a finger that has just lifted still has its end to deliver)
      touching = touching || input.contact_count || input.cancelled_count;
      if (tk_guest_due(0, touching)) {
        unsigned elapsed = owed > 3 ? 3 : owed;
        owed = 0;
        turned = true;
        turns++;
        input.buttons = 0;
        if (!pocket_runtime_frame_contacts(&input, elapsed)) {
          // The guest threw: the city goes on without it, and the status says why.
          guest = false;
          snprintf(failure, sizeof failure, "interface: %s", pocket_runtime_error());
          svcwire_shutdown();
        }
        double done = now();
        // The texture follows what the interface shows on every other frame
        // at most: a frame that lays it out and draws it twice in a row
        // misses its refresh.
        static unsigned redrawn_at;
        uint64_t hash = guest && frames - redrawn_at >= 2 ? ui_draw_hash() : drawn_hash;
        if (hash != drawn_hash) {
          drawn_hash = hash;
          redrawn_at = frames;
          redrawn = true;
          redraws++;
          interface_shown ^= 1;
          glBindFramebuffer(GL_FRAMEBUFFER, interface_target[interface_shown]);
          glViewport(0, 0, WIDTH, HEIGHT);
          glDisable(GL_SCISSOR_TEST);
          glClearColor(0, 0, 0, 0);
          glClear(GL_COLOR_BUFFER_BIT);
          ui_gl_render_over(0, 0, WIDTH, HEIGHT, WIDTH, HEIGHT);
        }
        timing[GUEST][slot] = (float)(done - stepped) * 1000;
        timing[REDRAW][slot] = (float)(now() - done) * 1000;
      }
    }
    if (stage == RUNNING) {
      stepped = now();
      arrivals();
      double arrived = now();
      tk_choose(lists, counts);
      tk_refill();
      double chosen = now();
      timing[ARRIVALS][slot] = (float)(arrived - stepped) * 1000;
      timing[CHOOSE][slot] = (float)(chosen - arrived) * 1000;
    }

    double drawing = now();
    // (`option=1024`: one sample a pixel, straight into the drawable)
    bool sampled = stage == RUNNING && scene_target && !(seen.option & 1024);
    glBindFramebuffer(GL_FRAMEBUFFER, sampled ? scene_target : framebuffer);
    if (stage == RUNNING) {
      // The interface is a picture every program of the scene reads at its own pixel: no pass of its own.
      render_frame(&seen, lists, counts, guest ? interface_texture[interface_shown] : 0);
      if (sampled) {
        static const GLenum both[2] = {GL_COLOR_ATTACHMENT0, GL_DEPTH_ATTACHMENT};
        glBindFramebuffer(READ_FRAMEBUFFER_APPLE, scene_target);
        glBindFramebuffer(DRAW_FRAMEBUFFER_APPLE, framebuffer);
        glResolveMultisampleFramebufferAPPLE();
        glDiscardFramebufferEXT(READ_FRAMEBUFFER_APPLE, 2, both);
        glBindFramebuffer(GL_FRAMEBUFFER, framebuffer);
      }
    } else {
      // Before the city is there, the interface alone: over the ground of the title card.
      static const float corners[8] = {-1, -1, 1, -1, -1, 1, 1, 1};
      glViewport(0, 0, HEIGHT, WIDTH);
      glDisable(GL_SCISSOR_TEST);
      glClearColor(0.09f, 0.07f, 0.15f, 1);
      glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT);
      if (guest) {
        glDisable(GL_DEPTH_TEST);
        glDisable(GL_CULL_FACE);
        glEnable(GL_BLEND);
        glBlendFunc(GL_ONE, GL_ONE_MINUS_SRC_ALPHA); // the interface's texture holds premultiplied colour
        glBindBuffer(GL_ARRAY_BUFFER, 0);
        glBindBuffer(GL_ELEMENT_ARRAY_BUFFER, 0);
        glEnableVertexAttribArray(0);
        for (int i = 1; i < 4; i++)
          glDisableVertexAttribArray(i);
        glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, 0, corners);
        glActiveTexture(GL_TEXTURE0);
        glBindTexture(GL_TEXTURE_2D, interface_texture[interface_shown]);
        glUseProgram(overlay);
        glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
        glDisable(GL_BLEND);
      }
    }
    double drawn = now();
    timing[SCENE][slot] = (float)(drawn - drawing) * 1000;
    // To time the GPU (`option=512`): the frame is drawn before anything else is asked of it. The frames of
    // such a run are not paced as a flight's are.
    timing[GPU][slot] = 0;
    if (stage == RUNNING && seen.option & 512) {
      glFinish();
      timing[GPU][slot] = (float)(now() - drawn) * 1000;
      drawn = now();
    }
    if (screen) {
      // The frame as presented: rows of the portrait drawable from its bottom; the host turns them.
      static uint8_t pixels[WIDTH * HEIGHT * 4];
      glReadPixels(0, 0, HEIGHT, WIDTH, GL_RGBA, GL_UNSIGNED_BYTE, pixels);
      write_file(tmp, "screen.rgba", pixels, sizeof pixels);
      snprintf(captured, sizeof captured, "%s", command);
      screen = false;
      reported = 0;
      started = start = now(); // a readback is not a frame's work
    }
    const GLenum depth = GL_DEPTH_ATTACHMENT;
    glDiscardFramebufferEXT(GL_FRAMEBUFFER, 1, &depth);
    glBindRenderbuffer(GL_RENDERBUFFER, colorbuffer);
    double submitted = now();
    timing[KICK][slot] = (float)(submitted - drawn) * 1000;
    ((BOOL(*)(id, SEL, unsigned))objc_msgSend)(context, sel("presentRenderbuffer:"), GL_RENDERBUFFER);
    double presented = now();
    // A frame is shown for one refresh; one that took half a refresh more is late.
    float interval = (float)(presented - previous) * 1000, limit = 25.0f;
    timing[WORK][slot] = (float)(drawn - start) * 1000 - timing[GPU][slot];
    timing[PRESENT][slot] = (float)(presented - submitted) * 1000;
    timing[INTERVAL][slot] = interval;
    previous = presented;
    frames++;
    if (stage == RUNNING) {
      late += interval > limit;
      average += (interval - average) * 0.05f;
      float worst = 0;
      for (unsigned i = 0; i < (frames < WINDOW ? frames : WINDOW); i++)
        worst = fmaxf(worst, timing[INTERVAL][i]);
      unsigned tris = render_stats.tris[0] + render_stats.tris[1] + render_stats.tris[2] + render_stats.tris[3];
      perf = (TkPerf){.frame = average, .worst = worst, .late = late, .frames = frames, .cpu = timing[WORK][slot], .gpu = timing[GPU][slot], .draws = render_stats.draws,
                      .tris = {render_stats.tris[0], render_stats.tris[1], render_stats.tris[2], render_stats.tris[3]}};
      tk_drew(tris);
      if (measured.until && !measured.done && presented >= measured.from) {
        if (!measured.frames || tris < measured.min_tris)
          measured.min_tris = tris;
        measured.frames++;
        measured.late += interval > limit;
        measured.worst = fmaxf(measured.worst, interval);
        measured.tris += tris;
        measured.draws += render_stats.draws;
        measured.turns += turned;
        measured.redraws += redrawn;
        measured.turn_ms += timing[GUEST][slot];
        if (redrawn)
          measured.redraw_ms += timing[REDRAW][slot];
        if (tris > measured.max_tris)
          measured.max_tris = tris;
        if (render_stats.draws > measured.max_draws)
          measured.max_draws = render_stats.draws;
        for (unsigned i = 0; i < TIMINGS; i++)
          measured.sums[i] += timing[i][slot];
        if (presented >= measured.until) {
          measured.done = true;
          measured.seconds = presented - measured.from;
          reported = 0;
        }
      }
    }
    if (presented - reported > 0.5) {
      reported = presented;
      status();
    }
    send(pool, "drain");
  }
  return NULL;
}

// ---- main thread

// Every finger goes to the interface, in its landscape pixels.
static void touched(id self, SEL _cmd, id touches, id event) {
  (void)_cmd, (void)event;
  id all = send(touches, "allObjects");
  unsigned count = ((unsigned (*)(id, SEL))objc_msgSend)(all, sel("count"));
  pthread_mutex_lock(&lock);
  for (unsigned i = 0; i < count; i++) {
    id touch = ((id(*)(id, SEL, unsigned))objc_msgSend)(all, sel("objectAtIndex:"), i);
    Spot at = ((Spot(*)(id, SEL, id))objc_msgSend_stret)(touch, sel("locationInView:"), self);
    int phase = ((int (*)(id, SEL))objc_msgSend)(touch, sel("phase")); // began, moved, stationary, ended, cancelled
    pocket_contact_event(&shared.touches, phase == 0 ? POCKET_TOUCH_DOWN : phase == 3 ? POCKET_TOUCH_UP : phase == 4 ? POCKET_TOUCH_CANCEL : POCKET_TOUCH_MOVE,
                         (int)((uintptr_t)touch >> 4 & 0x3fffffff), at.y, HEIGHT - at.x, WIDTH, HEIGHT);
  }
  pthread_mutex_unlock(&lock);
}
static Class layer_class(id self, SEL _cmd) {
  (void)self, (void)_cmd;
  return objc_getClass("CAEAGLLayer");
}
static void active(id self, SEL _cmd, id application) {
  (void)self, (void)application;
  pthread_mutex_lock(&lock);
  shared.active = _cmd == sel("applicationDidBecomeActive:");
  pocket_contacts_cancel(&shared.touches);
  pthread_cond_broadcast(&changed);
  while (!shared.active && !shared.parked)
    pthread_cond_wait(&changed, &lock);
  pthread_mutex_unlock(&lock);
}
static BOOL launched(id self, SEL _cmd, id application, id options) {
  (void)self, (void)_cmd, (void)options;
  snprintf(bundle, sizeof bundle, "%s", ((const char *(*)(id, SEL))objc_msgSend)(send(send(cls("NSBundle"), "mainBundle"), "bundlePath"), sel("UTF8String")));
  snprintf(tmp, sizeof tmp, "%s/tmp", getenv("HOME"));
  snprintf(documents, sizeof documents, "%s/Documents", getenv("HOME"));

  // PocketJS's link stubs carry no UIKit version, and UIKit gives an app that
  // old one pixel per point: the layer is 320 by 480 pixels.
  id window = ((id(*)(id, SEL, Frame))objc_msgSend)(send(cls("UIWindow"), "alloc"), sel("initWithFrame:"), (Frame){0, 0, HEIGHT, WIDTH});
  view = ((id(*)(id, SEL, Frame))objc_msgSend)(send(cls("TokyoView"), "alloc"), sel("initWithFrame:"), (Frame){0, 0, HEIGHT, WIDTH});
  send_id(window, "addSubview:", view);
  send_int(view, "setMultipleTouchEnabled:", 1);
  send_int(send(view, "layer"), "setOpaque:", 1);
  context = ((id(*)(id, SEL, int))objc_msgSend)(send(cls("EAGLContext"), "alloc"), sel("initWithAPI:"), 2);
  send_id(cls("EAGLContext"), "setCurrentContext:", context);
  GLuint depth;
  glGenFramebuffers(1, &framebuffer);
  glBindFramebuffer(GL_FRAMEBUFFER, framebuffer);
  glGenRenderbuffers(1, &colorbuffer);
  glBindRenderbuffer(GL_RENDERBUFFER, colorbuffer);
  ((BOOL(*)(id, SEL, unsigned, id))objc_msgSend)(context, sel("renderbufferStorage:fromDrawable:"), GL_RENDERBUFFER, send(view, "layer"));
  glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_RENDERBUFFER, colorbuffer);
  glGenRenderbuffers(1, &depth);
  glBindRenderbuffer(GL_RENDERBUFFER, depth);
  glRenderbufferStorage(GL_RENDERBUFFER, GL_DEPTH_COMPONENT24, HEIGHT, WIDTH); // OES_depth24
  glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_DEPTH_ATTACHMENT, GL_RENDERBUFFER, depth);
  if (!context || glCheckFramebufferStatus(GL_FRAMEBUFFER) != GL_FRAMEBUFFER_COMPLETE)
    snprintf(failure, sizeof failure, "OpenGL ES 2 is not available");
  else if (SAMPLES > 1) {
    GLuint buffers[2];
    glGenFramebuffers(1, &scene_target);
    glBindFramebuffer(GL_FRAMEBUFFER, scene_target);
    glGenRenderbuffers(2, buffers);
    glBindRenderbuffer(GL_RENDERBUFFER, buffers[0]);
    glRenderbufferStorageMultisampleAPPLE(GL_RENDERBUFFER, SAMPLES, RGBA8_OES, HEIGHT, WIDTH);
    glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_RENDERBUFFER, buffers[0]);
    glBindRenderbuffer(GL_RENDERBUFFER, buffers[1]);
    glRenderbufferStorageMultisampleAPPLE(GL_RENDERBUFFER, SAMPLES, GL_DEPTH_COMPONENT24, HEIGHT, WIDTH);
    glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_DEPTH_ATTACHMENT, GL_RENDERBUFFER, buffers[1]);
    if (glCheckFramebufferStatus(GL_FRAMEBUFFER) != GL_FRAMEBUFFER_COMPLETE) {
      glDeleteFramebuffers(1, &scene_target);
      scene_target = 0;
    }
  }
  send_id(cls("EAGLContext"), "setCurrentContext:", NULL);
  send(window, "makeKeyAndVisible");
  send_int(application, "setIdleTimerDisabled:", 1);
  // The guest's parser recurses: give its thread the main thread's megabyte.
  pthread_t thread;
  pthread_attr_t attributes;
  pthread_attr_init(&attributes);
  pthread_attr_setstacksize(&attributes, 1 << 20);
  pthread_create(&thread, &attributes, render, NULL);
  return 1;
}
int main(int argc, char **argv) {
  send(send(cls("NSAutoreleasePool"), "alloc"), "init");
  Class surface = objc_allocateClassPair(objc_getClass("UIView"), "TokyoView", 0);
  class_addMethod(object_getClass((id)surface), sel("layerClass"), (IMP)layer_class, "#@:");
  static const char *const touches[] = {"touchesBegan:withEvent:", "touchesMoved:withEvent:", "touchesEnded:withEvent:",
                                        "touchesCancelled:withEvent:"};
  for (unsigned i = 0; i < 4; i++)
    class_addMethod(surface, sel(touches[i]), (IMP)touched, "v@:@@");
  objc_registerClassPair(surface);
  Class delegate = objc_allocateClassPair(objc_getClass("NSObject"), "TokyoDelegate", 0);
  class_addMethod(delegate, sel("application:didFinishLaunchingWithOptions:"), (IMP)launched, "c@:@@");
  class_addMethod(delegate, sel("applicationWillResignActive:"), (IMP)active, "v@:@");
  class_addMethod(delegate, sel("applicationDidBecomeActive:"), (IMP)active, "v@:@");
  objc_registerClassPair(delegate);
  return UIApplicationMain(argc, argv, NULL, string("TokyoDelegate"));
}
