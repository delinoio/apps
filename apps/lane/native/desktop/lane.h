// SPDX-License-Identifier: Apache-2.0
#pragma once
#include <filesystem>
#include <fstream>
#include <iostream>
#include <iterator>
#include <stdexcept>
#include <vector>
#include "lynx_env.h"
#include "lynx_view.h"
#include "lynx_view_client.h"

class LaneClient : public lynx::pub::LynxViewClient {
 public:
  void OnLoadSuccess() override { std::clog << "{\"event\":\"lane_template_loaded\"}\n"; }
  void OnReceivedError(int code, const char*) override {
    std::cerr << "{\"event\":\"lane_template_failed\",\"code\":" << code << "}\n";
  }
};
inline void LoadLane(lynx::pub::LynxView& view, const std::filesystem::path& path) {
  std::ifstream input(path, std::ios::binary);
  if (!input) throw std::runtime_error("Cannot open embedded Lane template");
  auto* bytes = new std::vector<uint8_t>(std::istreambuf_iterator<char>(input),
                                        std::istreambuf_iterator<char>{});
  if (bytes->empty()) { delete bytes; throw std::runtime_error("Empty Lane template"); }
  auto meta = std::make_shared<lynx::pub::LynxLoadMeta>();
  meta->SetUrl("main.lynx.bundle");
  meta->SetBinaryData(bytes->data(), bytes->size(),
    [](uint8_t*, size_t, void* owner) { delete static_cast<std::vector<uint8_t>*>(owner); }, bytes);
  view.LoadTemplate(meta);
}
