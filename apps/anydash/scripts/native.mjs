// SPDX-License-Identifier: Apache-2.0
// Builds the AnyDash mobile hosts and the separate macOS and Windows widget
// hosts. `build` is unsigned for local use. `release` produces the signed store
// and widget packages for the unified release, with credentials supplied only
// by the protected release environment. Desktop builds use scripts/desktop.mjs.
import { spawnSync } from "node:child_process";
import {
  cpSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { png } from "./icons.mjs";
import {
  iosExportOptions,
  releaseAppxManifest,
  releaseIdentity,
} from "./release-config.mjs";

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

function xcodebuild(
  cwd,
  container,
  scheme,
  sdk,
  destination,
  settings = [],
  signed = false,
) {
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
      ...(signed ? [] : ["CODE_SIGNING_ALLOWED=NO"]),
      ...settings,
      "build",
    ],
    cwd,
  );
}

/** Developer ID signing settings for the separate macOS widget host. */
function developerIdSettings(team, version, buildNumber) {
  if (!/^[A-Z0-9]{10}$/.test(team ?? ""))
    throw new Error("ANYDASH_APPLE_TEAM_ID must be a 10-character team ID");
  return [
    "CODE_SIGN_STYLE=Manual",
    "CODE_SIGN_IDENTITY=Developer ID Application",
    `DEVELOPMENT_TEAM=${team}`,
    "ENABLE_HARDENED_RUNTIME=YES",
    `OTHER_CODE_SIGN_FLAGS=--timestamp${process.env.ANYDASH_KEYCHAIN ? ` --keychain ${process.env.ANYDASH_KEYCHAIN}` : ""}`,
    `MARKETING_VERSION=${version}`,
    `CURRENT_PROJECT_VERSION=${buildNumber}`,
  ];
}

function windowsWidgetAssets(cwd) {
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
}

/** Signed builds for the unified release; outputs land in each host's build/release. */
function releaseBuild() {
  if (platform === "android") {
    requireBundle();
    const { version, buildNumber } = releaseIdentity();
    for (const name of [
      "ANYDASH_ANDROID_KEYSTORE_PATH",
      "ANYDASH_ANDROID_KEYSTORE_PASSWORD",
      "ANYDASH_ANDROID_KEY_ALIAS",
      "ANYDASH_ANDROID_KEY_PASSWORD",
    ])
      if (!process.env[name]) throw new Error(`Missing ${name}`);
    run(
      "java",
      [
        "-classpath",
        "gradle/wrapper/gradle-wrapper.jar",
        "org.gradle.wrapper.GradleWrapperMain",
        "--no-daemon",
        `-PanydashVersionCode=${buildNumber}`,
        `-PanydashVersionName=${version}`,
        ":app:bundleRelease",
      ],
      join(root, "native/android"),
    );
  } else if (platform === "ios") {
    requireMac("iOS");
    requireBundle();
    const { version, buildNumber } = releaseIdentity();
    const team = process.env.ANYDASH_APPLE_TEAM_ID;
    const exportOptions = iosExportOptions(team);
    const cwd = join(root, "native/ios");
    const key = [
      "-allowProvisioningUpdates",
      "-authenticationKeyPath",
      process.env.APPLE_API_KEY_PATH,
      "-authenticationKeyID",
      process.env.APPLE_API_KEY,
      "-authenticationKeyIssuerID",
      process.env.APPLE_API_ISSUER,
    ];
    if (key.some((value) => !value))
      throw new Error(
        "APPLE_API_KEY_PATH, APPLE_API_KEY and APPLE_API_ISSUER are required",
      );
    rmSync(join(cwd, "build/release"), { recursive: true, force: true });
    run(
      "xcodebuild",
      [
        "-workspace",
        "AnyDash.xcworkspace",
        "-scheme",
        "AnyDash",
        "-configuration",
        "Release",
        "-destination",
        "generic/platform=iOS",
        "-archivePath",
        "build/release/AnyDash.xcarchive",
        ...key,
        "CODE_SIGN_STYLE=Automatic",
        `DEVELOPMENT_TEAM=${team}`,
        `MARKETING_VERSION=${version}`,
        `CURRENT_PROJECT_VERSION=${buildNumber}`,
        ...iosSettings(),
        "archive",
      ],
      cwd,
    );
    mkdirSync(join(cwd, "build/release"), { recursive: true });
    writeFileSync(
      join(cwd, "build/release/ExportOptions.plist"),
      exportOptions,
    );
    run(
      "xcodebuild",
      [
        "-exportArchive",
        "-archivePath",
        "build/release/AnyDash.xcarchive",
        "-exportPath",
        "build/release/export",
        "-exportOptionsPlist",
        "build/release/ExportOptions.plist",
        ...key,
      ],
      cwd,
    );
  } else if (platform === "macos-widgets") {
    requireMac("macOS widget");
    const { version, buildNumber } = releaseIdentity();
    const cwd = join(root, "native/widgets/macos");
    run("xcodegen", ["generate"], cwd);
    rmSync(join(cwd, "build"), { recursive: true, force: true });
    xcodebuild(
      cwd,
      ["-project", "AnyDashWidgets.xcodeproj"],
      "AnyDashWidgetHost",
      "macosx",
      "generic/platform=macOS",
      developerIdSettings(
        process.env.ANYDASH_APPLE_TEAM_ID,
        version,
        buildNumber,
      ),
      true,
    );
    const app = join(cwd, "build/Build/Products/Release/AnyDash Widgets.app");
    if (!existsSync(app)) throw new Error("The widget host build is missing.");
    mkdirSync(join(cwd, "build/release"), { recursive: true });
    cpSync(app, join(cwd, "build/release/AnyDash Widgets.app"), {
      recursive: true,
      verbatimSymlinks: true,
    });
  } else if (platform === "windows-widgets") {
    if (process.platform !== "win32" || process.arch !== "x64")
      throw new Error("The Widgets Board provider packages on Windows x64.");
    const { version } = releaseIdentity();
    const cwd = join(root, "native/widgets/windows/Provider");
    windowsWidgetAssets(cwd);
    const manifest = join(cwd, "Package.appxmanifest");
    const original = readFileSync(manifest, "utf8");
    // The MSIX publisher must equal the signing certificate subject; the
    // committed manifest keeps the development identity.
    writeFileSync(
      manifest,
      releaseAppxManifest(original, {
        publisher: process.env.ANYDASH_WINDOWS_PUBLISHER,
        version,
      }),
    );
    const packages = join(cwd, "AppPackages");
    try {
      rmSync(packages, { recursive: true, force: true });
      run(
        "dotnet",
        [
          "build",
          "-c",
          "Release",
          "-p:Platform=x64",
          `-p:Version=${version}`,
          `-p:AppxPackageDir=${packages}\\`,
        ],
        cwd,
      );
    } finally {
      writeFileSync(manifest, original);
    }
    const found = [];
    const visit = (directory) => {
      for (const entry of readdirSync(directory, { withFileTypes: true })) {
        const path = join(directory, entry.name);
        if (entry.isDirectory()) visit(path);
        else if (entry.name.endsWith(".msix")) found.push(path);
      }
    };
    visit(packages);
    if (found.length !== 1)
      throw new Error(`Expected one widget MSIX, found ${found.length}.`);
    const output = join(root, "native/widgets/windows/build/release");
    mkdirSync(output, { recursive: true });
    cpSync(found[0], join(output, "AnyDash.Widgets.msix"));
  } else throw new Error(`No release build for ${platform}.`);
}

try {
  if (action === "release") releaseBuild();
  else if (platform === "android" && action === "build") {
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
    windowsWidgetAssets(cwd);
    run("dotnet", ["build", "-c", "Release", "-p:Platform=x64"], cwd);
  } else
    throw new Error(
      "Expected android build|release, ios init|build|release, macos-widgets build|release or windows-widgets build|release; use scripts/desktop.mjs for desktop",
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
