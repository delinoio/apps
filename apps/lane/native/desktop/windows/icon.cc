// SPDX-License-Identifier: Apache-2.0
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <iterator>
#include <stdexcept>
#include <vector>

namespace {
uint32_t Read(const std::vector<uint8_t>& bytes, size_t offset, size_t count) {
  if (offset > bytes.size() || count > bytes.size() - offset) throw std::runtime_error("Invalid icon");
  uint32_t value = 0;
  for (size_t i = 0; i < count; ++i) value |= uint32_t(bytes[offset + i]) << (8 * i);
  return value;
}
void SetIcon(const std::filesystem::path& executable, const std::filesystem::path& icon) {
  std::ifstream file(icon, std::ios::binary);
  std::vector<uint8_t> bytes(std::istreambuf_iterator<char>(file), std::istreambuf_iterator<char>{});
  if (Read(bytes, 0, 2) != 0 || Read(bytes, 2, 2) != 1) throw std::runtime_error("Invalid icon");
  const auto count = Read(bytes, 4, 2);
  if (!count || count > 256 || bytes.size() < 6 + count * 16) throw std::runtime_error("Invalid icon");
  auto handle = BeginUpdateResourceW(executable.c_str(), FALSE);
  if (!handle) throw std::runtime_error("Cannot update executable icon");
  try {
    std::vector<uint8_t> group(bytes.begin(), bytes.begin() + 6);
    for (uint32_t index = 0; index < count; ++index) {
      const auto entry = 6 + index * 16;
      const auto size = Read(bytes, entry + 8, 4), offset = Read(bytes, entry + 12, 4);
      if (!size || offset > bytes.size() || size > bytes.size() - offset)
        throw std::runtime_error("Invalid icon image");
      const auto id = static_cast<WORD>(index + 1);
      if (!UpdateResourceW(handle, RT_ICON, MAKEINTRESOURCEW(id), MAKELANGID(LANG_NEUTRAL, SUBLANG_NEUTRAL),
          bytes.data() + offset, size)) throw std::runtime_error("Cannot update icon image");
      group.insert(group.end(), bytes.begin() + entry, bytes.begin() + entry + 12);
      group.push_back(static_cast<uint8_t>(id)); group.push_back(static_cast<uint8_t>(id >> 8));
    }
    if (!UpdateResourceW(handle, RT_GROUP_ICON, MAKEINTRESOURCEW(1), MAKELANGID(LANG_NEUTRAL, SUBLANG_NEUTRAL),
        group.data(), static_cast<DWORD>(group.size()))) throw std::runtime_error("Cannot update icon group");
  } catch (...) {
    EndUpdateResourceW(handle, TRUE);
    throw;
  }
  if (!EndUpdateResourceW(handle, FALSE)) throw std::runtime_error("Cannot save executable icon");
}
}
int wmain(int count, wchar_t** args) {
  try {
    if (count != 3) return 2;
    SetIcon(args[1], args[2]);
    return 0;
  } catch (...) {
    std::cerr << "{\"event\":\"lane_icon_packaging_failed\"}\n";
    return 1;
  }
}
