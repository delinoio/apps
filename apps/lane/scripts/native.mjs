// SPDX-License-Identifier: Apache-2.0
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { join } from "node:path";

const root = fileURLToPath(new URL("../", import.meta.url));
const [platform, action] = process.argv.slice(2);
function run(command, args, cwd = root) {
  console.info(
    JSON.stringify({ event: "lane_native_start", platform, action, command }),
  );
  const result = spawnSync(command, args, {
    cwd,
    stdio: "inherit",
    shell: false,
  });
  if (result.error) throw result.error;
  if (result.status !== 0)
    throw new Error(`${command} failed (${result.status ?? result.signal})`);
}
try {
  if (platform === "android" && action === "build") {
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
    if (process.platform !== "darwin")
      throw new Error("iOS builds require macOS and Xcode");
    const cwd = join(root, "native/ios");
    if (action === "init") {
      run("xcodegen", ["generate"], cwd);
      run("pod", ["install"], cwd);
    } else {
      run(
        "xcodebuild",
        [
          "-workspace",
          "Lane.xcworkspace",
          "-scheme",
          "Lane",
          "-configuration",
          "Release",
          "-sdk",
          "iphonesimulator",
          "-destination",
          "generic/platform=iOS Simulator",
          "-derivedDataPath",
          "build",
          "CODE_SIGNING_ALLOWED=NO",
          "build",
        ],
        cwd,
      );
    }
  } else
    throw new Error(
      "Expected android build or ios init|build; use scripts/desktop.mjs for desktop",
    );
  console.info(
    JSON.stringify({ event: "lane_native_complete", platform, action }),
  );
} catch (error) {
  console.error(
    JSON.stringify({
      event: "lane_native_failed",
      platform,
      action,
      message: error.message,
    }),
  );
  process.exitCode = 1;
}
