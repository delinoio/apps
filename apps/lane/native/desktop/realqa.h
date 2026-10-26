// SPDX-License-Identifier: Apache-2.0
#pragma once
#include "platform_runtime.h"
#include "../store/vendor/json/json.hpp"
#include <stdexcept>
#include <cmath>
namespace lane::desktop {
using CaptureJSON = nlohmann::json;
struct CaptureError : std::runtime_error {
  std::string reason;
  bool retryable;
  CaptureError(std::string message, std::string code = "ERROR_REASON_INVALID_INPUT", bool retry = false)
      : std::runtime_error(message), reason(std::move(code)), retryable(retry) {}
};
inline CaptureJSON CaptureFailure(const CaptureError& error) {
  return {{"ok", false}, {"error", {{"reason", error.reason}, {"message", error.what()}, {"retryable", error.retryable}}}};
}
// Called once on the OS main thread. Operations execute on Node worker threads;
// each platform marshals UI and hotkey registration back to its own UI queue.
std::shared_ptr<Handler> CreateCaptureHost(std::shared_ptr<Events> events);
inline double Coordinate(const CaptureJSON& value, const char* key, double fallback = 0) {
  double result = value.value(key, fallback);
  if (!std::isfinite(result)) throw CaptureError("Image coordinates must be finite.");
  return result;
}
inline void ValidateEdits(const CaptureJSON& request, size_t width, size_t height) {
  if (!request.contains("layers") || !request["layers"].is_array()) throw CaptureError("Expected image layers.");
  for (const auto& layer : request["layers"]) {
    const auto type = layer.value("type", "");
    if (type != "draw" && type != "arrow" && type != "rectangle" && type != "text" && type != "blur" && type != "cover")
      throw CaptureError("Unknown image tool.");
    for (auto key : {"x", "y", "width", "height", "strokeWidth", "fontSize"}) Coordinate(layer, key);
    if (layer.contains("points")) for (const auto& p : layer["points"]) { Coordinate(p, "x"); Coordinate(p, "y"); }
    const auto color = layer.value("color", "#2563EB");
    if (color.size() != 7 || color[0] != '#' || color.find_first_not_of("0123456789abcdefABCDEF", 1) != std::string::npos)
      throw CaptureError("Choose a valid annotation color.");
    if (Coordinate(layer, "width") < 0 || Coordinate(layer, "height") < 0 || Coordinate(layer, "strokeWidth", 3) <= 0 || Coordinate(layer, "fontSize", 20) <= 0)
      throw CaptureError("Annotation dimensions must be positive.");
  }
  if (request.contains("crop") && !request["crop"].is_null()) {
    const auto& crop = request["crop"];
    double x = Coordinate(crop, "x"), y = Coordinate(crop, "y"), w = Coordinate(crop, "width"), h = Coordinate(crop, "height");
    if (x < 0 || y < 0 || w < 1 || h < 1 || x + w > width || y + h > height)
      throw CaptureError("The crop must be inside the image.");
  }
}
}
