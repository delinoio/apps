// SPDX-License-Identifier: Apache-2.0
// Builds the AnyDash mobile hosts and the separate macOS widget host. Local
// builds are unsigned; signing and store credentials stay in the protected
// release environments. Desktop builds use scripts/desktop.mjs.
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

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
function xcodebuild(cwd, container, scheme, sdk, destination) {
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
  } else
    throw new Error(
      "Expected android build, ios init|build or macos-widgets build; use scripts/desktop.mjs for desktop",
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
