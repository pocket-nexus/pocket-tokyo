// What NativeActivity loads (`android.app.lib_name`): it hands the activity to
// the engine, which is a library of its own so that a development run can
// replace it without an install. `files/dev/libtokyo-engine.so` in the app's
// data is taken when it is there (tools/android.ts native puts it there
// through run-as); otherwise the one the package installed beside this file.
#include <android/log.h>
#include <android/native_activity.h>
#include <dlfcn.h>
#include <stdio.h>
#include <sys/stat.h>

typedef void Create(ANativeActivity *, void *, size_t);

__attribute__((visibility("default"))) void ANativeActivity_onCreate(ANativeActivity *activity, void *saved, size_t size) {
  char path[1024];
  struct stat present;
  void *engine = NULL;
  snprintf(path, sizeof path, "%s/dev/libtokyo-engine.so", activity->internalDataPath);
  if (!stat(path, &present))
    engine = dlopen(path, RTLD_NOW);
  if (!engine) {
    // The package's libraries stand beside the app's files (`lib` there is the system's link to where it
    // put them). This Android's dladdr gives a library's name without its directory.
    snprintf(path, sizeof path, "%s/../lib/libtokyo-engine.so", activity->internalDataPath);
    engine = dlopen(path, RTLD_NOW);
  }
  const char *why = engine ? "" : dlerror();
  Create *create = engine ? (Create *)dlsym(engine, "ANativeActivity_onCreate") : NULL;
  if (!create) {
    __android_log_print(ANDROID_LOG_ERROR, "PocketTokyo", "no engine at %s: %s", path, why ? why : "");
    ANativeActivity_finish(activity);
    return;
  }
  __android_log_print(ANDROID_LOG_INFO, "PocketTokyo", "engine: %s", path);
  create(activity, saved, size);
}
