// SPDX-License-Identifier: Apache-2.0
#pragma once

#include <cctype>
#include <sstream>
#include <string>

// Platform-neutral capture shortcut grammar: one or more modifiers plus one
// ASCII letter or digit, separated by '+'. Tokens are case-insensitive and the
// key is normalized to uppercase so both desktop hosts accept the same input.
struct LaneCaptureShortcut {
  bool command = false;
  bool control = false;
  bool alt = false;
  bool shift = false;
  char key = 0;
};

enum class LaneCaptureShortcutParse { Ok, Invalid };
enum class LaneShortcutPlatform { MacOS, Windows };

inline LaneCaptureShortcutParse LaneParseCaptureShortcut(const std::string& text,
                                                         LaneCaptureShortcut& out) {
  out = LaneCaptureShortcut{};
  std::istringstream stream(text);
  std::string token;
  while (std::getline(stream, token, '+')) {
    std::string lower;
    for (char c : token) lower.push_back(static_cast<char>(std::tolower(static_cast<unsigned char>(c))));
    if (lower == "command" || lower == "cmd" || lower == "meta" || lower == "win") out.command = true;
    else if (lower == "ctrl" || lower == "control") out.control = true;
    else if (lower == "alt" || lower == "option") out.alt = true;
    else if (lower == "shift") out.shift = true;
    else if (lower.size() == 1 && ((lower[0] >= 'a' && lower[0] <= 'z') || (lower[0] >= '0' && lower[0] <= '9')) && !out.key)
      out.key = static_cast<char>(std::toupper(static_cast<unsigned char>(lower[0])));
    else return LaneCaptureShortcutParse::Invalid;
  }
  // Shift alone would intercept ordinary typing, so require a non-Shift modifier.
  if (!out.key || !(out.command || out.control || out.alt)) return LaneCaptureShortcutParse::Invalid;
  return LaneCaptureShortcutParse::Ok;
}

// Reserve every Lane application chord representable by the capture grammar
// before replacing an existing binding: Lane menu commands, tab commands and the
// Edit menu installed by `{ role: "editMenu" }` (undo, redo, cut, copy, paste,
// select all and, on macOS, paste and match style). Matching is exact, so other
// modifier combinations with the same key stay assignable.
inline bool LaneReservedCaptureShortcut(const LaneCaptureShortcut& shortcut,
                                        LaneShortcutPlatform platform) {
  const bool mac = platform == LaneShortcutPlatform::MacOS;
  const bool primary = mac ? shortcut.command : shortcut.control;
  const bool other = mac ? shortcut.control : shortcut.command;
  if (!primary || other) return false;
  const char key = shortcut.key;
  if (!shortcut.alt && !shortcut.shift) {
    if (key == 'Q' || key == 'O' || key == 'K' || key == 'T' || key == 'W') return true;
    if (key >= '1' && key <= '9') return true;
    if (key == 'Z' || key == 'X' || key == 'C' || key == 'V' || key == 'A') return true;
    // Windows Edit menu redo.
    return !mac && key == 'Y';
  }
  if (!shortcut.alt && shortcut.shift) return key == 'T' || key == 'Z';
  // macOS Edit menu Paste and Match Style.
  return mac && shortcut.alt && shortcut.shift && key == 'V';
}
