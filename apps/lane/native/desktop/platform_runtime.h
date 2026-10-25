// SPDX-License-Identifier: Apache-2.0
#pragma once
#include "handler.h"
#include <mutex>
#include <utility>

namespace lane::desktop {
struct Events {
  void Push(std::string name, std::string arguments) {
    std::lock_guard<std::mutex> lock(mutex);
    pending.emplace_back(std::move(name), std::move(arguments));
  }
  std::vector<std::pair<std::string, std::string>> Drain() {
    std::lock_guard<std::mutex> lock(mutex);
    std::vector<std::pair<std::string, std::string>> result;
    result.swap(pending);
    return result;
  }
 private:
  std::mutex mutex;
  std::vector<std::pair<std::string, std::string>> pending;
};
struct PlatformRuntime {
  std::shared_ptr<Handler> store, host, control;
  virtual void Pump() {}
  virtual ~PlatformRuntime() = default;
};
std::string ReadNotificationActivation(const std::string& resources);
std::shared_ptr<PlatformRuntime> CreatePlatformRuntime(
    const std::string& resources, const std::string& scheme, std::shared_ptr<Events> events);
}
