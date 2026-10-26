// SPDX-License-Identifier: Apache-2.0
import { spawnSync } from "node:child_process";
import {
  cpSync,
  existsSync,
  mkdirSync,
  rmSync,
  writeFileSync,
  readFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve, isAbsolute } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { homedir } from "node:os";
import { packageChromeRelay } from "./desktop-relay.mjs";
import {
  ensureWindowsAppSdk,
  windowsAppSdkKeys,
} from "./desktop-windows-sdk.mjs";
import {
  installNativeMessaging,
  removeNativeMessaging,
} from "../../lane-chrome-extension/scripts/native-registration.mjs";
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
    windowsHide: true,
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
  if (!["build", "run", "install", "uninstall"].includes(action))
    throw new Error("Expected build, run, install, or uninstall.");
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
    if (
      process.env.LANE_RELEASE_TAG &&
      (!/^delino@v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(
        process.env.LANE_RELEASE_TAG,
      ) ||
        !/^[a-f0-9]{40}$/.test(process.env.LANE_SOURCE_REVISION || ""))
    )
      throw new Error(
        "Desktop release builds require a valid tag and source revision.",
      );
    const runtime = await ensureRuntime({
      variant: "devtool",
      allowFetch: false,
    });
    const runtimeRoot =
      process.platform === "darwin"
        ? resolve(runtime.executablePath, "../../..")
        : dirname(runtime.executablePath);
    const windowsSdk =
      process.platform === "win32"
        ? await ensureWindowsAppSdk({ output })
        : undefined;
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
      options.push("-A", process.arch === "arm64" ? "ARM64" : "x64");
      options.push(
        `-DLANE_NODE_IMPORT_LIBRARY=${join(runtimeRoot, "lynxtron.dll.lib")}`,
      );
      for (const key of windowsAppSdkKeys)
        options.push(`-D${key}=${windowsSdk[key]}`);
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
    await packageChromeRelay({ root, output, resources, binaryRoot });
    cpSync(
      join(binaryRoot, "lane_host.node"),
      join(resources, "lane_host.node"),
    );
    if (process.platform === "win32") {
      run(join(binaryRoot, "LaneIcon.exe"), [
        executable,
        join(root, "native/desktop/windows/Lane.ico"),
      ]);
      cpSync(
        join(root, "native/desktop/windows/Lane.ico"),
        join(resources, "Lane.ico"),
      );
      cpSync(
        join(binaryRoot, "Microsoft.WindowsAppRuntime.Bootstrap.dll"),
        join(resources, "Microsoft.WindowsAppRuntime.Bootstrap.dll"),
      );
      cpSync(join(binaryRoot, "licenses"), join(resources, "licenses"), {
        recursive: true,
      });
      for (const { path, name } of windowsSdk.notices ?? [])
        cpSync(path, join(resources, "licenses/windows-app-sdk", name));
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
        releaseTag: process.env.LANE_RELEASE_TAG || "",
        sourceRevision: process.env.LANE_SOURCE_REVISION || "",
        testing: process.env.LANE_NATIVE_TESTING === "1",
        chromeExtensionIds: (process.env.LANE_CHROME_EXTENSION_IDS || "")
          .split(",")
          .map((value) => value.trim())
          .filter(Boolean),
      }),
    );
    await build({
      absWorkingDir: root,
      entryPoints: ["native/desktop/update-install.mjs"],
      outfile: join(resources, "update-install.mjs"),
      bundle: true,
      platform: "node",
      format: "esm",
      target: "node22",
    });
    for (const file of ["main.lynx.bundle", "LICENSE", "NOTICE"])
      cpSync(join(root, "dist", file), join(resources, file));
    cpSync(join(root, "dist/licenses"), join(resources, "licenses/bundle"), {
      recursive: true,
    });
    cpSync(join(root, "native/licenses"), join(resources, "licenses/runtime"), {
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
      cpSync(
        join(root, "native/desktop/macos/Lane.icns"),
        join(app, "Contents/Resources/Lane.icns"),
      );
      const plist = join(app, "Contents/Info.plist");
      for (const [key, value] of Object.entries({
        CFBundleIdentifier: "io.delino.lane",
        CFBundleName: "Lane",
        CFBundleDisplayName: "Lane",
        CFBundleIconFile: "Lane.icns",
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
  } else if (action === "install" || action === "uninstall") {
    const testRoot = process.env.LANE_NATIVE_INSTALL_TEST_ROOT;
    // Relocating a manifest cannot isolate HKCU. Refuse Windows probes until
    // registration has a separate test backend; never alter the user's key.
    if (testRoot && process.platform === "win32")
      throw new Error(
        "Windows scratch installation is unavailable: Chrome registration cannot be isolated from HKCU.",
      );
    if (
      testRoot &&
      (process.env.LANE_NATIVE_TESTING !== "1" || !isAbsolute(testRoot))
    )
      throw new Error(
        "Scratch installation requires explicit native testing and an absolute test root.",
      );
    if (
      testRoot &&
      action === "install" &&
      !JSON.parse(readFileSync(join(resources, "package.json"), "utf8")).testing
    )
      throw new Error(
        "Scratch installation requires a native-testing package.",
      );
    const installed = testRoot
      ? join(
          testRoot,
          process.platform === "darwin"
            ? "Applications/Lane.app"
            : "Programs/Lane",
        )
      : process.platform === "darwin"
        ? join(homedir(), "Applications/Lane.app")
        : join(process.env.LOCALAPPDATA, "Programs/Lane");
    const installedResources =
      process.platform === "darwin"
        ? join(installed, "Contents/Resources/app")
        : join(installed, "resources/app");
    if (action === "install") {
      if (!existsSync(executable))
        throw new Error("Run pnpm build:desktop first.");
      if (existsSync(installed))
        throw new Error(
          "A Lane installation already exists. Quit and uninstall it before replacing it.",
        );
      mkdirSync(dirname(installed), { recursive: true });
      cpSync(app, installed, {
        recursive: true,
        dereference: false,
        verbatimSymlinks: true,
      });
    }
    const config = JSON.parse(
      readFileSync(join(installedResources, "package.json"), "utf8"),
    );
    if (config.name !== "lane")
      throw new Error("The installed directory is not owned by Lane.");
    const registration = {
      extensionIds: config.chromeExtensionIds,
      ...(testRoot ? { home: testRoot } : {}),
      relayExecutable: join(
        installedResources,
        "relay",
        process.platform === "darwin"
          ? "lane-chrome-host"
          : "lane-chrome-host.exe",
      ),
      ...(process.platform === "win32"
        ? {
            manifestDirectory: join(
              testRoot || process.env.LOCALAPPDATA,
              "io.delino.lane/native-messaging",
            ),
          }
        : {}),
    };
    if (action === "install") {
      if (config.chromeExtensionIds?.length)
        await installNativeMessaging(registration);
      else
        console.info(
          JSON.stringify({ event: "lane_chrome_registration_unconfigured" }),
        );
    } else {
      if (config.chromeExtensionIds?.length)
        await removeNativeMessaging(registration);
      rmSync(installed, { recursive: true });
    }
    console.info(
      JSON.stringify({
        event:
          action === "install"
            ? "lane_desktop_installed"
            : "lane_desktop_uninstalled",
      }),
    );
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
