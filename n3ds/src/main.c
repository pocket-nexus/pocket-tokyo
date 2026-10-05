/* Pocket Tokyo on Nintendo 3DS.
 *
 * The same city as the Vita build, lowered by the city compiler for this
 * machine (profiles/n3ds30.json): 400 x 240 on the upper screen at 30 frames
 * a second, the PICA200's vertex programs and fixed combiners, one Circle Pad.
 * What moves and what a frame draws is the Rust core (core/).
 *
 * Controls: the Circle Pad flies (ahead and back, turning left and right),
 * X and B look up and down, L and R go down and up, Y flies faster. Left and
 * right on the +Control Pad turn the clock; up and down set how fast it runs.
 * START returns to the tour. L + R + START leaves.
 *
 * Development loop over PocketJS's paired LAN wire (port 8131, compiled in
 * from vendor/pocketjs/hosts/3ds): {"t":"tokyo.control","text":"..."} steers
 * the run and is answered with a tokyo.status record; "screenshot" captures
 * both screens; a .3dsx install replaces this program. In an emulator,
 * sdmc:/pocket-tokyo/boot.txt holds control words, plus shot=N (write frame N
 * to shot.bgr there) and exit=N. */
#include <3ds.h>
#include <citro3d.h>
#include <math.h>
#include <pocket3d_title.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>

#include "core.h"
#include "devserver.h"
#include "hbldr.h"
#include "native.h"
#include "render.h"
#include "soc.h"

unsigned int __stacksize__ = 256 * 1024;
extern int __system_argc;
extern char **__system_argv;

#define TAG(a, b, c, d) ((u32)(a) | ((u32)(b) << 8) | ((u32)(c) << 16) | ((u32)(d) << 24))
/* Triangles a frame may draw: the distances of the levels of detail follow it. */
#define BUDGET 60000

static const u32 transfer = GX_TRANSFER_FLIP_VERT(0) | GX_TRANSFER_OUT_TILED(0) | GX_TRANSFER_RAW_COPY(0) | GX_TRANSFER_IN_FORMAT(GX_TRANSFER_FMT_RGBA8) | GX_TRANSFER_OUT_FORMAT(GX_TRANSFER_FMT_RGB8) |
                            GX_TRANSFER_SCALING(GX_TRANSFER_SCALE_NO);

static const char *stage = "boot";
static char app_error[256];
static FILE *boot_log;

static void flush_console(void) { GSPGPU_FlushDataCache(gfxGetFramebuffer(GFX_BOTTOM, GFX_LEFT, NULL, NULL), 320 * 240 * 2); }

static void say(const char *message) {
  printf("%s\n", message);
  flush_console();
  if (boot_log) {
    fprintf(boot_log, "%s\n", message);
    fflush(boot_log);
  }
  devserver_report_log("info", message);
}

/* ---------------------------------------------------------------- the pack */

typedef struct {
  u32 tag, offset, size, zero;
} Section;

static FILE *pack;
static Section sections[32];
static u32 section_count, pack_bytes;

static const Section *section(u32 tag) {
  for (u32 i = 0; i < section_count; i++)
    if (sections[i].tag == tag)
      return &sections[i];
  return NULL;
}

/* Reads a whole section into memory from `alloc` (malloc or linearAlloc). */
static void *read_section(u32 tag, void *(*alloc)(size_t), u32 *size) {
  const Section *s = section(tag);
  if (!s)
    return NULL;
  void *p = alloc(s->size ? s->size : 1);
  if (!p)
    return NULL;
  if (fseek(pack, s->offset, SEEK_SET) != 0 || fread(p, 1, s->size, pack) != s->size)
    return NULL;
  if (size)
    *size = s->size;
  return p;
}

static bool open_pack(void) {
  pack = fopen("romfs:/city.pack", "rb");
  if (!pack) {
    snprintf(app_error, sizeof app_error, "romfs:/city.pack is missing");
    return false;
  }
  u32 head[4];
  if (fread(head, 4, 4, pack) != 4 || head[0] != TAG('T', 'K', 'P', 'K') || head[1] != 3 || head[2] > 32 || fread(sections, sizeof(Section), head[2], pack) != head[2]) {
    snprintf(app_error, sizeof app_error, "city.pack is not a version 3 pack");
    return false;
  }
  section_count = head[2];
  fseek(pack, 0, SEEK_END);
  pack_bytes = ftell(pack);
  return true;
}

static void *linear(size_t n) { return linearAlloc(n); }

static const TkNearCell *near_cells;
static u16 *swept;
static u32 cell_count;

static bool load(void) {
  u32 sizes[8];
  tk_sizes(sizes);
  if (sizes[0] != sizeof(TkCity) || sizes[1] != sizeof(TkBatch) || sizes[2] != sizeof(TkNearCell) || sizes[3] != sizeof(TkItem) || sizes[4] != sizeof(TkView) || sizes[5] != sizeof(TkSlot) || sizes[6] != TK_SLOTS ||
      sizes[7] != TK_DOME_VERTS) {
    snprintf(app_error, sizeof app_error, "core.h does not match the core library");
    return false;
  }
  if (!open_pack())
    return false;
  static RenderData data;
  static TkPack p;
  u32 size = 0, pictures = 0, lamp_bytes = 0;
  say("Reading the city");
  p.city = read_section(TAG('C', 'I', 'T', 'Y'), malloc, &size);
  p.regions = read_section(TAG('R', 'E', 'G', 'N'), malloc, &size);
  p.region_count = size / 24;
  p.blocks = read_section(TAG('B', 'L', 'C', 'K'), malloc, &size);
  p.block_count = size / 24;
  p.cells = read_section(TAG('C', 'E', 'L', 'L'), malloc, &size);
  p.cell_count = cell_count = size / 32;
  p.batches = read_section(TAG('B', 'T', 'C', 'H'), malloc, &size);
  p.batch_count = size / sizeof(TkBatch);
  p.spans = read_section(TAG('S', 'P', 'A', 'N'), malloc, &size);
  p.span_count = size / 4;
  p.tour = read_section(TAG('T', 'O', 'U', 'R'), malloc, &size);
  p.tour_count = size / 24;
  p.heights = read_section(TAG('H', 'M', 'A', 'P'), malloc, NULL);
  p.near = near_cells = read_section(TAG('N', 'C', 'E', 'L'), malloc, NULL);
  p.meta = read_section(TAG('M', 'E', 'T', 'A'), malloc, &p.meta_len);
  p.landmarks = read_section(TAG('L', 'A', 'N', 'D'), malloc, &size);
  p.landmark_count = p.landmarks ? size / 64 : 0;
  const Section *near = section(TAG('N', 'E', 'A', 'R'));
  if (!p.city || !p.regions || !p.blocks || !p.cells || !p.batches || !p.spans || !p.tour || !p.heights || !p.near || !p.meta || !near) {
    snprintf(app_error, sizeof app_error, "the pack lacks a table, or memory is short");
    return false;
  }
  p.near_offset = near->offset;
  say("Reading the levels that stay");
  data.vtx[0] = read_section(TAG('V', 'T', 'O', 'P'), linear, NULL);
  data.vtx[1] = read_section(TAG('V', 'W', 'A', 'L'), linear, NULL);
  data.vtx[2] = read_section(TAG('V', 'S', 'O', 'L'), linear, NULL);
  data.idx = read_section(TAG('I', 'D', 'X', '0'), linear, &size);
  if (!data.vtx[0] || !data.vtx[1] || !data.vtx[2] || !data.idx) {
    snprintf(app_error, sizeof app_error, "no linear memory for the city's vertices");
    return false;
  }
  GSPGPU_FlushDataCache(data.idx, size);
  say("Reading the pictures");
  data.pictures = read_section(TAG('H', 'P', 'I', 'C'), malloc, &pictures);
  u8 *texels = read_section(TAG('H', 'T', 'E', 'X'), linear, NULL);
  u8 *lamps = read_section(TAG('L', 'A', 'M', 'P'), linear, &lamp_bytes);
  if (!data.pictures || !texels || !lamps || lamp_bytes != MAP_SIDE * MAP_SIDE * 2 || p.city->grid_w > MAP_SIDE || p.city->grid_h > MAP_SIDE) {
    snprintf(app_error, sizeof app_error, "the pack's pictures are not a 3DS pack's");
    return false;
  }
  data.city = p.city;
  data.batches = p.batches;
  data.near = p.near;
  data.picture_count = pictures / sizeof(TkPicture);
  data.block_count = p.block_count;
  data.texels = texels;
  data.lamps = lamps;
  for (u32 i = 0; i < cell_count; i++)
    if (near_cells[i].size > data.slot_bytes)
      data.slot_bytes = near_cells[i].size;
  /* The vertex sections are flushed whole: the GPU reads them as they are. */
  for (int k = 0; k < 3; k++) {
    const Section *s = section(k == 0 ? TAG('V', 'T', 'O', 'P') : k == 1 ? TAG('V', 'W', 'A', 'L') : TAG('V', 'S', 'O', 'L'));
    if (s->size)
      GSPGPU_FlushDataCache(data.vtx[k], s->size);
  }
  say("Handing the pictures to the GPU");
  if (!render_init(&data, app_error, sizeof app_error))
    return false;
  say("Starting the flight");
  /* The textures hold their own copies now. */
  linearFree(texels);
  linearFree(lamps);
  swept = malloc(p.city->grid_w * p.city->grid_h * 2);
  /* (the tables are in memory; the cells are the reading thread's, through a handle of its own. A file left open
   * on the ROMFS is closed by the C library at exit, after the ROMFS is gone.) */
  fclose(pack);
  pack = NULL;
  const char *e = tk_init(&p, BUDGET);
  if (e || !swept) {
    snprintf(app_error, sizeof app_error, "%s", e ? e : "no memory for the shadows");
    return false;
  }
  return true;
}

/* ---------------------------------------------------------------- the thread that reads cells and sweeps shadows
 *
 * It runs below the frame's priority, in the time the frame waits for the GPU or the display. A cell's near
 * level is one record of the pack, read whole into its slot. When no cell is wanted and the sun has moved a
 * degree, the shadows' texture is drawn anew. */
static volatile float want_sun[3], want_shade;
static volatile bool worker_quit;
static volatile u32 cells_read, shadows_swept, read_ms_total, sweep_ms_last;

static void worker(void *arg) {
  (void)arg;
  FILE *f = fopen("romfs:/city.pack", "rb");
  TkSlot *slots = tk_slots();
  float done_sun[3] = {0, 0, 0}, done_shade = -1.0f;
  while (!worker_quit && f) {
    int pick = -1;
    for (int i = 0; i < TK_SLOTS; i++)
      if (slots[i].state == TK_WANTED && (pick < 0 || slots[i].rank < slots[pick].rank))
        pick = i;
    if (pick >= 0) {
      TkSlot *s = &slots[pick];
      u64 t0 = svcGetSystemTick();
      if (fseek(f, s->offset, SEEK_SET) == 0 && fread(render_slot(pick), 1, s->size, f) == s->size) {
        render_slot_arrived(pick, &near_cells[s->cell]);
        cells_read++;
        read_ms_total += (u32)((svcGetSystemTick() - t0) * 1000 / SYSCLOCK_ARM11);
        s->state = TK_READY;
      } else {
        svcSleepThread(200000000);
      }
      continue;
    }
    float sun[3] = {want_sun[0], want_sun[1], want_sun[2]}, shade = want_shade;
    bool lit = sun[0] != 0.0f || sun[1] != 0.0f || sun[2] != 0.0f;
    /* (the cosine of one degree; and the shade is written into the texture, so a change of it counts too) */
    if (lit && (sun[0] * done_sun[0] + sun[1] * done_sun[1] + sun[2] * done_sun[2] < 0.99985f || fabsf(shade - done_shade) > 0.03f)) {
      u64 t0 = svcGetSystemTick();
      tk_shadows(sun, shade, swept, render_shadows(), MAP_SIDE);
      GSPGPU_FlushDataCache(render_shadows(), MAP_SIDE * MAP_SIDE);
      memcpy(done_sun, sun, sizeof sun);
      done_shade = shade;
      shadows_swept++;
      sweep_ms_last = (u32)((svcGetSystemTick() - t0) * 1000 / SYSCLOCK_ARM11);
      continue;
    }
    svcSleepThread(5000000);
  }
  if (f)
    fclose(f);
}

/* ---------------------------------------------------------------- the wire */

static TkPerf perf;
static float cpu_ms, gpu_ms;
static u32 frame_no;
static bool new3ds;

static int status_text(char *line, size_t cap) {
  static char body[2048], extra[512];
  int n = snprintf(extra, sizeof extra,
                   "\"build\":\"" TOKYO_BUILD_ID "\",\"phase\":\"%s\",\"error\":\"%s\",\"packBytes\":%lu,\"linearFree\":%lu,\"vramFree\":%lu,\"new3ds\":%s,\"read\":{\"cells\":%lu,\"ms\":%lu},\"shadows\":{\"sweeps\":%lu,\"ms\":%lu}", stage,
                   app_error, (unsigned long)pack_bytes, (unsigned long)linearSpaceFree(), (unsigned long)vramSpaceFree(), new3ds ? "true" : "false", (unsigned long)cells_read, (unsigned long)read_ms_total, (unsigned long)shadows_swept,
                   (unsigned long)sweep_ms_last);
  if (!strcmp(stage, "running")) {
    tk_status(body, sizeof body, &perf, extra, n);
    /* {"target":... becomes {"t":"tokyo.status","target":... */
    return snprintf(line, cap, "{\"t\":\"tokyo.status\",%s", body + 1);
  }
  return snprintf(line, cap, "{\"t\":\"tokyo.status\",\"target\":\"3ds\",\"stage\":\"%s\",%s}", stage, extra);
}

static void report_status(void) {
  static char line[2800];
  int n = status_text(line, sizeof line);
  devserver_send_ctrl(line, n);
}

/* The "text" member of a control line: words for the flight, no escapes. */
static void controls(const char *line) {
  if (!strstr(line, "\"tokyo.control\""))
    return;
  const char *t = strstr(line, "\"text\":\"");
  if (t && !strcmp(stage, "running")) {
    t += 8;
    const char *end = strchr(t, '"');
    if (end)
      tk_control(t, end - t);
  }
  report_status();
}

static bool capture(C3D_RenderTarget *target) {
  uint8_t *top = NULL, *bottom = NULL;
  unsigned width = target->frameBuf.height, height = target->frameBuf.width;
  if (!devserver_screenshot_begin(frame_no, width, height, 320, 240, &top, &bottom))
    return false;
  C3D_SyncDisplayTransfer(target->frameBuf.colorBuf, GX_BUFFER_DIM(height, width), (u32 *)top, GX_BUFFER_DIM(height, width), transfer);
  GSPGPU_InvalidateDataCache(top, width * height * 3);
  u16 w, h;
  const uint16_t *fb = (const uint16_t *)gfxGetFramebuffer(GFX_BOTTOM, GFX_LEFT, &w, &h);
  if (w != 240 || h != 320) {
    devserver_screenshot_cancel();
    return false;
  }
  /* The console is RGB565; the wire carries BGR8 for both screens. */
  for (unsigned i = 0; i < 320 * 240; i++) {
    unsigned p = fb[i];
    bottom[3 * i] = (p & 31) * 255 / 31;
    bottom[3 * i + 1] = ((p >> 5) & 63) * 255 / 63;
    bottom[3 * i + 2] = ((p >> 11) & 31) * 255 / 31;
  }
  return true;
}

/* An emulator run: the upper screen as it is in the render target, to the memory card. */
static void write_shot(C3D_RenderTarget *target) {
  unsigned width = target->frameBuf.height, height = target->frameBuf.width;
  u8 *out = linearAlloc(width * height * 3);
  if (!out)
    return;
  C3D_SyncDisplayTransfer(target->frameBuf.colorBuf, GX_BUFFER_DIM(height, width), (u32 *)out, GX_BUFFER_DIM(height, width), transfer);
  GSPGPU_InvalidateDataCache(out, width * height * 3);
  FILE *f = fopen("sdmc:/pocket-tokyo/shot.bgr", "wb");
  if (f) {
    fwrite(out, 1, width * height * 3, f);
    fclose(f);
  }
  linearFree(out);
}

static void read_pad(TkPad *pad) {
  u32 held = hidKeysHeld();
  circlePosition c;
  hidCircleRead(&c);
  pad->buttons = (held & KEY_Y ? TK_FAST : 0) | (held & (KEY_R | KEY_ZR) ? TK_UP : 0) | (held & (KEY_L | KEY_ZL) ? TK_DOWN : 0);
  pad->keys = (held & KEY_START ? TK_TOUR : 0) | (held & KEY_SELECT ? TK_STATS : 0) | (held & KEY_DRIGHT ? TK_LATER : 0) | (held & KEY_DLEFT ? TK_EARLIER : 0) | (held & KEY_DUP ? TK_FASTER : 0) | (held & KEY_DDOWN ? TK_SLOWER : 0);
  /* The Circle Pad reaches about 150 units; nothing inside a sixth of that. One stick: it flies ahead and turns. */
  float x = fmaxf(-1.0f, fminf(1.0f, c.dx / 150.0f)), y = fmaxf(-1.0f, fminf(1.0f, c.dy / 150.0f));
  pad->lx = 0.0f;
  pad->ly = fabsf(y) < 0.16f ? 0.0f : y;
  pad->rx = fabsf(x) < 0.16f ? 0.0f : x;
  pad->ry = (held & KEY_X ? 0.7f : 0.0f) - (held & KEY_B ? 0.7f : 0.0f);
}

/* The lower screen: the place and the hour, and the frame in numbers. */
static void lower_text(const TkView *view) {
  char title[96];
  tk_title(title, sizeof title);
  printf("\x1b[2;2HPOCKET TOKYO\x1b[K\n\n  %s\x1b[K\n\n  %4.1f fps  %4.1f ms\x1b[K\n  cpu %4.1f  gpu %4.1f\x1b[K\n  %luk triangles  %lu draws\x1b[K\n  near %3.0f m  mid %4.0f m\x1b[K\n\n  Circle Pad: fly   X B: look\x1b[K\n  L R: down, up   Y: fast\x1b[K\n  +Pad: the clock   START: tour\x1b[K\n",
         title, 1000.0f / fmaxf(perf.frame, 0.1f), perf.frame, cpu_ms, gpu_ms, (unsigned long)((perf.tris[0] + perf.tris[1] + perf.tris[2] + perf.tris[3]) / 1000), (unsigned long)perf.draws, view->near, view->mid);
  flush_console();
}

int main(void) {
  gfxInitDefault();
  gfxSet3D(false);
  mkdir("sdmc:/pocket-tokyo", 0777);
  /* Test runs: control words for the start, title=0, shot=N, exit=N. */
  static char boot[512];
  FILE *bf = fopen("sdmc:/pocket-tokyo/boot.txt", "r");
  if (bf) {
    size_t n = fread(boot, 1, sizeof boot - 1, bf);
    boot[n] = 0;
    fclose(bf);
  }
  long shot_at = -1, exit_at = -1;
  const char *w;
  if ((w = strstr(boot, "shot=")))
    shot_at = atol(w + 5);
  if ((w = strstr(boot, "exit=")))
    exit_at = atol(w + 5);
  /* The Pocket3D title card, on both screens, before the GPU is set up. */
  if (!strstr(boot, "title=0"))
    pocket3d_title_play();
  gfxSetDoubleBuffering(GFX_BOTTOM, false);
  consoleInit(GFX_BOTTOM, NULL);
  boot_log = fopen("sdmc:/pocket-tokyo/boot.log", "w");
  say("Pocket Tokyo " TOKYO_BUILD_ID);
  char error[256] = {0};
  PocketRuntimeState state = {0};
  if (__system_argc > 0 && __system_argv)
    native_set_running_path(__system_argv[0]);
  mkdir("sdmc:/pocketjs", 0777);
  mkdir("sdmc:/pocketjs/runtime", 0777);
  devserver_allow_packages(false);
  DevserverInitResult dev = devserver_init(&state, error, sizeof error);
  say(dev == DEVSERVER_READY ? "Remote debugger ready" : error);
  /* A New 3DS runs at 804 MHz when asked; an Old 3DS ignores it. */
  osSetSpeedupEnable(true);
  APT_CheckNew3DS(&new3ds);
  /* (a frame is a few hundred draws, each with a matrix of its own: four times the usual command buffer) */
  bool gpu_ok = C3D_Init(C3D_DEFAULT_CMDBUF_SIZE * 4), gpu_stalled = false;
  C3D_RenderTarget *top = gpu_ok ? C3D_RenderTargetCreate(240, 400, GPU_RB_RGBA8, GPU_RB_DEPTH24_STENCIL8) : NULL;
  if (top)
    C3D_RenderTargetSetOutput(top, GFX_TOP, GFX_LEFT, transfer);
  Result romfs = romfsInit();
  bool loaded = false;
  stage = "loading";
  devserver_set_runtime(&state, NULL, stage, 0);
  devserver_poll();
  if (!gpu_ok || !top)
    snprintf(app_error, sizeof app_error, "PICA initialization failed");
  else if (R_FAILED(romfs))
    snprintf(app_error, sizeof app_error, "romfsInit failed: %08lx", (unsigned long)romfs);
  else
    loaded = load();
  Thread reader = NULL;
  if (loaded) {
    /* From here the pack is the reading thread's. */
    s32 priority = 0x30;
    svcGetThreadPriority(&priority, CUR_THREAD_HANDLE);
    reader = threadCreate(worker, NULL, 32 * 1024, priority + 1, -2, false);
    if (boot[0])
      tk_control(boot, strlen(boot));
    stage = "running";
    consoleClear();
  } else {
    stage = "load-error";
    say(app_error);
  }
  devserver_set_runtime(&state, NULL, stage, 0);
  u64 last = svcGetSystemTick(), next_retry = 0;
  u32 ticks = 2, late = 0, last_count = C3D_FrameCounter(0);
  float worst = 0, avg = 33.3f;
  bool shot = false, capture_pending = false;
  while (aptMainLoop()) {
    hidScanInput();
    u32 held = hidKeysHeld();
    if ((held & (KEY_L | KEY_R | KEY_START)) == (KEY_L | KEY_R | KEY_START))
      break;
    if (!capture_pending)
      devserver_poll();
    u64 now = svcGetSystemTick();
    if (dev != DEVSERVER_READY && now >= next_retry) {
      dev = devserver_init(&state, error, sizeof error);
      next_retry = now + SYSCLOCK_ARM11 * 3ULL;
    }
    char launch[POCKET_RUNTIME_NATIVE_NAME_BYTES + 1], path[POCKET_NATIVE_PATH_BYTES];
    if (devserver_take_launch(launch) && native_path_for(launch, path)) {
      if (hbldr_launch_on_exit(path, error, sizeof error)) {
        devserver_flush(1000);
        devserver_report_native("launching", launch, "exiting to start it");
        devserver_poll();
        devserver_flush(1000);
        for (unsigned i = 0; i < 200; i++) {
          devserver_poll();
          svcSleepThread(1000000);
        }
        break;
      }
      devserver_report_native("launch-error", launch, error);
    }
    static char control[8192];
    size_t n;
    while ((n = devserver_recv_ctrl(control, sizeof control - 1)) > 0) {
      control[n] = 0;
      char *save = NULL;
      for (char *line = strtok_r(control, "\n", &save); line; line = strtok_r(NULL, "\n", &save)) {
        if (strstr(line, "\"screenshot\""))
          devserver_request_screenshot();
        controls(line);
      }
    }
    if (native_receiving() || gpu_stalled || !loaded) {
      /* (an emulator run that did not load ends here, with its error in boot.log) */
      if (exit_at >= 0 && !loaded)
        break;
      svcSleepThread(1000000);
      continue;
    }
    if (exit_at >= 0 && frame_no < 3)
      say(frame_no == 0 ? "Frame 0" : frame_no == 1 ? "Frame 1" : "Frame 2");
    float frame_ms = (float)(now - last) * 1000.0f / SYSCLOCK_ARM11;
    last = now;

    /* ------------------------------------------------------------ input, the flight, the frame's draws */
    TkPad pad;
    read_pad(&pad);
    tk_step(&pad, ticks);
    TkView view;
    tk_view(&view);
    want_shade = view.shade;
    for (int k = 0; k < 3; k++)
      want_sun[k] = view.sun[k];
    /* (chosen while the GPU draws the frame before) */
    const TkItem *lists[TK_KINDS];
    u32 counts[TK_KINDS];
    tk_choose(lists, counts);
    if (exit_at >= 0 && frame_no < 2)
      say("Chosen");
    float chosen_ms = (float)(svcGetSystemTick() - now) * 1000.0f / SYSCLOCK_ARM11;

    /* ------------------------------------------------------------ the previous frame leaves the GPU */
    u64 wait_start = svcGetSystemTick();
    while (!C3D_FrameBegin(C3D_FRAME_NONBLOCK)) {
      if (!capture_pending)
        devserver_poll();
      svcSleepThread(100000);
      if (svcGetSystemTick() - wait_start > SYSCLOCK_ARM11 * 4ULL) {
        stage = "gpu-timeout";
        say("GPU timeout: the debugger remains available");
        devserver_set_runtime(&state, NULL, stage, 0);
        gpu_stalled = true;
        break;
      }
    }
    if (gpu_stalled)
      continue;
    gpu_ms = C3D_GetDrawingTime();
    if (capture_pending) {
      devserver_screenshot_ready();
      capture_pending = false;
    }
    if (shot) {
      capture_pending = capture(top);
      shot = false;
    }
    if (exit_at >= 0 && (long)frame_no >= exit_at) {
      static char line[2800];
      status_text(line, sizeof line);
      FILE *f = fopen("sdmc:/pocket-tokyo/status.json", "w");
      if (f) {
        fputs(line, f);
        fclose(f);
      }
      f = fopen("sdmc:/pocket-tokyo/done", "w");
      if (f)
        fclose(f);
      break;
    }
    /* Hold the pace: a frame is shown for `pace` refreshes. */
    while (C3D_FrameCounter(0) - last_count < view.pace) {
      if (!capture_pending)
        devserver_poll();
      svcSleepThread(100000);
    }
    u32 count = C3D_FrameCounter(0);
    ticks = count - last_count;
    last_count = count;
    if (ticks > view.pace)
      late++;
    if (ticks > view.pace + 1)
      ticks = view.pace + 1;
    avg += (frame_ms - avg) * 0.05f;
    worst = frame_no % 120 == 0 ? frame_ms : fmaxf(worst, frame_ms);

    /* ------------------------------------------------------------ this frame */
    u64 build_start = svcGetSystemTick();
    tk_refill();
    render_frame(top, &view, lists, counts);
    if (exit_at >= 0 && frame_no < 2)
      say("Drawn");
    C3D_FrameEnd(0);
    /* (an emulator run's picture: this frame, once the GPU has drawn it) */
    if ((long)frame_no == shot_at)
      write_shot(top);
    float build_ms = (float)(svcGetSystemTick() - build_start) * 1000.0f / SYSCLOCK_ARM11;
    cpu_ms = chosen_ms + build_ms;
    perf = (TkPerf){.frame = avg, .worst = worst, .late = late, .frames = frame_no, .cpu = cpu_ms, .gpu = gpu_ms, .draws = render_stats.draws, .tris = {render_stats.tris[0], render_stats.tris[1], render_stats.tris[2], render_stats.tris[3]}};
    tk_drew(perf.tris[0] + perf.tris[1] + perf.tris[2] + perf.tris[3]);
    frame_no++;
    devserver_set_runtime(&state, NULL, stage, frame_no);
    devserver_set_frame_stats(frame_no, render_stats.draws, (perf.tris[0] + perf.tris[1] + perf.tris[2] + perf.tris[3]) * 3, 0);
    devserver_set_frame_timing(0, (uint32_t)(chosen_ms * 1000), (uint32_t)(build_ms * 1000), (uint32_t)(gpu_ms * 1000), (uint32_t)(frame_ms * 1000));
    shot = shot || devserver_take_screenshot_request();
    if (frame_no % 15 == 7)
      lower_text(&view);
  }

  worker_quit = true;
  if (reader) {
    threadJoin(reader, 2000000000ULL);
    threadFree(reader);
  }
  if (gpu_ok && !gpu_stalled) {
    /* Retire the frame in flight before the targets go. */
    for (unsigned i = 0; i < 4000 && !C3D_FrameBegin(C3D_FRAME_NONBLOCK); i++) {
      devserver_poll();
      svcSleepThread(1000000);
    }
    C3D_FrameEnd(0);
    if (top)
      C3D_RenderTargetDelete(top);
    C3D_Fini();
  }
  if (!gpu_stalled || !capture_pending)
    devserver_shutdown();
  soc_shutdown();
  if (R_SUCCEEDED(romfs))
    romfsExit();
  if (native_exit_pending() && !native_finish_exit(error, sizeof error))
    say(error);
  if (boot_log)
    fclose(boot_log);
  gfxExit();
  return 0;
}
