// SPDX-License-Identifier: Apache-2.0
// Builds the AnyDash mobile hosts and the separate macOS widget host. Local
// builds are unsigned; signing and store credentials stay in the protected
// release environments. Desktop builds use scripts/desktop.mjs.
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { png } from "./icons.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const [platform, action] = process.argv.slice(2);

function run(command, args, cwd = root) {
  console.info(
    JSON.stringify({
      event: "anydash_native_command",
      platform,
      action,
      command,
    }),
  );
  const result = spawnSync(command, args, {
    cwd,
    stdio: "inherit",
    shell: false,
    // CocoaPods requires a UTF-8 locale.
    env: { ...process.env, LANG: process.env.LANG || "en_US.UTF-8" },
  });
  if (result.error) throw result.error;
  if (result.status !== 0)
    throw new Error(`${command} failed (${result.status ?? result.signal})`);
}

function requireMac(name) {
  if (process.platform !== "darwin")
    throw new Error(`${name} builds require macOS and Xcode.`);
}

function requireBundle() {
  if (!existsSync(join(root, "dist/main.lynx.bundle")))
    throw new Error("Run pnpm build first.");
}

/** Unsigned simulator or macOS build with the given workspace or project. */
/** Public build settings for the iOS host; credentials never enter the build. */
function iosSettings() {
  const settings = {
    ANYDASH_API_URL: process.env.PUBLIC_ANYDASH_API_URL,
    ANYDASH_RESOURCE: process.env.PUBLIC_ANYDASH_RESOURCE,
    DELCORE_RESOURCE: process.env.PUBLIC_DELCORE_RESOURCE,
    LOGTO_ENDPOINT: process.env.PUBLIC_LOGTO_ENDPOINT,
    LOGTO_NATIVE_APP_ID: process.env.PUBLIC_ANYDASH_NATIVE_APP_ID,
  };
  return Object.entries(settings)
    .filter(([, value]) => value)
    .map(([key, value]) => {
      if (!/^[A-Za-z0-9:/._-]+$/.test(value))
        throw new Error(`${key} has unsupported characters.`);
      return `${key}=${value}`;
    });
}

function xcodebuild(cwd, container, scheme, sdk, destination, settings = []) {
  run(
    "xcodebuild",
    [
      ...container,
      "-scheme",
      scheme,
      "-configuration",
      "Release",
      "-sdk",
      sdk,
      "-destination",
      destination,
      "-derivedDataPath",
      "build",
      "CODE_SIGNING_ALLOWED=NO",
      ...settings,
      "build",
    ],
    cwd,
  );
}

try {
  if (platform === "android" && action === "build") {
    requireBundle();
    run(
      "java",
      [
        "-classpath",
        "gradle/wrapper/gradle-wrapper.jar",
        "org.gradle.wrapper.GradleWrapperMain",
        "--no-daemon",
        ":app:assembleRelease",
      ],
      join(root, "native/android"),
    );
  } else if (platform === "ios" && ["init", "build"].includes(action)) {
    requireMac("iOS");
    const cwd = join(root, "native/ios");
    if (action === "init") {
      run("xcodegen", ["generate"], cwd);
      run("pod", ["install"], cwd);
    } else {
      requireBundle();
      xcodebuild(
        cwd,
        ["-workspace", "AnyDash.xcworkspace"],
        "AnyDash",
        "iphonesimulator",
        "generic/platform=iOS Simulator",
        iosSettings(),
      );
    }
  } else if (platform === "macos-widgets" && action === "build") {
    requireMac("macOS widget");
    const cwd = join(root, "native/widgets/macos");
    run("xcodegen", ["generate"], cwd);
    xcodebuild(
      cwd,
      ["-project", "AnyDashWidgets.xcodeproj"],
      "AnyDashWidgetHost",
      "macosx",
      "generic/platform=macOS",
    );
  } else if (platform === "windows-widgets" && action === "build") {
    if (process.platform !== "win32" || process.arch !== "x64")
      throw new Error("The Widgets Board provider packages on Windows x64.");
    const cwd = join(root, "native/widgets/windows/Provider");
    // Package images come from the AnyDash mark; no image binaries are committed.
    mkdirSync(join(cwd, "Assets"), { recursive: true });
    for (const [name, size] of [
      ["StoreLogo.png", 50],
      ["Square44x44Logo.png", 44],
      ["Square150x150Logo.png", 150],
      ["WidgetIcon.png", 48],
      ["WidgetScreenshot.png", 300],
    ])
      writeFileSync(join(cwd, "Assets", name), png(size));
    run("dotnet", ["build", "-c", "Release", "-p:Platform=x64"], cwd);
  } else
    throw new Error(
      "Expected android build, ios init|build, macos-widgets build or windows-widgets build; use scripts/desktop.mjs for desktop",
    );
  console.info(
    JSON.stringify({ event: "anydash_native_complete", platform, action }),
  );
} catch (error) {
  console.error(
    JSON.stringify({
      event: "anydash_native_failed",
      platform,
      action,
      message: error.message,
    }),
  );
  process.exitCode = 1;
}
