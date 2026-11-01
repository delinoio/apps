// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const desktop = fileURLToPath(new URL("../native/desktop/", import.meta.url));
const json = fileURLToPath(
  new URL("../native/store/vendor/json/json.hpp", import.meta.url),
);
const reservations = fileURLToPath(
  new URL("../native/desktop/shortcut-reservations.h", import.meta.url),
);

function shortcutMethod(platform) {
  const source = readFileSync(
    join(desktop, platform, "capture." + (platform === "macos" ? "mm" : "cc")),
    "utf8",
  );
  // The macOS Shortcut method uses the key-code table declared before it.
  const keyCodes = source.indexOf(
    "  static const std::map<char,UInt32>& KeyCodes(",
  );
  const start =
    keyCodes >= 0 ? keyCodes : source.indexOf("  CaptureJSON Shortcut(");
  const end = source.indexOf("  CaptureJSON Call(", start);
  assert.ok(
    start >= 0 && end > start,
    `${platform} Shortcut method is present`,
  );
  return source.slice(start, end);
}

const common = `#include <cassert>
#include <cstdint>
#include <map>
#include <sstream>
#include <stdexcept>
#include <string>
#include <vector>
#include "${json}"
#include "${reservations}"
using CaptureJSON = nlohmann::json;
struct CaptureError : std::runtime_error {
  std::string reason;
  CaptureError(const char* message, const char* code = "") : std::runtime_error(message), reason(code) {}
};
int registrations = 0, releases = 0;
bool allowRegistration = true;
std::vector<std::string> operations;
template <typename Host>
void conflict(Host& host, const char* mode, const std::string& accelerator) {
  const int before = registrations, released = releases;
  bool rejected = false;
  try { host.Shortcut({{"mode", mode}, {"accelerator", accelerator}}); }
  catch (const CaptureError& error) {
    rejected = true;
    assert(error.reason == "ERROR_REASON_CONFLICT");
  }
  assert(rejected && registrations == before && releases == released);
}
`;

const windows = `${common}
using UINT = unsigned int;
constexpr UINT MOD_NOREPEAT = 0x4000, MOD_CONTROL = 2, MOD_ALT = 1, MOD_SHIFT = 4, MOD_WIN = 8;
int RegisterHotKey(int, int id, UINT, UINT) {
  ++registrations; operations.push_back("register"); return allowRegistration;
}
int UnregisterHotKey(int, int) { ++releases; operations.push_back("release"); return 1; }
struct Win {
  int window = 0, next = 1;
  std::map<std::string, int> shortcuts;
  std::map<int, std::string> modes;
${shortcutMethod("windows")}
};
int main() {
  for (const char* mode : {"screen", "window", "region", "dom"}) {
    Win host;
    for (const char* prefix : {"Ctrl+", "Control+"}) {
      for (const char* key : {"K", "Q", "O", "T", "W", "Z", "X", "C", "V", "A", "Y", "1", "2", "3", "4", "5", "6", "7", "8", "9"})
        conflict(host, mode, std::string(prefix) + key);
      conflict(host, mode, std::string(prefix) + "Shift+T");
      conflict(host, mode, std::string(prefix) + "Shift+Z");
      assert(host.shortcuts.empty());
    }
    host.Shortcut({{"mode", mode}, {"accelerator", "Ctrl+Shift+8"}});
    const int original = host.shortcuts.at(mode);
    for (const char* prefix : {"Ctrl+", "Control+"}) {
      for (const char* key : {"K", "Q", "O", "T", "W", "Z", "X", "C", "V", "A", "Y", "1", "2", "3", "4", "5", "6", "7", "8", "9"})
        conflict(host, mode, std::string(prefix) + key);
      conflict(host, mode, std::string(prefix) + "Shift+T");
      conflict(host, mode, std::string(prefix) + "Shift+Z");
      assert(host.shortcuts.at(mode) == original);
    }
    allowRegistration = false;
    const int before = releases;
    bool osConflict = false;
    try { host.Shortcut({{"mode", mode}, {"accelerator", "Ctrl+Shift+9"}}); }
    catch (const CaptureError& error) { osConflict = error.reason == "ERROR_REASON_CONFLICT"; }
    assert(osConflict && releases == before && host.shortcuts.at(mode) == original);
    allowRegistration = true;
    host.Shortcut({{"mode", mode}, {"accelerator", "Ctrl+Alt+K"}});
    assert(operations.at(operations.size() - 2) == "register");
    assert(operations.back() == "release");
    host.Shortcut({{"mode", mode}, {"accelerator", ""}});
    assert(host.shortcuts.empty() && releases == before + 2);
  }
}
`;

const macos = `#import <Cocoa/Cocoa.h>
#import <Carbon/Carbon.h>
${common}
OSStatus FakeRegister(UInt32, UInt32, EventHotKeyID, EventTargetRef, OptionBits, EventHotKeyRef* output) {
  ++registrations; operations.push_back("register");
  *output = reinterpret_cast<EventHotKeyRef>(static_cast<uintptr_t>(registrations));
  return allowRegistration ? noErr : -9878;
}
OSStatus FakeUnregister(EventHotKeyRef) { ++releases; operations.push_back("release"); return noErr; }
OSStatus FakeSystem(CFArrayRef* output) {
  *output = CFArrayCreate(nullptr, nullptr, 0, nullptr); return noErr;
}
#define RegisterEventHotKey FakeRegister
#define UnregisterEventHotKey FakeUnregister
#define CopySymbolicHotKeys FakeSystem
struct Mac {
  std::map<std::string, EventHotKeyRef> shortcuts;
  std::map<UInt32, std::string> modes;
  UInt32 next = 1;
  void Main(void (^block)(void)) { block(); }
${shortcutMethod("macos")}
};
int main() { @autoreleasepool {
  for (const char* mode : {"screen", "window", "region", "dom"}) {
    Mac host;
    for (const char* prefix : {"Command+", "Cmd+", "Meta+"}) {
      for (const char* key : {"K", "Q", "O", "T", "W", "Z", "X", "C", "V", "A", "1", "2", "3", "4", "5", "6", "7", "8", "9"})
        conflict(host, mode, std::string(prefix) + key);
      conflict(host, mode, std::string(prefix) + "Shift+T");
      conflict(host, mode, std::string(prefix) + "Shift+Z");
      assert(host.shortcuts.empty());
    }
    host.Shortcut({{"mode", mode}, {"accelerator", "Command+Shift+8"}});
    auto original = host.shortcuts.at(mode);
    for (const char* prefix : {"Command+", "Cmd+", "Meta+"}) {
      for (const char* key : {"K", "Q", "O", "T", "W", "Z", "X", "C", "V", "A", "1", "2", "3", "4", "5", "6", "7", "8", "9"})
        conflict(host, mode, std::string(prefix) + key);
      conflict(host, mode, std::string(prefix) + "Shift+T");
      conflict(host, mode, std::string(prefix) + "Shift+Z");
      assert(host.shortcuts.at(mode) == original);
    }
    allowRegistration = false;
    const int before = releases;
    bool osConflict = false;
    try { host.Shortcut({{"mode", mode}, {"accelerator", "Command+Shift+9"}}); }
    catch (const CaptureError& error) { osConflict = error.reason == "ERROR_REASON_CONFLICT"; }
    assert(osConflict && releases == before && host.shortcuts.at(mode) == original);
    allowRegistration = true;
    host.Shortcut({{"mode", mode}, {"accelerator", "Command+Option+N"}});
    host.Shortcut({{"mode", mode}, {"accelerator", "command+option+m"}});
    operations.clear();
    host.Shortcut({{"mode", mode}, {"accelerator", "Command+Option+K"}});
    assert(operations.at(operations.size() - 2) == "register");
    assert(operations.back() == "release");
    host.Shortcut({{"mode", mode}, {"accelerator", ""}});
    assert(host.shortcuts.empty() && releases == before + 4);
  }
} }
`;

// cl.exe is on PATH only inside a Visual Studio developer environment, while
// CMake's Visual Studio generator locates MSVC itself. Load the documented
// MSVC host toolchain through vswhere unless a developer shell is active.
function msvcEnvironment() {
  if (process.env.VCINSTALLDIR) return process.env;
  const arch = process.arch === "arm64" ? "arm64" : "x64";
  const vswhere = join(
    process.env["ProgramFiles(x86)"] ?? "C:\\Program Files (x86)",
    "Microsoft Visual Studio",
    "Installer",
    "vswhere.exe",
  );
  const installation = execFileSync(
    vswhere,
    [
      "-latest",
      "-products",
      "*",
      "-requires",
      arch === "arm64"
        ? "Microsoft.VisualStudio.Component.VC.Tools.ARM64"
        : "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
      "-property",
      "installationPath",
    ],
    { encoding: "utf8" },
  ).trim();
  assert.ok(
    installation,
    "Visual Studio with the MSVC C++ toolchain is installed",
  );
  const vcvars = join(
    installation,
    "VC",
    "Auxiliary",
    "Build",
    "vcvarsall.bat",
  );
  const variables = execFileSync(
    "cmd.exe",
    ["/d", "/s", "/c", `"call "${vcvars}" ${arch} >nul && set"`],
    { encoding: "utf8", windowsVerbatimArguments: true },
  );
  const environment = {};
  for (const line of variables.split(/\r?\n/)) {
    const separator = line.indexOf("=", 1);
    if (separator > 0)
      environment[line.slice(0, separator)] = line.slice(separator + 1);
  }
  assert.ok(
    environment.VCINSTALLDIR,
    "vcvarsall.bat loads the MSVC environment",
  );
  return environment;
}

function runFixture(source, flags) {
  const directory = mkdtempSync(join(tmpdir(), "lane-shortcuts-"));
  try {
    const windows = process.platform === "win32";
    const input = join(
      directory,
      flags.length ? "fixture.mm" : windows ? "fixture.cpp" : "fixture.cc",
    );
    const output = join(directory, "fixture");
    writeFileSync(input, source);
    if (windows) {
      const executable = `${output}.exe`;
      execFileSync(
        "cl.exe",
        ["/nologo", "/std:c++17", "/EHsc", "/TP", input, `/Fe:${executable}`],
        { cwd: directory, env: msvcEnvironment(), stdio: "pipe" },
      );
      execFileSync(executable, [], { stdio: "pipe" });
    } else {
      execFileSync(
        flags.length ? "clang++" : "c++",
        ["-std=c++17", ...flags, input, "-o", output],
        { stdio: "pipe" },
      );
      execFileSync(output, [], { stdio: "pipe" });
    }
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

// Windows native builds use the documented MSVC host toolchain. Keep the
// fixture off Linux, where Lane has no native desktop compiler contract.
test(
  "Windows native shortcut validation preserves bindings before registration",
  { skip: !["darwin", "win32"].includes(process.platform) },
  () => runFixture(windows, []),
);

test(
  "macOS native shortcut validation preserves bindings before registration",
  { skip: process.platform !== "darwin" },
  () =>
    runFixture(macos, [
      "-fblocks",
      "-framework",
      "Cocoa",
      "-framework",
      "Carbon",
    ]),
);
