// SPDX-License-Identifier: Apache-2.0
// Verifies the installed platform package before the launcher executes it. The
// launcher never downloads or compiles a fallback executable.
import { createHash } from "node:crypto";
import { lstatSync, readFileSync, realpathSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, resolve, sep } from "node:path";

/** Every CLI target in the AnyDash support matrix. */
export const Platform = Object.freeze({
  DarwinArm64: "darwin-arm64",
  DarwinX64: "darwin-x64",
  LinuxArm64: "linux-arm64",
  LinuxX64: "linux-x64",
  WindowsArm64: "windows-arm64",
  WindowsX64: "windows-x64",
});
export const platforms = Object.freeze(Object.values(Platform));

export function platformKey(os = process.platform, arch = process.arch) {
  const key = `${os === "win32" ? "windows" : os}-${arch}`;
  if (!platforms.includes(key))
    throw new Error(`unsupported platform ${os}/${arch}`);
  return key;
}

export function platformPackage(key) {
  if (!platforms.includes(key)) throw new Error(`unsupported platform ${key}`);
  const [os, cpu] = key.split("-");
  return {
    name: `@delinoio/anydash-${key}`,
    os: os === "windows" ? "win32" : os,
    cpu,
  };
}

export const executableName = (key) =>
  key.startsWith("windows-") ? "anydash.exe" : "anydash";

const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

export function verifiedExecutable(root, key = platformKey()) {
  if (!platforms.includes(key)) throw new Error(`unsupported platform ${key}`);
  const pkg = JSON.parse(readFileSync(resolve(root, "package.json"), "utf8"));
  const manifest = JSON.parse(
    readFileSync(resolve(root, "manifest.json"), "utf8"),
  );
  if (
    manifest.schemaVersion !== 1 ||
    manifest.version !== pkg.version ||
    !/^\d+\.\d+\.\d+$/.test(pkg.version)
  )
    throw new Error("manifest version mismatch");
  if (!/^[a-f0-9]{40}$/.test(manifest.sourceRevision ?? ""))
    throw new Error("invalid source revision manifest");
  const entry = manifest.artifacts?.[key];
  if (!entry) throw new Error(`missing artifact for ${key}`);
  const info = platformPackage(key);
  if (pkg.optionalDependencies?.[info.name] !== pkg.version)
    throw new Error(`invalid optional dependency for ${key}`);
  let platformRoot;
  let metadata;
  try {
    platformRoot = dirname(
      createRequire(resolve(root, "package.json")).resolve(
        `${info.name}/package.json`,
      ),
    );
    metadata = JSON.parse(
      readFileSync(resolve(platformRoot, "package.json"), "utf8"),
    );
  } catch {
    throw new Error(
      `missing platform package ${info.name}; reinstall with optional dependencies enabled`,
    );
  }
  if (
    metadata.name !== info.name ||
    metadata.version !== pkg.version ||
    JSON.stringify(metadata.os) !== JSON.stringify([info.os]) ||
    JSON.stringify(metadata.cpu) !== JSON.stringify([info.cpu])
  )
    throw new Error(`platform package mismatch for ${key}`);
  if (
    entry.path !== executableName(key) ||
    entry.platform !== key ||
    entry.version !== pkg.version ||
    !/^[a-f0-9]{64}$/.test(entry.sha256 ?? "")
  )
    throw new Error(`invalid artifact manifest for ${key}`);
  const file = resolve(platformRoot, entry.path);
  if (
    !lstatSync(file).isFile() ||
    !realpathSync(file).startsWith(`${realpathSync(platformRoot)}${sep}`)
  )
    throw new Error("artifact must be a regular packaged file");
  const bytes = readFileSync(file);
  if (bytes.length !== entry.size || sha256(bytes) !== entry.sha256)
    throw new Error(`checksum mismatch for ${key}`);
  return file;
}
