// Pocket Tokyo on the Redmi 1S (Android 4.3, Adreno 305): the shell.
//
// One NativeActivity and one thread. It owns the window and its EGL surface,
// the touches, the PocketJS guest that is the interface (QuickJS, PocketJS's UI
// core and its OpenGL ES draw-list backend) and the files a development host
// talks through. The city is the core's (../core, behind core.h): this file
// draws no part of it, and no text or control of its own.
//
// A launch: the Pocket3D title card, then the guest, then the pack a step a
// frame under the guest's loading screen, then frames. A frame: the flight's
// step, the guest's turn when it is worth one, the city straight into the
// window's buffer, the interface over it, eglSwapBuffers.
//
// The window's buffers are the panel's own pixels a quarter turn over; the
// driver draws turned and the display processor shows the buffer as an
// overlay, so showing a frame costs the GPU nothing.
#include <EGL/egl.h>
#include <GLES3/gl3.h>
#include <android/asset_manager.h>
#include <android/log.h>
#include <android/window.h>
#include <android_native_app_glue.h>
#include <fcntl.h>
#include <math.h>
#include <pthread.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

#include "contact_latch.h"
#include "core.h"
#include "pocket_runtime.h"

extern uint64_t ui_draw_hash(void);
extern int32_t ui_gl_render_over(int32_t x, int32_t y, int32_t width, int32_t height, int32_t window_width, int32_t window_height);
extern void svcwire_shutdown(void);

#define LOG(...) __android_log_print(ANDROID_LOG_INFO, "PocketTokyo", __VA_ARGS__)

// The interface's own pixels: tools/android.ts hands in the viewport its build resolved.
#ifndef LOGICAL_WIDTH
#define LOGICAL_WIDTH 640
#define LOGICAL_HEIGHT 360
#endif
#ifndef TOKYO_BUILD
#define TOKYO_BUILD "dev"
#endif
// Samples a pixel of the window, and the size of its buffers (0: the panel's own). A development
// run may ask for others in files/dev/boot.txt.
#ifndef SAMPLES
#define SAMPLES 0
#endif
#ifndef BUFFER_WIDTH
#define BUFFER_WIDTH 0
#define BUFFER_HEIGHT 0
#endif
// Turns the guest is offered a second.
#define TURN_HZ 30
#define REFRESH_MS (1000.0 / 60.0)
#define WINDOW 240

enum { CARD, GUEST, LOADING, RUNNING, FAILED };
static int stage = CARD;

static struct android_app *app;
static EGLDisplay display = EGL_NO_DISPLAY;
static EGLConfig config;
static EGLContext context = EGL_NO_CONTEXT;
static EGLSurface surface = EGL_NO_SURFACE;
static int width, height;   // the window's buffers
static int panel_w, panel_h; // the window on the panel: what a touch is measured in
static int samples = SAMPLES, buffer_w = BUFFER_WIDTH, buffer_h = BUFFER_HEIGHT;
static bool resumed, skip_card, development;
static char files[512], dev[560];

static PocketContactLatch touches;
static struct { int count; float at[4][2]; } fingers, fingered; // contacts a control message holds down
static int tap;

static bool guest;
static char failure[512];
static unsigned turns;
static int pack_fd = -1;
static off_t pack_offset, pack_length;

static TkPerf perf;
static float window_ms[WINDOW];
static unsigned frames;
static double debt; // milliseconds the frames shown are behind one a refresh
static bool profile, screen, hidden;
static float gpu_ms;
static char command[64], captured[64];
static struct { double until; unsigned frames, late; float worst; double sum; unsigned done_frames, done_late; float done_worst, done_mean; } mark;

// ---------------------------------------------------------------- files

static double now(void) {
  struct timespec t;
  clock_gettime(CLOCK_MONOTONIC, &t);
  return (double)t.tv_sec + (double)t.tv_nsec * 1e-9;
}

static void write_file(const char *directory, const char *name, const void *data, size_t size) {
  char path[700], temporary[720];
  snprintf(path, sizeof path, "%s/%s", directory, name);
  snprintf(temporary, sizeof temporary, "%s.part", path);
  FILE *file = fopen(temporary, "wb");
  if (!file)
    return;
  fwrite(data, 1, size, file);
  fclose(file);
  rename(temporary, path);
}

// A file of the app: the development copy in files/dev when there is one, otherwise the package's own.
// `before` goes in front of it; the result is NUL-terminated.
static char *read_whole(const char *name, const char *before, size_t *size) {
  char path[700];
  size_t lead = strlen(before);
  snprintf(path, sizeof path, "%s/%s", dev, name);
  FILE *file = fopen(path, "rb");
  if (file) {
    fseek(file, 0, SEEK_END);
    long length = ftell(file);
    fseek(file, 0, SEEK_SET);
    char *data = malloc(lead + (size_t)length + 1);
    memcpy(data, before, lead);
    if (fread(data + lead, 1, (size_t)length, file) != (size_t)length)
      length = 0;
    fclose(file);
    data[lead + (size_t)length] = 0;
    *size = lead + (size_t)length;
    return data;
  }
  AAsset *asset = AAssetManager_open(app->activity->assetManager, name, AASSET_MODE_BUFFER);
  if (!asset)
    return NULL;
  size_t length = (size_t)AAsset_getLength(asset);
  char *data = malloc(lead + length + 1);
  memcpy(data, before, lead);
  if (AAsset_read(asset, data + lead, length) != (int)length)
    length = 0;
  AAsset_close(asset);
  data[lead + length] = 0;
  *size = lead + length;
  return data;
}

// Where the pack is: a file of its own in files/dev, or the package's entry, which the package stores
// as it is so that it is read in place.
static bool open_pack(void) {
  char path[700];
  struct stat info;
  snprintf(path, sizeof path, "%s/city.pack", dev);
  pack_fd = open(path, O_RDONLY);
  if (pack_fd >= 0 && !fstat(pack_fd, &info)) {
    pack_offset = 0;
    pack_length = info.st_size;
    return true;
  }
  AAsset *asset = AAssetManager_open(app->activity->assetManager, "city.pack", AASSET_MODE_UNKNOWN);
  if (!asset)
    return false;
  pack_fd = AAsset_openFileDescriptor(asset, &pack_offset, &pack_length);
  AAsset_close(asset);
  return pack_fd >= 0;
}

static int read_number(const char *path) {
  FILE *file = fopen(path, "r");
  int value = -1;
  if (file) {
    if (fscanf(file, "%d", &value) != 1)
      value = -1;
    fclose(file);
  }
  return value;
}

// ---------------------------------------------------------------- the window

static bool make_surface(void) {
  if (display == EGL_NO_DISPLAY) {
    display = eglGetDisplay(EGL_DEFAULT_DISPLAY);
    eglInitialize(display, NULL, NULL);
    // Multisampling comes from the window's own config: the samples live in tile memory and are
    // resolved as a tile is stored.
    const EGLint want[] = {EGL_RENDERABLE_TYPE, 0x0040 /* EGL_OPENGL_ES3_BIT_KHR */, EGL_SURFACE_TYPE, EGL_WINDOW_BIT, EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8,
                           EGL_DEPTH_SIZE, 24, EGL_STENCIL_SIZE, 8, EGL_SAMPLE_BUFFERS, samples ? 1 : 0, EGL_SAMPLES, samples, EGL_NONE};
    EGLint count = 0;
    if (!eglChooseConfig(display, want, &config, 1, &count) || count < 1) {
      snprintf(failure, sizeof failure, "no OpenGL ES 3.0 window with %d samples", samples);
      return false;
    }
    panel_w = ANativeWindow_getWidth(app->window);
    panel_h = ANativeWindow_getHeight(app->window);
  }
  EGLint format = 0;
  eglGetConfigAttrib(display, config, EGL_NATIVE_VISUAL_ID, &format);
  // Buffers of another size than the panel's are scaled by the display processor.
  ANativeWindow_setBuffersGeometry(app->window, buffer_w, buffer_h, format);
  surface = eglCreateWindowSurface(display, config, app->window, NULL);
  if (context == EGL_NO_CONTEXT) {
    const EGLint version[] = {EGL_CONTEXT_CLIENT_VERSION, 3, EGL_NONE};
    context = eglCreateContext(display, config, EGL_NO_CONTEXT, version);
  }
  if (surface == EGL_NO_SURFACE || context == EGL_NO_CONTEXT || !eglMakeCurrent(display, surface, surface, context)) {
    snprintf(failure, sizeof failure, "no OpenGL ES 3.0 context (EGL 0x%04x)", eglGetError());
    return false;
  }
  eglSwapInterval(display, 1);
  eglQuerySurface(display, surface, EGL_WIDTH, &width);
  eglQuerySurface(display, surface, EGL_HEIGHT, &height);
  tk_window((uint32_t)width, (uint32_t)height);
  LOG("window %dx%d (panel %dx%d), %d samples, %s", width, height, panel_w, panel_h, samples, glGetString(GL_RENDERER));
  return true;
}

static void drop_surface(void) {
  if (display == EGL_NO_DISPLAY)
    return;
  eglMakeCurrent(display, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
  if (surface != EGL_NO_SURFACE)
    eglDestroySurface(display, surface);
  surface = EGL_NO_SURFACE;
}

static void on_command(struct android_app *a, int32_t what) {
  switch (what) {
  case APP_CMD_INIT_WINDOW:
    if (a->window && !make_surface())
      stage = FAILED;
    break;
  case APP_CMD_TERM_WINDOW:
    drop_surface();
    break;
  case APP_CMD_RESUME:
    resumed = true;
    break;
  case APP_CMD_PAUSE:
    resumed = false;
    pocket_contacts_cancel(&touches);
    break;
  case APP_CMD_LOST_FOCUS:
    pocket_contacts_cancel(&touches);
    break;
  }
}

// Fingers go to the interface: a contact per pointer, in the panel's pixels. The back key and the menu key
// go to the flow.
static int32_t on_input(struct android_app *a, AInputEvent *event) {
  (void)a;
  if (AInputEvent_getType(event) == AINPUT_EVENT_TYPE_MOTION) {
    int32_t action = AMotionEvent_getAction(event);
    int32_t what = action & AMOTION_EVENT_ACTION_MASK;
    size_t index = (size_t)((action & AMOTION_EVENT_ACTION_POINTER_INDEX_MASK) >> AMOTION_EVENT_ACTION_POINTER_INDEX_SHIFT);
    size_t count = AMotionEvent_getPointerCount(event);
    if (what == AMOTION_EVENT_ACTION_CANCEL) {
      pocket_contacts_cancel(&touches);
    } else if (what == AMOTION_EVENT_ACTION_MOVE) {
      for (size_t i = 0; i < count; i++)
        pocket_contact_event(&touches, POCKET_TOUCH_MOVE, AMotionEvent_getPointerId(event, i), AMotionEvent_getX(event, i), AMotionEvent_getY(event, i), panel_w, panel_h);
    } else if (index < count) {
      bool down = what == AMOTION_EVENT_ACTION_DOWN || what == AMOTION_EVENT_ACTION_POINTER_DOWN;
      bool up = what == AMOTION_EVENT_ACTION_UP || what == AMOTION_EVENT_ACTION_POINTER_UP;
      if (down || up)
        pocket_contact_event(&touches, down ? POCKET_TOUCH_DOWN : POCKET_TOUCH_UP, AMotionEvent_getPointerId(event, index), AMotionEvent_getX(event, index), AMotionEvent_getY(event, index),
                             panel_w, panel_h);
    }
    return 1;
  }
  if (AInputEvent_getType(event) == AINPUT_EVENT_TYPE_KEY) {
    int32_t key = AKeyEvent_getKeyCode(event);
    if (key != AKEYCODE_BACK && key != AKEYCODE_MENU)
      return 0;
    if (AKeyEvent_getAction(event) == AKEY_EVENT_ACTION_UP) {
      if (key == AKEYCODE_MENU) {
        static const char menu[] = "ui=menu";
        if (stage == RUNNING)
          tk_control(menu, sizeof menu - 1);
      } else if (stage != RUNNING || !tk_back()) {
        // Nothing to go back to: the app ends, as the system's own key would have it.
        ANativeActivity_finish(app->activity);
      }
    }
    return 1;
  }
  return 0;
}

// ---------------------------------------------------------------- a picture over the whole window

// The GPU's own time for a frame, where the driver counts it (EXT_disjoint_timer_query): a query around
// everything a frame draws, read two frames later. Unlike a wait for the GPU it leaves the frame's pace and
// the driver's choice of binning alone.
#define TIME_ELAPSED 0x88BF
static void (*query_begin)(GLenum, GLuint), (*query_end)(GLenum), (*query_get)(GLuint, GLenum, GLuint *);
static GLuint queries[4];
static float timer_ms;
static bool timers;

static void timer_start(void) {
  const char *extensions = (const char *)glGetString(GL_EXTENSIONS);
  void (*generate)(GLsizei, GLuint *) = (void (*)(GLsizei, GLuint *))eglGetProcAddress("glGenQueriesEXT");
  query_begin = (void (*)(GLenum, GLuint))eglGetProcAddress("glBeginQueryEXT");
  query_end = (void (*)(GLenum))eglGetProcAddress("glEndQueryEXT");
  query_get = (void (*)(GLuint, GLenum, GLuint *))eglGetProcAddress("glGetQueryObjectuivEXT");
  timers = extensions && strstr(extensions, "GL_EXT_disjoint_timer_query") && generate && query_begin && query_end && query_get;
  if (timers)
    generate(4, queries);
}

static GLuint cover_program, cover_array;

static void cover_start(void) {
  static const char *const sources[2] = {
    "#version 300 es\nin vec2 aPos; out highp vec2 vAt;\n"
    // (the picture's first row is its top one)
    "void main() { gl_Position = vec4(aPos, 0.0, 1.0); vAt = vec2(aPos.x * 0.5 + 0.5, 0.5 - aPos.y * 0.5); }\n",
    "#version 300 es\nprecision mediump float; uniform sampler2D uPicture; in highp vec2 vAt; out vec4 oColor;\n"
    "void main() { oColor = texture(uPicture, vAt); }\n"};
  static const GLfloat corners[] = {-1, -1, 3, -1, -1, 3};
  cover_program = glCreateProgram();
  for (unsigned i = 0; i < 2; i++) {
    GLuint shader = glCreateShader(i ? GL_FRAGMENT_SHADER : GL_VERTEX_SHADER);
    glShaderSource(shader, 1, &sources[i], NULL);
    glCompileShader(shader);
    glAttachShader(cover_program, shader);
    glDeleteShader(shader);
  }
  glBindAttribLocation(cover_program, 0, "aPos");
  glLinkProgram(cover_program);
  GLuint buffer;
  glGenVertexArrays(1, &cover_array);
  glBindVertexArray(cover_array);
  glGenBuffers(1, &buffer);
  glBindBuffer(GL_ARRAY_BUFFER, buffer);
  glBufferData(GL_ARRAY_BUFFER, sizeof corners, corners, GL_STATIC_DRAW);
  glEnableVertexAttribArray(0);
  glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, 0, 0);
  glBindVertexArray(0);
}

// The window's buffer as it stands, for the development host: its width and height, then RGBA rows from
// the bottom.
static void write_picture(const char *name) {
  uint8_t *pixels = malloc((size_t)width * (size_t)height * 4 + 8);
  memcpy(pixels, &width, 4);
  memcpy(pixels + 4, &height, 4);
  glReadPixels(0, 0, width, height, GL_RGBA, GL_UNSIGNED_BYTE, pixels + 8);
  write_file(files, name, pixels, (size_t)width * (size_t)height * 4 + 8);
  free(pixels);
}

static void show(void) {
  static const GLenum unused[] = {GL_DEPTH, GL_STENCIL};
  glInvalidateFramebuffer(GL_FRAMEBUFFER, 2, unused);
  eglSwapBuffers(display, surface);
}

// The Pocket3D title card, first at every launch and before anything of the city's is read or shown. The
// frames are PocketJS's (the core draws each tick's, `tk_card`); they reach the window through a texture,
// which is deleted when the card ends. The card follows the clock, so a slow frame skips a tick and the card
// still takes its 2.4 seconds. One frame of it per call.
static void card(void) {
  static uint8_t *pixels;
  static GLuint texture;
  static double start;
  static uint32_t shown = UINT32_MAX;
  if (!pixels) {
    pixels = malloc((size_t)width * (size_t)height * 4);
    glGenTextures(1, &texture);
    glBindTexture(GL_TEXTURE_2D, texture);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
    glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA8, width, height, 0, GL_RGBA, GL_UNSIGNED_BYTE, NULL);
    start = now();
  }
  uint32_t tick = (uint32_t)((now() - start) * 60.0);
  uint32_t drawn = skip_card ? 0 : tk_card(pixels, (uint32_t)width, (uint32_t)height, tick, shown);
  if (!drawn) {
    glDeleteTextures(1, &texture);
    free(pixels);
    pixels = NULL;
    stage = GUEST;
    return;
  }
  glActiveTexture(GL_TEXTURE0);
  glBindTexture(GL_TEXTURE_2D, texture);
  if (drawn == 1) {
    shown = tick;
    glPixelStorei(GL_UNPACK_ALIGNMENT, 4);
    glTexSubImage2D(GL_TEXTURE_2D, 0, 0, 0, width, height, GL_RGBA, GL_UNSIGNED_BYTE, pixels);
  }
  glViewport(0, 0, width, height);
  glDisable(GL_DEPTH_TEST);
  glDisable(GL_BLEND);
  glDisable(GL_SCISSOR_TEST);
  glClearColor(0, 0, 0, 1);
  glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT | GL_STENCIL_BUFFER_BIT);
  glUseProgram(cover_program);
  glBindVertexArray(cover_array);
  glDrawArrays(GL_TRIANGLES, 0, 3);
  glBindVertexArray(0);
  char want[700];
  snprintf(want, sizeof want, "%s/title.want", files);
  if (tick >= 72 && !access(want, F_OK)) {
    // A development run that left files/title.want gets the card's held frame, as presented.
    write_picture("title.rgba");
    unlink(want);
  }
  show();
}

// ---------------------------------------------------------------- the development host

// The status, written by a thread of its own: a file write on the render thread is a late frame.
static pthread_mutex_t report_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t report_due = PTHREAD_COND_INITIALIZER;
static char report[6000];
static bool report_new;

static void *reporter(void *unused) {
  (void)unused;
  static char text[6000];
  for (;;) {
    pthread_mutex_lock(&report_lock);
    while (!report_new)
      pthread_cond_wait(&report_due, &report_lock);
    report_new = false;
    size_t size = strlen(report);
    memcpy(text, report, size + 1);
    pthread_mutex_unlock(&report_lock);
    write_file(files, "status.json", text, size);
  }
  return NULL;
}

static void status(void) {
  static char extra[1500], text[6000];
  char memory[64] = "0";
  FILE *statm = fopen("/proc/self/statm", "r");
  if (statm) {
    unsigned long pages = 0, resident = 0;
    if (fscanf(statm, "%lu %lu", &pages, &resident) == 2)
      snprintf(memory, sizeof memory, "%lu", resident * 4096);
    fclose(statm);
  }
  static const char *const stages[] = {"card", "guest", "loading", "running", "failed"};
  snprintf(extra, sizeof extra,
           "\"build\":\"%s\",\"shell\":\"%s\",\"failure\":\"%s\",\"samples\":%d,\"panel\":[%d,%d],\"development\":%s,\"interface\":{\"up\":%s,\"error\":\"%s\",\"turnHz\":%d,\"turns\":%u,\"logical\":[%d,%d]},"
           "\"gpuMs\":%.2f,\"gpuTimerMs\":%.2f,\"profile\":%s,\"residentBytes\":%s,\"glError\":%u,\"gpuClock\":%d,\"thermal\":%d,\"command\":\"%s\",\"captured\":\"%s\","
           "\"mark\":{\"frames\":%u,\"late\":%u,\"worstMs\":%.2f,\"meanMs\":%.3f}",
           TOKYO_BUILD, stages[stage], failure, samples, panel_w, panel_h, development ? "true" : "false", guest ? "true" : "false", guest ? "" : pocket_runtime_error(), TURN_HZ, turns, LOGICAL_WIDTH,
           LOGICAL_HEIGHT, gpu_ms, timer_ms, profile ? "true" : "false", memory, glGetError(), read_number("/sys/class/kgsl/kgsl-3d0/gpuclk"), read_number("/sys/class/thermal/thermal_zone0/temp"), command,
           captured, mark.done_frames, mark.done_late, mark.done_worst, mark.done_mean);
  for (char *c = extra; *c; c++)
    if (*c == '\n' || *c == '\r' || *c == '\t')
      *c = ' ';
  uint32_t size = tk_status(text, sizeof text, &perf, extra, (uint32_t)strlen(extra));
  pthread_mutex_lock(&report_lock);
  memcpy(report, text, size + 1);
  report_new = true;
  pthread_cond_signal(&report_due);
  pthread_mutex_unlock(&report_lock);
}

// A word of a control message that is this shell's:
//   touch=X,Y[;X,Y…]  fingers held on the panel, in the interface's pixels; touch=off lifts them
//   tap=X,Y           one finger down for a few turns
//   screen=1          writes the next frame, as drawn, to files/screen.rgba (rows from the bottom)
//   interface=0       leaves the interface out of the frame (its turns go on), for measurements
//   profile=1         waits for the GPU inside every frame and times it (`gpuMs`); such frames are not a flight's
//   mark=SECONDS      measures that long (`mark` in the status)
static void host_word(const char *word) {
  if (!strncmp(word, "touch=", 6) || !strncmp(word, "tap=", 4)) {
    const char *at = strchr(word, '=') + 1;
    fingers.count = 0;
    while (fingers.count < 4) {
      char *end;
      float x = strtof(at, &end);
      if (end == at || *end != ',')
        break;
      float y = strtof(end + 1, &end);
      fingers.at[fingers.count][0] = x * (float)panel_w / LOGICAL_WIDTH;
      fingers.at[fingers.count][1] = y * (float)panel_h / LOGICAL_HEIGHT;
      fingers.count++;
      if (*end != ';')
        break;
      at = end + 1;
    }
    tap = word[1] == 'a' ? 4 : 0;
  } else if (!strcmp(word, "screen=1")) {
    screen = true;
  } else if (!strncmp(word, "profile=", 8)) {
    profile = word[8] == '1';
  } else if (!strncmp(word, "interface=", 10)) {
    hidden = word[10] == '0';
  } else if (!strncmp(word, "mark=", 5)) {
    memset(&mark, 0, sizeof mark);
    mark.until = now() + atof(word + 5);
  }
}

// Commands from the host (tools/android.ts ctl): a nonce on the first line, then words for this shell, the
// flow and the flight. The file is replaced in one step and acknowledged by its nonce in the status.
static void control(void) {
  char path[700];
  static char words[2048];
  snprintf(path, sizeof path, "%s/control.txt", files);
  FILE *file = fopen(path, "rb");
  if (!file)
    return;
  words[fread(words, 1, sizeof words - 1, file)] = 0;
  fclose(file);
  unlink(path);
  char *text = strchr(words, '\n');
  if (!text)
    return;
  *text++ = 0;
  snprintf(command, sizeof command, "%s", words);
  text[strcspn(text, "\r\n")] = 0;
  tk_control(text, (uint32_t)strlen(text));
  char *save = NULL;
  for (char *word = strtok_r(text, " ", &save); word; word = strtok_r(NULL, " ", &save))
    host_word(word);
}

// What a development run set for the launch: files/dev/boot.txt, words on one line
// (`samples=2 width=960 height=540 title=0`).
static void boot_words(void) {
  size_t size;
  char path[700];
  struct stat info;
  snprintf(path, sizeof path, "%s/boot.txt", dev);
  development = !stat(dev, &info);
  FILE *file = fopen(path, "rb");
  if (!file)
    return;
  static char words[512];
  size = fread(words, 1, sizeof words - 1, file);
  words[size] = 0;
  fclose(file);
  char *save = NULL;
  for (char *word = strtok_r(words, " \r\n", &save); word; word = strtok_r(NULL, " \r\n", &save)) {
    if (!strncmp(word, "samples=", 8))
      samples = atoi(word + 8);
    else if (!strncmp(word, "width=", 6))
      buffer_w = atoi(word + 6);
    else if (!strncmp(word, "height=", 7))
      buffer_h = atoi(word + 7);
    else if (!strcmp(word, "title=0"))
      skip_card = true;
  }
}

// ---------------------------------------------------------------- frames

// The guest's turn, when it is worth one: what the fingers are doing goes in, the flight's state is read
// and commands are left for the next tk_step.
static void turn(unsigned ticks, bool always) {
  static unsigned owed;
  owed += ticks;
  if (!guest || owed < 60 / TURN_HZ)
    return;
  PocketRuntimeContactsInput input;
  if (tap && !--tap)
    fingers.count = 0;
  for (int i = 0; i < 4; i++) {
    if (i < fingers.count)
      pocket_contact_event(&touches, i < fingered.count ? POCKET_TOUCH_MOVE : POCKET_TOUCH_DOWN, -1 - i, fingers.at[i][0], fingers.at[i][1], panel_w, panel_h);
    else if (i < fingered.count)
      pocket_contact_event(&touches, POCKET_TOUCH_UP, -1 - i, fingered.at[i][0], fingered.at[i][1], panel_w, panel_h);
  }
  fingered = fingers;
  pocket_contacts_sample(&touches, &input, panel_w, panel_h, LOGICAL_WIDTH, LOGICAL_HEIGHT, pocket_runtime_hit_test_bounds);
  // (a finger that has just lifted still has its end to deliver)
  bool touching = input.contact_count || input.cancelled_count;
  if (!always && !tk_guest_due(0, touching))
    return;
  unsigned elapsed = owed > 3 ? 3 : owed;
  owed = 0;
  turns++;
  input.buttons = 0;
  if (!pocket_runtime_frame_contacts(&input, elapsed)) {
    // The guest threw: the city goes on without it, and the status says why.
    guest = false;
    snprintf(failure, sizeof failure, "interface: %s", pocket_runtime_error());
    svcwire_shutdown();
  }
}

// The interface over what the frame holds.
static void interface_draw(void) {
  if (!guest || hidden)
    return;
  glBindVertexArray(0);
  glBindBuffer(GL_ELEMENT_ARRAY_BUFFER, 0);
  glUseProgram(0);
  glActiveTexture(GL_TEXTURE0);
  glDepthMask(GL_TRUE);
  ui_gl_render_over(0, 0, width, height, width, height);
}

static void frame(void) {
  static double previous, shown_at;
  static float sums[5];
  double start = now();
  if (!previous || start - previous > 0.5)
    previous = start - REFRESH_MS / 1000;
  // One tick of the flight per display refresh since the last frame: a late frame catches up.
  unsigned ticks = (unsigned)((start - previous) * 60 + 0.5);
  ticks = ticks < 1 ? 1 : ticks > 3 ? 3 : ticks;
  previous = start;

  if (stage == CARD) {
    card();
    return;
  }
  if (stage == GUEST) {
    // The interface comes up before the pack is read, so the load shows through it.
    static const char reading[] = "Reading the city";
    tk_stage(TK_STAGE_LOADING, reading, sizeof reading - 1);
    size_t script_size = 0, pak_size = 0, prefs_size = 0;
    char rate[40], path[700];
    snprintf(rate, sizeof rate, "globalThis.__simHz=%d;", TURN_HZ);
    char *script = read_whole("tokyo.js", rate, &script_size), *pak = read_whole("tokyo.pak", "", &pak_size);
    snprintf(path, sizeof path, "%s/interface.json", files);
    FILE *kept = fopen(path, "rb");
    if (kept) {
      static char prefs[4096];
      prefs_size = fread(prefs, 1, sizeof prefs, kept);
      fclose(kept);
      tk_prefs_stored(prefs, (uint32_t)prefs_size);
    }
    guest = script && pak && pocket_runtime_boot(script, script_size, (const uint8_t *)pak, pak_size, LOGICAL_WIDTH, LOGICAL_HEIGHT) && pocket_runtime_gl_initialize();
    if (!guest)
      snprintf(failure, sizeof failure, "interface: %s", script && pak ? pocket_runtime_error() : "tokyo.js or tokyo.pak is missing");
    if (!open_pack()) {
      static const char missing[] = "The city's pack is missing from the package";
      snprintf(failure, sizeof failure, "%s", missing);
      tk_stage(TK_STAGE_ERROR, missing, sizeof missing - 1);
      stage = FAILED;
    } else {
      stage = LOADING;
    }
    status();
    return;
  }

  if (frames % 12 == 0)
    control();
  if (timers) {
    // The query of four frames ago has its answer by now.
    GLuint ready = 0, nanoseconds = 0;
    if (frames >= 4) {
      query_get(queries[frames % 4], 0x8867 /* GL_QUERY_RESULT_AVAILABLE */, &ready);
      if (ready) {
        query_get(queries[frames % 4], 0x8866 /* GL_QUERY_RESULT */, &nanoseconds);
        timer_ms += ((float)nanoseconds * 1e-6f - timer_ms) * 0.1f;
        perf.gpu = timer_ms;
        perf.gpu_now = (float)nanoseconds * 1e-6f;
      }
    }
    query_begin(TIME_ELAPSED, queries[frames % 4]);
  }
  double stepped = start, turned, drawn, covered;
  glBindFramebuffer(GL_FRAMEBUFFER, 0);
  glViewport(0, 0, width, height);
  glDisable(GL_SCISSOR_TEST);
  glDepthMask(GL_TRUE);
  glColorMask(GL_TRUE, GL_TRUE, GL_TRUE, GL_TRUE);
  if (stage == RUNNING) {
    tk_step(ticks);
    tk_report(&perf);
    static char keep[4096];
    uint32_t n = tk_prefs_take(keep, sizeof keep);
    if (n)
      write_file(files, "interface.json", keep, n);
    stepped = now();
    turn(ticks, false);
    turned = now();
    // Every pixel is cleared: a tile is then not read back before it is drawn.
    glClearColor(0, 0, 0, 1);
    glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT | GL_STENCIL_BUFFER_BIT);
    tk_draw(&perf);
    drawn = now();
  } else {
    // Before the city is there, the interface alone, over the ground of the title card; the pack is read
    // a step a frame once the interface has had two frames to say so.
    if (stage == LOADING && frames >= 2) {
      static char message[400];
      int32_t done = tk_load(pack_fd, (int64_t)pack_offset, (int64_t)pack_length, message, sizeof message);
      if (done < 0) {
        snprintf(failure, sizeof failure, "%s", message);
        tk_stage(TK_STAGE_ERROR, message, (uint32_t)strlen(message));
        stage = FAILED;
      } else if (done > 0) {
        stage = RUNNING;
        previous = now();
      } else {
        tk_stage(TK_STAGE_LOADING, message, (uint32_t)strlen(message));
      }
      start = now();
    }
    stepped = now();
    turn(2, true);
    turned = now();
    glClearColor(0.09f, 0.07f, 0.15f, 1);
    glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT | GL_STENCIL_BUFFER_BIT);
    drawn = now();
  }
  interface_draw();
  if (timers)
    query_end(TIME_ELAPSED);
  covered = now();
  gpu_ms = 0;
  if (profile) {
    // To time the GPU the frame is finished before anything else is asked of it.
    glFinish();
    gpu_ms = (float)(now() - covered) * 1000;
    covered = now();
  }
  if (screen) {
    write_picture("screen.rgba");
    snprintf(captured, sizeof captured, "%s", command);
    screen = false;
    covered = now();
  }
  show();
  double done = now();

  // Frames are shown one a refresh while the GPU keeps up. A frame that takes longer leaves a refresh
  // without a new picture: the time behind adds up, and each whole refresh of it is a late frame.
  float interval = shown_at ? (float)(done - shown_at) * 1000 : (float)REFRESH_MS;
  shown_at = done;
  // (the first frames after the pack is read are the load's, not the flight's)
  static unsigned running;
  running = stage == RUNNING ? running + 1 : 0;
  bool counted = running > 20 && interval < 500;
  unsigned missed = 0;
  if (counted) {
    debt += interval - REFRESH_MS;
    if (debt < -REFRESH_MS * 0.5)
      debt = -REFRESH_MS * 0.5;
    while (debt > REFRESH_MS * 0.5) {
      debt -= REFRESH_MS;
      missed++;
    }
    perf.late += missed;
    perf.frames++;
    // (a development run says what a late frame was made of)
    if (missed && development)
      LOG("late: %.1f ms (step %.1f, guest %.1f, draw %.1f, interface %.1f, swap %.1f), GPU %.1f ms", interval, (stepped - start) * 1000, (turned - stepped) * 1000,
           (drawn - turned) * 1000, (covered - drawn) * 1000, (done - covered) * 1000, perf.gpu);
    window_ms[frames % WINDOW] = interval;
    float sum = 0, worst = 0;
    for (unsigned i = 0; i < WINDOW; i++) {
      float ms = perf.frames > i ? window_ms[i] : (float)REFRESH_MS;
      sum += ms;
      worst = ms > worst ? ms : worst;
    }
    perf.frame = sum / WINDOW;
    perf.worst = worst;
    perf.last = interval;
    if (mark.until) {
      mark.frames++;
      mark.late += missed;
      mark.sum += interval;
      mark.worst = interval > mark.worst ? interval : mark.worst;
      if (done >= mark.until) {
        mark.done_frames = mark.frames;
        mark.done_late = mark.late;
        mark.done_worst = mark.worst;
        mark.done_mean = (float)(mark.sum / mark.frames);
        mark.until = 0;
      }
    }
  }
  sums[0] += (float)(stepped - start) * 1000;
  sums[1] += (float)(turned - stepped) * 1000;
  sums[2] += (float)(drawn - turned) * 1000;
  sums[3] += (float)(covered - drawn) * 1000;
  sums[4] += (float)(done - covered) * 1000;
  frames++;
  if (frames % 30 == 0) {
    perf.step = sums[0] / 30;
    perf.guest = sums[1] / 30;
    perf.draw = sums[2] / 30;
    perf.interface = sums[3] / 30;
    perf.swap = sums[4] / 30;
    memset(sums, 0, sizeof sums);
    status();
  }
}

void android_main(struct android_app *a) {
  app = a;
  a->onAppCmd = on_command;
  a->onInputEvent = on_input;
  snprintf(files, sizeof files, "%s", a->activity->internalDataPath);
#ifndef TOKYO_RELEASE
  // A release has no development copies: `dev` stays empty, and no file is under it.
  snprintf(dev, sizeof dev, "%s/dev", files);
#endif
  mkdir(files, 0700);
  boot_words();
  // The screen stays on while the city is flown over. A development run also comes up over the lock screen.
  uint32_t flags = AWINDOW_FLAG_KEEP_SCREEN_ON | AWINDOW_FLAG_FULLSCREEN;
  if (development)
    flags |= AWINDOW_FLAG_SHOW_WHEN_LOCKED | AWINDOW_FLAG_TURN_SCREEN_ON | AWINDOW_FLAG_DISMISS_KEYGUARD;
  ANativeActivity_setWindowFlags(a->activity, flags, 0);
  pthread_t writer;
  pthread_create(&writer, NULL, reporter, NULL);
  bool started = false;
  for (;;) {
    int events;
    struct android_poll_source *source;
    bool ready = surface != EGL_NO_SURFACE && resumed;
    while (ALooper_pollAll(ready ? 0 : -1, NULL, &events, (void **)&source) >= 0) {
      if (source)
        source->process(a, source);
      if (a->destroyRequested) {
        // The process ends with the activity: a second launch starts from the title card.
        drop_surface();
        exit(0);
      }
      ready = surface != EGL_NO_SURFACE && resumed;
    }
    if (!ready)
      continue;
    if (!started) {
      started = true;
      cover_start();
      timer_start();
    }
    if (stage == FAILED && failure[0] && frames == 0)
      LOG("failed: %s", failure);
    frame();
  }
}
