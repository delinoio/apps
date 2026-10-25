// SPDX-License-Identifier: Apache-2.0
#pragma once
#include <condition_variable>
#include <deque>
#include <functional>
#include <future>
#include <mutex>
#include <stdexcept>
#include <thread>

namespace lane::windows {
// The store is created, called and destroyed on this thread. UI work never
// waits for a database transaction; SDK async workers may wait for their result.
class SerialWorker {
 public:
  explicit SerialWorker(std::function<void()> start, std::function<void()> tick,
                        std::function<void()> finish)
      : thread_([this, start, tick, finish] {
          try { start(); } catch (...) { startup_error_ = std::current_exception(); }
          auto next = std::chrono::steady_clock::now();
          for (;;) {
            std::function<void()> task;
            {
              std::unique_lock<std::mutex> lock(mutex_);
              ready_.wait_until(lock, next, [this] { return stopping_ || !queue_.empty(); });
              if (stopping_ && queue_.empty()) break;
              if (!queue_.empty()) { task = std::move(queue_.front()); queue_.pop_front(); }
            }
            if (task) task();
            if (std::chrono::steady_clock::now() >= next) {
              if (!startup_error_) { try { tick(); } catch (...) {} }
              next = std::chrono::steady_clock::now() + std::chrono::milliseconds(500);
            }
          }
          try { finish(); } catch (...) {}
        }) {}
  ~SerialWorker() { Stop(); }
  SerialWorker(const SerialWorker&) = delete;
  SerialWorker& operator=(const SerialWorker&) = delete;
  std::future<std::string> Submit(std::function<std::string()> function) {
    auto task = std::make_shared<std::packaged_task<std::string()>>([this, function = std::move(function)] {
      if (startup_error_) std::rethrow_exception(startup_error_);
      return function();
    });
    auto result = task->get_future();
    {
      std::lock_guard<std::mutex> lock(mutex_);
      if (stopping_) throw std::runtime_error("Lane worker has stopped");
      queue_.push_back([task] { (*task)(); });
    }
    ready_.notify_one();
    return result;
  }
  void Stop() {
    { std::lock_guard<std::mutex> lock(mutex_); stopping_ = true; }
    ready_.notify_all();
    if (thread_.joinable()) thread_.join();
  }
 private:
  std::mutex mutex_;
  std::condition_variable ready_;
  std::deque<std::function<void()>> queue_;
  bool stopping_ = false;
  std::exception_ptr startup_error_;
  std::thread thread_;
};
}  // namespace lane::windows
