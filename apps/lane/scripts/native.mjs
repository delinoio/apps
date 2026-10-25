// SPDX-License-Identifier: Apache-2.0
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { existsSync } from "node:fs";

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
  if (platform === "desktop" && action === "build") {
    if (!["darwin", "win32"].includes(process.platform))
      throw new Error("Use macOS or Windows for Clay builds");
    const architecture = { arm64: "arm64", x64: "amd64", ia32: "386" }[
      process.arch
    ];
    if (!architecture) throw new Error("Unsupported desktop CPU architecture");
    const os = process.platform === "darwin" ? "darwin" : "windows";
    const executable = process.platform === "win32" ? "lane.exe" : "lane";
    const packagedHelper = join(
      root,
      "../../packages/lane/dist/artifacts",
      `lane-${os}-${architecture}`,
      executable,
    );
    const helper =
      process.env.LANE_SYNC_HELPER ||
      (existsSync(packagedHelper) ? packagedHelper : "");
    if (helper && !existsSync(helper))
      throw new Error("Configured Lane sync helper is missing");
    run("cmake", [
      "-S",
      "native/desktop",
      "-B",
      "native/desktop/build",
      `-DLANE_SYNC_HELPER=${helper}`,
      `-DLANE_URL_SCHEME=${process.env.LANE_URL_SCHEME || "lane-dev"}`,
    ]);
    run("cmake", ["--build", "native/desktop/build", "--config", "Release"]);
  } else if (platform === "desktop" && action === "run") {
    if (process.platform === "darwin")
      run("open", ["native/desktop/build/Lane.app"]);
    else if (process.platform === "win32")
      run(join(root, "native/desktop/build/Release/Lane.exe"), []);
    else throw new Error("Use macOS or Windows for Clay hosts");
  } else if (platform === "android" && action === "build") {
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
      "Expected desktop build|run, android build, or ios init|build",
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
