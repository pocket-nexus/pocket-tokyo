// The city, behind the functions of ../core (Rust): the pack, what a frame
// draws, the shadows, the traffic, the flight and the flow around it, and the
// interface's channel.
#ifndef TOKYO_CORE_H
#define TOKYO_CORE_H

#include <stdint.h>

// What the shell measured of the frames it has shown (`Perf` in ../core/src/lib.rs).
typedef struct {
  float frame, worst, last; // milliseconds from one shown frame to the next: mean and worst of the last 240, and the last
  uint32_t late, frames;    // refreshes on which no new frame was ready; frames shown
  float step, guest, draw, interface, swap; // milliseconds of the render thread in a frame, by what it did
  float gpu; // milliseconds the GPU took over a frame, smoothed; 0 where the driver does not say
  float gpu_now; // the same for the latest frame the driver has answered for
} TkPerf;

enum { TK_STAGE_LOADING = 0, TK_STAGE_ERROR = 1 };

// A frame of the Pocket3D title card as RGBA rows: 0 the card is over, 1 drawn, 2 the same as at tick `shown`.
uint32_t tk_card(uint8_t *pixels, uint32_t width, uint32_t height, uint32_t tick, uint32_t shown);
// What the interface shows while there is no flight.
void tk_stage(uint32_t stage, const char *message, uint32_t length);
void tk_prefs_stored(const char *text, uint32_t length);
uint32_t tk_prefs_take(char *out, uint32_t capacity);
uint32_t tk_guest_due(uint32_t buttons, uint32_t touching);
uint32_t tk_interface_open(void);
// The window's size in pixels.
void tk_window(uint32_t width, uint32_t height);
// One step of reading the pack at `offset` of `fd`: 0 more to do, 1 the city is ready, -1 it failed.
int32_t tk_load(int fd, int64_t offset, int64_t length, char *message, uint32_t capacity);
void tk_step(uint32_t ticks);
void tk_report(const TkPerf *perf);
// The city and the sky into the bound target, which the caller has cleared.
void tk_draw(const TkPerf *perf);
void tk_control(const char *words, uint32_t length);
// The back key: 0 when the flow has nothing to do with it.
uint32_t tk_back(void);
uint32_t tk_status(char *out, uint32_t capacity, const TkPerf *perf, const char *extra, uint32_t extra_length);

#endif
