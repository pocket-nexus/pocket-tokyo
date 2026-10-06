// What NativeActivity loads (`android.app.lib_name`): it hands the activity to
// the engine, which is a library of its own so that a development run can
// replace it without an install. `files/dev/libtokyo-engine.so` in the app's
// data is taken when it is there (tools/android.ts native puts it there
// through run-as); otherwise the one the package installed beside this file.
// A release (TOKYO_RELEASE) takes the package's and no other.
//
// It also asks for the whole screen where the system draws a navigation bar
// over the window (`whole_screen`): that request is the UI thread's, and this
// file's functions are the ones that run on it.
#include <android/log.h>
#include <android/native_activity.h>
#include <dlfcn.h>
#include <jni.h>
#include <stdio.h>
#include <sys/stat.h>

typedef void Create(ANativeActivity *, void *, size_t);

// Android 4.4 and later hide the navigation bar and the status bar on request and bring them back for a
// moment at a swipe from an edge: View.setSystemUiVisibility with the sticky immersive flags (layout stable,
// layout under the bars, hide navigation, fullscreen, immersive sticky), asked of the window's decor view
// through JNI, since the package has no Java of its own. The Redmi 1S (Android 4.3, keys under its panel) is
// not asked. With no request for the display's cutout, the system keeps a window held on its side clear of it.
static void whole_screen(ANativeActivity *activity) {
  JNIEnv *env = activity->env;
  jclass kind = (*env)->GetObjectClass(env, activity->clazz);
  jobject window = (*env)->CallObjectMethod(env, activity->clazz, (*env)->GetMethodID(env, kind, "getWindow", "()Landroid/view/Window;"));
  jclass window_kind = window ? (*env)->GetObjectClass(env, window) : NULL;
  jobject decor = window ? (*env)->CallObjectMethod(env, window, (*env)->GetMethodID(env, window_kind, "getDecorView", "()Landroid/view/View;")) : NULL;
  jclass decor_kind = decor ? (*env)->GetObjectClass(env, decor) : NULL;
  if (decor)
    (*env)->CallVoidMethod(env, decor, (*env)->GetMethodID(env, decor_kind, "setSystemUiVisibility", "(I)V"), (jint)(0x0100 | 0x0200 | 0x0400 | 0x0002 | 0x0004 | 0x1000));
  if ((*env)->ExceptionCheck(env))
    (*env)->ExceptionClear(env);
  (*env)->DeleteLocalRef(env, decor_kind);
  (*env)->DeleteLocalRef(env, decor);
  (*env)->DeleteLocalRef(env, window_kind);
  (*env)->DeleteLocalRef(env, window);
  (*env)->DeleteLocalRef(env, kind);
}

// The system drops the request when another window has had the focus (the notification shade, a dialog): it
// is made again each time this one takes it, before the engine hears of the focus.
static void (*engine_focus)(ANativeActivity *, int);

static void on_focus(ANativeActivity *activity, int focused) {
  if (focused)
    whole_screen(activity);
  if (engine_focus)
    engine_focus(activity, focused);
}

__attribute__((visibility("default"))) void ANativeActivity_onCreate(ANativeActivity *activity, void *saved, size_t size) {
  char path[1024];
  void *engine = NULL;
#ifndef TOKYO_RELEASE
  struct stat present;
  snprintf(path, sizeof path, "%s/dev/libtokyo-engine.so", activity->internalDataPath);
  if (!stat(path, &present))
    engine = dlopen(path, RTLD_NOW);
#endif
  if (!engine) {
    // The package's libraries stand beside the app's files (`lib` there is the system's link to where it
    // put them). Android 4.3's dladdr gives a library's name without its directory.
    snprintf(path, sizeof path, "%s/../lib/libtokyo-engine.so", activity->internalDataPath);
    engine = dlopen(path, RTLD_NOW);
  }
  if (!engine) {
    // A current Android keeps no such link: it searches the package's own library directory by name.
    snprintf(path, sizeof path, "libtokyo-engine.so");
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
  if (activity->sdkVersion >= 19) {
    engine_focus = activity->callbacks->onWindowFocusChanged;
    activity->callbacks->onWindowFocusChanged = on_focus;
    whole_screen(activity);
  }
}
