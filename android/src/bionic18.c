// Five functions Rust's standard library links against and Android 4.3's C library (API 18) does not
// have. They are declared here by hand: the NDK's headers for this API level hide or inline them.
#include <stdarg.h>
#include <stddef.h>

extern int open(const char *path, int flags, ...);
extern int openat(int directory, const char *path, int flags, ...);
extern int mknod(const char *path, unsigned mode, unsigned long long device);
extern void (*bsd_signal(int signal, void (*handler)(int)))(int);

#define LARGE_FILE 0400000
#define FIFO 0010000

int open64(const char *path, int flags, ...) {
  va_list arguments;
  va_start(arguments, flags);
  int mode = va_arg(arguments, int);
  va_end(arguments);
  return open(path, flags | LARGE_FILE, mode);
}

int openat64(int directory, const char *path, int flags, ...) {
  va_list arguments;
  va_start(arguments, flags);
  int mode = va_arg(arguments, int);
  va_end(arguments);
  return openat(directory, path, flags | LARGE_FILE, mode);
}

int mkfifo(const char *path, unsigned mode) {
  return mknod(path, (mode & 07777) | FIFO, 0);
}

void (*signal(int number, void (*handler)(int)))(int) {
  return bsd_signal(number, handler);
}

// Rust asks for the loaded objects only to name the frames of a backtrace; a panic here aborts.
int dl_iterate_phdr(int (*each)(void *, size_t, void *), void *data) {
  (void)each;
  (void)data;
  return 0;
}
