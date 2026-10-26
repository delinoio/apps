import { createHash } from "node:crypto";
import {
  copyFileSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import {
  basename,
  dirname,
  isAbsolute,
  join,
  relative,
  resolve,
} from "node:path";
import { spawnSync } from "node:child_process";

export const windowsAppSdkKeys = Object.freeze([
  "LANE_WINDOWS_APP_SDK_INCLUDE_DIR",
  "LANE_WINDOWS_APP_SDK_PROJECTIONS_DIR",
  "LANE_WINDOWS_APP_SDK_BOOTSTRAP_DLL",
  "LANE_WINDOWS_APP_SDK_LICENSE",
]);

// Component pins from Microsoft.WindowsAppSDK 1.8.260921001's nuspec;
// official NuGet catalog packageHash values use SHA512.
// https://learn.microsoft.com/nuget/api/catalog-resource
export const windowsAppSdkPackages = Object.freeze({
  foundation: Object.freeze({
    id: "microsoft.windowsappsdk.foundation",
    version: "1.8.260803002",
    sha512:
      "k42gfJvompU7ojvo0C/w8p+GgNlqe32vBkV35LtSWuhS8gY0XHDMrEchzqwV/mhzLHZhEl1XSGxNEJCtzOSJmw==",
  }),
  runtime: Object.freeze({
    id: "microsoft.windowsappsdk.runtime",
    version: "1.8.260921001",
    sha512:
      "1wyTKgg/4hfbDyFduAOajqK8e9+wuCgA9IcqBBBtwpNaJCOjHPIu04kFkYkqemiuNAPShAyDiRUtNV+FMsdHMA==",
  }),
  cppwinrt: Object.freeze({
    id: "microsoft.windows.cppwinrt",
    version: "2.0.250303.1",
    sha512:
      "rfnscFmliz4OsAV95SkAaS9YMFrui6cI0mXSc6gRJ5eL65vyWZsAhVthtyXU5usGIGtmiX6urvGuyDlI1gvCkw==",
  }),
});

const digest = (bytes) => createHash("sha512").update(bytes).digest("base64");
const requiredProjections = [
  "winrt/base.h",
  "winrt/Microsoft.Windows.AppNotifications.h",
  "winrt/Microsoft.Windows.AppLifecycle.h",
  "winrt/Windows.Data.Xml.Dom.h",
];

function requireFile(path) {
  if (
    !isAbsolute(path) ||
    !statSync(path).isFile() ||
    statSync(path).size === 0
  )
    throw new Error(`Expected a nonempty absolute SDK file: ${path}`);
}

function validatePaths(paths) {
  for (const key of windowsAppSdkKeys)
    if (!isAbsolute(paths[key])) throw new Error(`${key} must be absolute.`);
  for (const header of ["MddBootstrap.h", "WindowsAppSDK-VersionInfo.h"])
    requireFile(join(paths.LANE_WINDOWS_APP_SDK_INCLUDE_DIR, header));
  for (const header of requiredProjections)
    requireFile(join(paths.LANE_WINDOWS_APP_SDK_PROJECTIONS_DIR, header));
  requireFile(paths.LANE_WINDOWS_APP_SDK_BOOTSTRAP_DLL);
  requireFile(paths.LANE_WINDOWS_APP_SDK_LICENSE);
  for (const notice of paths.notices) requireFile(notice.path);
  return paths;
}

function contained(root, path) {
  const suffix = relative(root, path);
  if (
    !suffix ||
    suffix === ".." ||
    suffix.startsWith("..\\") ||
    suffix.startsWith("../") ||
    isAbsolute(suffix)
  )
    throw new Error(`SDK cache path escapes its root: ${path}`);
  return path;
}

function rejectLinks(path) {
  let current = resolve(path);
  while (true) {
    try {
      if (lstatSync(current).isSymbolicLink())
        throw new Error(`SDK cache must not contain links: ${current}`);
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
    const parent = dirname(current);
    if (parent === current) break;
    current = parent;
  }
}

function removeGenerated(root, path) {
  contained(root, path);
  rejectLinks(path);
  rmSync(path, { recursive: true, force: true });
}

function run(command, args) {
  const result = spawnSync(command, args, {
    shell: false,
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
    windowsHide: true,
  });
  if (result.error) throw result.error;
  if (result.status !== 0)
    throw new Error(
      `${command} failed (${result.status ?? result.signal}): ${result.stderr || result.stdout}`,
    );
  return result.stdout;
}

export async function acquireNugetPackage(
  cache,
  spec,
  { fetchImpl = fetch } = {},
) {
  if (
    !isAbsolute(cache) ||
    !/^[a-z0-9]+(?:[.-][a-z0-9]+)*$/.test(spec.id) ||
    !/^\d+(?:\.\d+){2,3}$/.test(spec.version) ||
    !/^[A-Za-z0-9+/]{86}==$/.test(spec.sha512)
  )
    throw new Error(
      "Invalid pinned NuGet package specification or cache path.",
    );
  rejectLinks(cache);
  mkdirSync(cache, { recursive: true });
  const name = `${spec.id}.${spec.version}.nupkg`;
  const destination = contained(cache, join(cache, name));
  rejectLinks(destination);
  if (
    existsSync(destination) &&
    digest(readFileSync(destination)) === spec.sha512
  )
    return destination;
  console.info(
    JSON.stringify({
      event: "lane_windows_sdk_download",
      package: spec.id,
      version: spec.version,
    }),
  );
  const response = await fetchImpl(
    `https://api.nuget.org/v3-flatcontainer/${spec.id}/${spec.version}/${name}`,
  );
  if (!response.ok)
    throw new Error(
      `NuGet download failed for ${name}: HTTP ${response.status}`,
    );
  const bytes = Buffer.from(await response.arrayBuffer());
  if (digest(bytes) !== spec.sha512)
    throw new Error(`Official NuGet SHA512 mismatch for ${name}`);
  const temporary = mkdtempSync(join(cache, ".download-"));
  try {
    const pending = join(temporary, name);
    writeFileSync(pending, bytes, { flag: "wx" });
    removeGenerated(cache, destination);
    renameSync(pending, destination);
  } finally {
    removeGenerated(cache, temporary);
  }
  return destination;
}

export function validateArchiveEntries(text) {
  const entries = text.split(/\r?\n/).filter(Boolean);
  if (!entries.length) throw new Error("Empty SDK archive.");
  for (const entry of entries) {
    if (
      entry.startsWith("/") ||
      entry.includes("\\") ||
      entry.includes(":") ||
      entry.startsWith("-") ||
      entry.split("/").some((part) => part === ".." || part === ".") ||
      /[\x00-\x1f]/.test(entry)
    )
      throw new Error(`Unsafe SDK archive entry: ${entry}`);
  }
  return entries;
}

function fileInventory(directory, prefix = "") {
  const files = {};
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    const name = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isSymbolicLink()) throw new Error(`Unexpected SDK link: ${path}`);
    if (entry.isDirectory()) Object.assign(files, fileInventory(path, name));
    else if (entry.isFile()) files[name] = digest(readFileSync(path));
    else throw new Error(`Unexpected SDK file type: ${path}`);
  }
  return files;
}

export function findWindowsSdkMetadata(env = process.env) {
  const sdkRoot =
    env.WindowsSdkDir ||
    join(
      env["ProgramFiles(x86)"] || "C:/Program Files (x86)",
      "Windows Kits/10",
    );
  if (!isAbsolute(sdkRoot)) throw new Error("WindowsSdkDir must be absolute.");
  const union = join(sdkRoot, "UnionMetadata");
  const requested = env.WindowsSDKVersion?.replace(/[\\/]+$/, "");
  if (requested && !/^10\.0\.\d+\.\d+$/.test(requested))
    throw new Error(
      "WindowsSDKVersion must identify a Windows 10/11 SDK version.",
    );
  const versions = requested
    ? [requested]
    : existsSync(union)
      ? readdirSync(union)
          .filter((name) => /^10\.0\.\d+\.\d+$/.test(name))
          .sort((left, right) =>
            right.localeCompare(left, "en", { numeric: true }),
          )
      : [];
  for (const version of versions) {
    const metadata = join(union, version, "Windows.winmd");
    if (existsSync(metadata)) {
      requireFile(metadata);
      return {
        version,
        path: metadata,
        sha512: digest(readFileSync(metadata)),
      };
    }
  }
  throw new Error(
    "Install the Windows 10/11 SDK (Windows.winmd), or set WindowsSdkDir and WindowsSDKVersion.",
  );
}

function sdkPaths(directory, arch) {
  return {
    LANE_WINDOWS_APP_SDK_INCLUDE_DIR: join(directory, "include"),
    LANE_WINDOWS_APP_SDK_PROJECTIONS_DIR: join(directory, "projections"),
    LANE_WINDOWS_APP_SDK_BOOTSTRAP_DLL: join(
      directory,
      "foundation",
      "runtimes",
      `win-${arch}`,
      "native",
      "Microsoft.WindowsAppRuntime.Bootstrap.dll",
    ),
    LANE_WINDOWS_APP_SDK_LICENSE: join(directory, "runtime", "license.txt"),
    notices: [
      {
        path: join(directory, "foundation/license.txt"),
        name: "foundation-license.txt",
      },
      {
        path: join(directory, "runtime/NOTICE.txt"),
        name: "runtime-NOTICE.txt",
      },
      {
        path: join(directory, "cppwinrt/LICENSE"),
        name: "cppwinrt-LICENSE.txt",
      },
    ],
  };
}

export async function ensureWindowsAppSdk({
  output,
  arch = process.arch,
  env = process.env,
  fetchImpl = fetch,
  runCommand = run,
  acquirePackage = acquireNugetPackage,
} = {}) {
  const supplied = windowsAppSdkKeys.filter((key) => env[key] !== undefined);
  if (supplied.length) {
    if (
      supplied.length !== windowsAppSdkKeys.length ||
      supplied.some((key) => typeof env[key] !== "string" || !env[key].trim())
    )
      throw new Error(
        "Set all four LANE_WINDOWS_APP_SDK_* paths together, or leave all four unset.",
      );
    return validatePaths({
      ...Object.fromEntries(windowsAppSdkKeys.map((key) => [key, env[key]])),
      notices: [],
    });
  }
  if (!["x64", "arm64"].includes(arch))
    throw new Error(`Unsupported Windows SDK architecture: ${arch}`);
  if (
    typeof output !== "string" ||
    !isAbsolute(output) ||
    basename(output) !== "build" ||
    basename(dirname(output)) !== "desktop"
  )
    throw new Error(
      "Windows SDK output must be an absolute desktop/build directory.",
    );
  rejectLinks(output);
  const metadata = findWindowsSdkMetadata(env);
  const cache = contained(output, join(output, "windows-app-sdk"));
  rejectLinks(cache);
  mkdirSync(cache, { recursive: true });
  const lock = join(cache, ".lock");
  rejectLinks(lock);
  try {
    mkdirSync(lock);
  } catch (error) {
    if (error.code === "EEXIST")
      throw new Error(
        `Windows SDK provisioning is already locked: ${lock}. Remove a stale lock only after its process exits.`,
      );
    throw error;
  }
  let stage;
  try {
    const identity = {
      schema: 1,
      arch,
      packages: windowsAppSdkPackages,
      metadata,
    };
    const key = createHash("sha256")
      .update(JSON.stringify(identity))
      .digest("hex");
    const destination = contained(cache, join(cache, key));
    rejectLinks(destination);
    const receipt = join(destination, "ready.json");
    if (existsSync(receipt)) {
      try {
        const saved = JSON.parse(readFileSync(receipt, "utf8"));
        const actual = fileInventory(destination);
        delete actual["ready.json"];
        if (
          JSON.stringify(saved.identity) === JSON.stringify(identity) &&
          JSON.stringify(saved.files) === JSON.stringify(actual)
        )
          return validatePaths(sdkPaths(destination, arch));
      } catch (error) {
        console.info(
          JSON.stringify({
            event: "lane_windows_sdk_cache_rebuild",
            reason: error.message,
          }),
        );
      }
    }
    stage = mkdtempSync(join(cache, ".provision-"));
    for (const [component, spec] of Object.entries(windowsAppSdkPackages)) {
      const archive = await acquirePackage(join(cache, "packages"), spec, {
        fetchImpl,
      });
      const entries = validateArchiveEntries(
        runCommand("tar", ["-tf", archive]),
      );
      const selected = entries.filter((entry) =>
        component === "foundation"
          ? entry.startsWith("metadata/") ||
            entry === "include/MddBootstrap.h" ||
            entry ===
              `runtimes/win-${arch}/native/Microsoft.WindowsAppRuntime.Bootstrap.dll` ||
            /^(license\.txt|NOTICE\.txt)$/.test(entry)
          : component === "runtime"
            ? [
                "include/WindowsAppSDK-VersionInfo.h",
                "license.txt",
                "NOTICE.txt",
              ].includes(entry)
            : ["bin/cppwinrt.exe", "LICENSE"].includes(entry),
      );
      if (!selected.length)
        throw new Error(`Required SDK files missing from ${spec.id}`);
      const extracted = join(stage, component);
      mkdirSync(extracted);
      runCommand("tar", ["-xf", archive, "-C", extracted, "--", ...selected]);
      fileInventory(extracted);
    }
    const includes = join(stage, "include");
    mkdirSync(includes);
    copyFileSync(
      join(stage, "foundation/include/MddBootstrap.h"),
      join(includes, "MddBootstrap.h"),
    );
    copyFileSync(
      join(stage, "runtime/include/WindowsAppSDK-VersionInfo.h"),
      join(includes, "WindowsAppSDK-VersionInfo.h"),
    );
    const generator = join(stage, "cppwinrt/bin/cppwinrt.exe");
    requireFile(generator);
    const inputs = [
      "Microsoft.Windows.AppNotifications.winmd",
      "Microsoft.Windows.AppLifecycle.winmd",
    ].map((name) => join(stage, "foundation/metadata", name));
    inputs.forEach(requireFile);
    console.info(
      JSON.stringify({
        event: "lane_windows_sdk_generate",
        arch,
        windowsSdk: metadata.version,
        cppwinrt: windowsAppSdkPackages.cppwinrt.version,
      }),
    );
    runCommand(generator, [
      "-input",
      metadata.path,
      ...inputs.flatMap((path) => ["-input", path]),
      "-reference",
      join(stage, "foundation/metadata"),
      "-output",
      join(stage, "projections"),
      "-base",
    ]);
    requireFile(join(stage, "cppwinrt/LICENSE"));
    requireFile(join(stage, "foundation/license.txt"));
    requireFile(join(stage, "runtime/NOTICE.txt"));
    validatePaths(sdkPaths(stage, arch));
    writeFileSync(
      join(stage, "ready.json"),
      JSON.stringify({ identity, files: fileInventory(stage) }, null, 2),
      { flag: "wx" },
    );
    removeGenerated(cache, destination);
    renameSync(stage, destination);
    stage = undefined;
    return sdkPaths(destination, arch);
  } finally {
    if (stage) removeGenerated(cache, stage);
    removeGenerated(cache, lock);
  }
}
