// SPDX-License-Identifier: Apache-2.0
#include "shortcut-reservations.h"
#include <iostream>
#include <stdexcept>
#include <string>

namespace {
void Check(bool ok, const std::string& message) {
  if (!ok) throw std::runtime_error(message);
}
LaneCaptureShortcut Parse(const std::string& text) {
  LaneCaptureShortcut value;
  Check(LaneParseCaptureShortcut(text, value) == LaneCaptureShortcutParse::Ok, "Rejected " + text);
  return value;
}
bool Reserved(const std::string& text, LaneShortcutPlatform platform) {
  return LaneReservedCaptureShortcut(Parse(text), platform);
}
void Run() {
  auto lower = Parse("cmd+shift+n");
  Check(lower.command && lower.shift && !lower.control && !lower.alt && lower.key == 'N',
        "Lowercase tokens must normalize to the same shortcut");
  Check(Parse("Command+M").key == 'M' && Parse("Ctrl+0").key == '0', "Letter and digit keys must parse");
  for (auto text : {"", "Shift+A", "A", "Ctrl", "Ctrl+F1", "Ctrl+AB", "Ctrl+A+B", "Ctrl+Space", "Ctrl+-", "Hyper+A"}) {
    LaneCaptureShortcut value;
    Check(LaneParseCaptureShortcut(text, value) == LaneCaptureShortcutParse::Invalid,
          std::string("Accepted invalid shortcut ") + text);
  }
  const auto mac = LaneShortcutPlatform::MacOS, windows = LaneShortcutPlatform::Windows;
  // Lane, tab and Edit menu commands on each platform's primary modifier.
  for (auto key : {"Q", "O", "K", "T", "W", "1", "5", "9", "Z", "X", "C", "V", "A"}) {
    Check(Reserved(std::string("Command+") + key, mac), std::string("macOS must reserve Command+") + key);
    Check(Reserved(std::string("Ctrl+") + key, windows), std::string("Windows must reserve Ctrl+") + key);
    Check(!Reserved(std::string("Ctrl+") + key, mac), std::string("macOS Control is not Lane's primary: ") + key);
    Check(!Reserved(std::string("Command+") + key, windows), std::string("Windows key is not Lane's primary: ") + key);
  }
  Check(Reserved("Command+Shift+T", mac) && Reserved("Ctrl+Shift+T", windows), "Reopen tab must be reserved");
  Check(Reserved("Command+Shift+Z", mac) && Reserved("Ctrl+Shift+Z", windows), "Redo must be reserved");
  Check(Reserved("Ctrl+Y", windows) && !Reserved("Command+Y", mac), "Windows redo is Ctrl+Y only");
  Check(Reserved("Command+Option+Shift+V", mac) && !Reserved("Ctrl+Alt+Shift+V", windows),
        "Paste and Match Style is reserved on macOS only");
  // Exact modifier matching keeps neighboring chords assignable.
  for (auto text : {"Command+Shift+C", "Command+Option+C", "Command+0", "Command+Shift+1", "Command+Control+Z",
                    "Command+Option+V", "Command+Shift+V", "Command+S", "Command+N", "Command+M"})
    Check(!Reserved(text, mac), std::string("macOS over-reserved ") + text);
  for (auto text : {"Ctrl+Shift+C", "Ctrl+Alt+C", "Ctrl+0", "Ctrl+Shift+Y", "Ctrl+Win+Z", "Ctrl+Alt+Shift+V", "Alt+Q"})
    Check(!Reserved(text, windows), std::string("Windows over-reserved ") + text);
}
}  // namespace

int main() {
  try {
    Run();
    std::cout << "Lane capture shortcut reservation tests passed\n";
    return 0;
  } catch (const std::exception& error) {
    std::cerr << error.what() << '\n';
    return 1;
  }
}
