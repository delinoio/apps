// SPDX-License-Identifier: Apache-2.0
#include <jni.h>
#include <limits>
#include <memory>
#include <stdexcept>
#include <string>
#include "lane_store.h"
#include "lane_log.h"

namespace {
// JNI modified UTF-8 corrupts supplementary characters and embedded NULs.
// Byte arrays carry standard UTF-8 in both directions, including Markdown.
std::string decode(JNIEnv* env, jbyteArray value) {
  if (!value) throw std::invalid_argument("missing bytes");
  const auto size = env->GetArrayLength(value);
  std::string result(static_cast<size_t>(size), '\0');
  env->GetByteArrayRegion(value, 0, size, reinterpret_cast<jbyte*>(result.data()));
  if (env->ExceptionCheck()) throw std::runtime_error("JNI read failed");
  return result;
}
jbyteArray encode(JNIEnv* env, const std::string& value) {
  if (value.size() > static_cast<size_t>(std::numeric_limits<jsize>::max()))
    throw std::length_error("JNI response too large");
  auto result = env->NewByteArray(static_cast<jsize>(value.size()));
  if (result) env->SetByteArrayRegion(result, 0, static_cast<jsize>(value.size()),
                                     reinterpret_cast<const jbyte*>(value.data()));
  return result;
}
struct Peer {
  std::unique_ptr<lane::Store> engine;
  std::string root;
  JavaVM* vm;
  jobject host;
  jmethodID callback;
  std::string call(int operation, const std::string& space = "",
                   const std::string& method = "", const std::string& request = "") {
    JNIEnv* env = nullptr;
    if (vm->GetEnv(reinterpret_cast<void**>(&env), JNI_VERSION_1_6) != JNI_OK)
      throw std::runtime_error("Host callback requires attached worker");
    if (env->PushLocalFrame(8) < 0) throw std::runtime_error("JNI local frame failed");
    auto result = static_cast<jbyteArray>(env->CallObjectMethod(
        host, callback, operation, encode(env, space), encode(env, method), encode(env, request)));
    if (env->ExceptionCheck()) {
      env->ExceptionClear();
      env->PopLocalFrame(nullptr);
      throw std::runtime_error("Host callback failed");
    }
    auto value = decode(env, result);
    env->PopLocalFrame(nullptr);
    return value;
  }
};
lane::Store& store(jlong handle) {
  if (!handle) throw std::invalid_argument("closed store");
  return *reinterpret_cast<Peer*>(handle)->engine;
}
void fail(JNIEnv* env) {
  // Never forward exception text: SQLite and filesystem errors may contain content.
  if (!env->ExceptionCheck()) {
    auto type = env->FindClass("java/lang/IllegalStateException");
    if (type) env->ThrowNew(type, "Lane native storage operation failed");
  }
}
}
extern "C" JNIEXPORT jlong JNICALL
Java_io_delino_lane_NativeStore_create(JNIEnv* env, jobject self, jbyteArray root) {
  try {
    auto peer = std::make_unique<Peer>();
    peer->root = decode(env, root);
    peer->engine = std::make_unique<lane::Store>(peer->root);
    env->GetJavaVM(&peer->vm);
    peer->callback = env->GetMethodID(env->GetObjectClass(self), "hostCall", "(I[B[B[B)[B");
    if (!peer->callback) throw std::runtime_error("Missing host callback");
    peer->host = env->NewGlobalRef(self);
    if (!peer->host) throw std::runtime_error("Missing host reference");
    auto* p = peer.get();
    lane::HostHooks hooks;
    hooks.remote = [p](std::string space, std::string method, std::string request) {
      return p->call(0, space, method, request);
    };
    hooks.authorize = [p](std::string account) { return p->call(1, "", "", account); };
    hooks.requestNotificationPermission = [p]() { return p->call(2); };
    hooks.disconnect = [p](std::string space) { p->call(3, space); };
    peer->engine->setHostHooks(std::move(hooks));
    return reinterpret_cast<jlong>(peer.release());
  }
  catch (...) { fail(env); return 0; }
}
extern "C" JNIEXPORT jbyteArray JNICALL
Java_io_delino_lane_NativeStore_call(JNIEnv* env, jobject, jlong handle,
                                    jint operation, jbyteArray method, jbyteArray request) {
  try {
    auto& engine = store(handle);
    switch (operation) {
      case 0: return encode(env, engine.open(decode(env, request)));
      case 1: return encode(env, engine.invoke(decode(env, method), decode(env, request)));
      case 2: return encode(env, engine.close(decode(env, request)));
      case 3: return encode(env, engine.poll());
      case 4: return encode(env, engine.approveAccount(decode(env, request), decode(env, method) == "true"));
      case 5: {
        auto input = decode(env, request);
        auto split = input.find('\n');
        if (split == std::string::npos) throw std::invalid_argument("Invalid lease");
        return encode(env, engine.acquireSync(decode(env, method), input.substr(0, split), std::stoi(input.substr(split + 1))));
      }
      case 6: return encode(env, engine.pendingSync(decode(env, method), decode(env, request)));
      case 7: return encode(env, engine.applySync(decode(env, method), decode(env, request)));
      case 8: return encode(env, engine.claimNotifications(decode(env, method), decode(env, request)));
      case 9: return encode(env, engine.acknowledgeNotification(decode(env, method), decode(env, request)));
      case 10: return encode(env, engine.recordNotificationPermission(decode(env, method), decode(env, request)));
      default: throw std::invalid_argument("unknown JNI operation");
    }
  } catch (...) { fail(env); return nullptr; }
}
extern "C" JNIEXPORT void JNICALL
Java_io_delino_lane_NativeStore_destroy(JNIEnv* env, jobject, jlong handle) {
  auto* peer = reinterpret_cast<Peer*>(handle);
  if (!peer) return;
  env->DeleteGlobalRef(peer->host);
  delete peer;
}

extern "C" JNIEXPORT void JNICALL
Java_io_delino_lane_NativeStore_logEvent(JNIEnv* env, jobject, jlong handle, jbyteArray event) {
  try {
    if (handle) lane_log(reinterpret_cast<Peer*>(handle)->root, decode(env, event));
  } catch (...) { /* Diagnostic failures do not escape into the bridge. */ }
}
