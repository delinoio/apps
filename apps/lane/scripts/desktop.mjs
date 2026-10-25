// SPDX-License-Identifier: Apache-2.0
import { spawnSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { ensureRuntime } from "@lynx-js/lynxtron/runtime-manager";

const root = fileURLToPath(new URL("../", import.meta.url));
const require = createRequire(import.meta.url);
const output = join(root, "native/desktop/build");
const scheme = process.env.LANE_URL_SCHEME || "lane-dev";
const action = process.argv[2];
function run(command, args) {
  console.info(JSON.stringify({ event: "lane_desktop_command", command }));
  const result = spawnSync(command, args, {
    cwd: root,
    stdio: "inherit",
    shell: false,
  });
  if (result.error) throw result.error;
  if (result.status !== 0)
    throw new Error(`${command} failed (${result.status ?? result.signal})`);
}
try {
  if (!["darwin", "win32"].includes(process.platform))
    throw new Error("Lane desktop supports macOS and Windows.");
  if (!["lane-dev", "lane"].includes(scheme))
    throw new Error("LANE_URL_SCHEME must be lane-dev or lane.");
  if (!["build", "run"].includes(action))
    throw new Error("Expected build or run.");
  const app =
    process.platform === "darwin"
      ? join(output, "Lane.app")
      : join(output, "application");
  const executable =
    process.platform === "darwin"
      ? join(app, "Contents/MacOS/lynxtron")
      : join(app, "lynxtron.exe");
  const resources =
    process.platform === "darwin"
      ? join(app, "Contents/Resources/app")
      : join(app, "resources/app");
  if (action === "build") {
    const runtime = await ensureRuntime({
      variant: "devtool",
      allowFetch: false,
    });
    const runtimeRoot =
      process.platform === "darwin"
        ? resolve(runtime.executablePath, "../../..")
        : dirname(runtime.executablePath);
    // Clear only the generated application; retain incremental native compilation.
    rmSync(app, { recursive: true, force: true });
    cpSync(runtimeRoot, app, {
      recursive: true,
      dereference: false,
      verbatimSymlinks: true,
    });
    mkdirSync(resources, { recursive: true });
    const nativeBuild = join(output, "addon");
    const options = [
      "-S",
      "native/desktop",
      "-B",
      nativeBuild,
      `-DLANE_NODE_API_INCLUDE=${require("node-api-headers").include_dir}`,
      `-DLANE_NATIVE_TESTING=${process.env.LANE_NATIVE_TESTING === "1" ? "ON" : "OFF"}`,
    ];
    if (process.platform === "win32") {
      options.push(
        `-DLANE_NODE_IMPORT_LIBRARY=${join(runtimeRoot, "lynxtron.dll.lib")}`,
      );
      for (const key of [
        "LANE_WINDOWS_APP_SDK_INCLUDE_DIR",
        "LANE_WINDOWS_APP_SDK_PROJECTIONS_DIR",
        "LANE_WINDOWS_APP_SDK_BOOTSTRAP_DLL",
        "LANE_WINDOWS_APP_SDK_LICENSE",
      ])
        if (process.env[key]) options.push(`-D${key}=${process.env[key]}`);
    }
    run("cmake", options);
    run("cmake", [
      "--build",
      nativeBuild,
      "--config",
      "Release",
      "--parallel",
      "4",
    ]);
    const binaryRoot =
      process.platform === "win32" ? join(nativeBuild, "Release") : nativeBuild;
    cpSync(
      join(binaryRoot, "lane_host.node"),
      join(resources, "lane_host.node"),
    );
    if (process.platform === "win32") {
      cpSync(
        join(binaryRoot, "Microsoft.WindowsAppRuntime.Bootstrap.dll"),
        join(resources, "Microsoft.WindowsAppRuntime.Bootstrap.dll"),
      );
      cpSync(join(binaryRoot, "licenses"), join(resources, "licenses"), {
        recursive: true,
      });
    }
    await build({
      absWorkingDir: root,
      entryPoints: ["native/desktop/main.mjs"],
      outfile: join(resources, "main.mjs"),
      bundle: true,
      platform: "node",
      format: "esm",
      target: "node22",
      external: ["lynxtron"],
    });
    writeFileSync(
      join(resources, "package.json"),
      JSON.stringify({
        name: "lane",
        version: "0.1.0",
        type: "module",
        main: "main.mjs",
        scheme,
      }),
    );
    for (const file of ["main.lynx.bundle", "LICENSE", "NOTICE"])
      cpSync(join(root, "dist", file), join(resources, file));
    cpSync(join(root, "dist/licenses"), join(resources, "licenses/bundle"), {
      recursive: true,
    });
    const vendor = join(root, "native/store/vendor");
    for (const notice of [
      "sqlite/LICENSE",
      "ada/LICENSE-APACHE",
      "ada/LICENSE-MIT",
      "json/LICENSE.MIT",
      "picosha2/LICENSE",
      "utf8proc/LICENSE.md",
    ]) {
      mkdirSync(dirname(join(resources, "licenses/lane-store", notice)), {
        recursive: true,
      });
      cpSync(
        join(vendor, notice),
        join(resources, "licenses/lane-store", notice),
      );
    }
    cpSync(
      join(
        dirname(require.resolve("node-api-headers/package.json")),
        "LICENSE",
      ),
      join(resources, "licenses/node-api-headers.txt"),
    );
    const arch = { arm64: "arm64", x64: "amd64", ia32: "386" }[process.arch];
    const os = process.platform === "darwin" ? "darwin" : "windows";
    const helperName = process.platform === "darwin" ? "lane" : "lane.exe";
    const packaged = resolve(
      root,
      `../../packages/lane/dist/artifacts/lane-${os}-${arch}/${helperName}`,
    );
    const helper =
      process.env.LANE_SYNC_HELPER ||
      (existsSync(packaged) ? packaged : undefined);
    if (helper) {
      if (!existsSync(helper))
        throw new Error("Configured Lane sync helper is missing.");
      const source = dirname(helper);
      const destination = join(resources, "helpers");
      mkdirSync(destination, { recursive: true });
      for (const name of ["LICENSE", "NOTICE", "THIRD_PARTY_NOTICES"])
        cpSync(join(source, name), join(destination, name), {
          recursive: true,
        });
      cpSync(helper, join(destination, helperName));
    } else
      console.info(
        JSON.stringify({
          event: "lane_sync_helper_unavailable",
          localModeAvailable: true,
        }),
      );
    if (process.platform === "darwin") {
      const plist = join(app, "Contents/Info.plist");
      for (const [key, value] of Object.entries({
        CFBundleIdentifier: "io.delino.lane",
        CFBundleName: "Lane",
        CFBundleDisplayName: "Lane",
        LaneURLScheme: scheme,
        CFBundleURLTypes: [
          { CFBundleURLName: "io.delino.lane", CFBundleURLSchemes: [scheme] },
        ],
      }))
        run("plutil", ["-replace", key, "-json", JSON.stringify(value), plist]);
      // Local builds use ad-hoc signing; release credentials remain external.
      run("codesign", ["--force", "--deep", "--sign", "-", app]);
    }
    console.info(JSON.stringify({ event: "lane_desktop_built" }));
  } else {
    if (!existsSync(executable))
      throw new Error("Run pnpm build:desktop first.");
    run(executable, []);
  }
} catch (error) {
  console.error(
    JSON.stringify({ event: "lane_desktop_failed", message: error.message }),
  );
  process.exitCode = 1;
}
