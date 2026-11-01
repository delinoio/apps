// SPDX-License-Identifier: Apache-2.0
// Builds and launches the AnyDash Lynxtron desktop host (macOS arm64/x64,
// Windows x64). Local builds are ad-hoc signed and cannot self-update; release
// signing, notarization and the update key stay in the protected release
// environment.
import { spawnSync } from "node:child_process";
import {
  cpSync,
  existsSync,
  mkdirSync,
  rmSync,
  writeFileSync,
  readFileSync,
} from "node:fs";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { ensureRuntime } from "@lynx-js/lynxtron/runtime-manager";
import { ico, png } from "./icons.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const output = join(root, "native/desktop/build");
const action = process.argv[2];
const { version } = JSON.parse(
  readFileSync(join(root, "package.json"), "utf8"),
);

function run(command, args) {
  console.info(JSON.stringify({ event: "anydash_desktop_command", command }));
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

/** The collector executable the package embeds; built from cmds/anydash. */
function collectorSource() {
  const name = process.platform === "win32" ? "anydash.exe" : "anydash";
  const candidates = [
    process.env.ANYDASH_COLLECTOR_EXECUTABLE,
    resolve(root, "../../target/release", name),
  ].filter(Boolean);
  return candidates.find((path) => existsSync(path));
}

try {
  if (!["darwin", "win32"].includes(process.platform))
    throw new Error("AnyDash desktop supports macOS and Windows.");
  if (process.platform === "win32" && process.arch !== "x64")
    throw new Error("The Windows desktop app ships for x64 only.");
  if (!["build", "run"].includes(action))
    throw new Error("Expected build or run.");
  const app =
    process.platform === "darwin"
      ? join(output, "AnyDash.app")
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
    const release = process.env.ANYDASH_RELEASE_BUILD === "1";
    if (release && !process.env.ANYDASH_UPDATE_PUBLIC_KEY)
      throw new Error("Release builds require ANYDASH_UPDATE_PUBLIC_KEY.");
    const runtime = await ensureRuntime({
      variant: "devtool",
      allowFetch: false,
    });
    const runtimeRoot =
      process.platform === "darwin"
        ? resolve(runtime.executablePath, "../../..")
        : dirname(runtime.executablePath);
    rmSync(app, { recursive: true, force: true });
    cpSync(runtimeRoot, app, {
      recursive: true,
      dereference: false,
      verbatimSymlinks: true,
    });
    mkdirSync(resources, { recursive: true });
    await build({
      absWorkingDir: root,
      entryPoints: ["native/desktop/main.mjs"],
      outfile: join(resources, "main.mjs"),
      bundle: true,
      platform: "node",
      format: "esm",
      target: "node22",
      external: ["@lynx-js/lynxtron"],
    });
    writeFileSync(
      join(resources, "package.json"),
      JSON.stringify({
        name: "anydash",
        version,
        type: "module",
        main: "main.mjs",
        testing: process.env.ANYDASH_NATIVE_TESTING === "1",
        releaseBuild: release,
        api:
          process.env.PUBLIC_ANYDASH_API_URL || "https://anydash.api.delino.io",
        anydashResource:
          process.env.PUBLIC_ANYDASH_RESOURCE ||
          "https://anydash.api.delino.io",
        delcoreResource:
          process.env.PUBLIC_DELCORE_RESOURCE ||
          "https://delcore.api.delino.io",
        logtoEndpoint: process.env.PUBLIC_LOGTO_ENDPOINT || "",
        logtoAppId: process.env.PUBLIC_ANYDASH_NATIVE_APP_ID || "",
        updatePublicKey: process.env.ANYDASH_UPDATE_PUBLIC_KEY || "",
        teamId: process.env.ANYDASH_APPLE_TEAM_ID || "",
        publisher: process.env.ANYDASH_WINDOWS_PUBLISHER || "",
      }),
    );
    for (const file of ["main.lynx.bundle", "LICENSE", "NOTICE"])
      if (existsSync(join(root, "dist", file)))
        cpSync(join(root, "dist", file), join(resources, file));
    if (!existsSync(join(resources, "main.lynx.bundle")))
      throw new Error("Run pnpm build first.");
    cpSync(join(root, "dist/licenses"), join(resources, "licenses/bundle"), {
      recursive: true,
    });
    cpSync(join(root, "native/licenses"), join(resources, "licenses/runtime"), {
      recursive: true,
    });
    const collector = collectorSource();
    if (collector) {
      mkdirSync(join(resources, "bin"), { recursive: true });
      cpSync(
        collector,
        join(
          resources,
          "bin",
          process.platform === "win32" ? "anydash.exe" : "anydash",
        ),
      );
    } else
      console.info(
        JSON.stringify({
          event: "anydash_collector_not_embedded",
          hint: "cargo build --release -p anydash",
        }),
      );
    writeFileSync(
      join(resources, "TrayTemplate.png"),
      png(16, { template: true }),
    );
    writeFileSync(
      join(resources, "TrayTemplate@2x.png"),
      png(32, { template: true }),
    );
    writeFileSync(join(resources, "AnyDash.ico"), ico());
    if (process.platform === "darwin") {
      const iconset = join(output, "AnyDash.iconset");
      rmSync(iconset, { recursive: true, force: true });
      mkdirSync(iconset, { recursive: true });
      for (const size of [16, 32, 128, 256, 512]) {
        writeFileSync(join(iconset, `icon_${size}x${size}.png`), png(size));
        writeFileSync(
          join(iconset, `icon_${size}x${size}@2x.png`),
          png(size * 2),
        );
      }
      run("iconutil", [
        "-c",
        "icns",
        iconset,
        "-o",
        join(app, "Contents/Resources/AnyDash.icns"),
      ]);
      rmSync(iconset, { recursive: true, force: true });
      const plist = join(app, "Contents/Info.plist");
      for (const [key, value] of Object.entries({
        CFBundleIdentifier: "io.delino.anydash",
        CFBundleName: "AnyDash",
        CFBundleDisplayName: "AnyDash",
        CFBundleShortVersionString: version,
        CFBundleIconFile: "AnyDash.icns",
        LSMinimumSystemVersion: "14.0",
        CFBundleURLTypes: [
          {
            CFBundleURLName: "io.delino.anydash",
            CFBundleURLSchemes: ["anydash"],
          },
        ],
      }))
        run("plutil", ["-replace", key, "-json", JSON.stringify(value), plist]);
      // Local builds use ad-hoc signing; release credentials remain external.
      run("codesign", ["--force", "--deep", "--sign", "-", app]);
    }
    console.info(
      JSON.stringify({
        event: "anydash_desktop_built",
        collectorEmbedded: !!collector,
      }),
    );
  } else {
    if (!existsSync(executable))
      throw new Error("Run pnpm build:desktop first.");
    run(executable, []);
  }
} catch (error) {
  console.error(
    JSON.stringify({ event: "anydash_desktop_failed", message: error.message }),
  );
  process.exitCode = 1;
}
