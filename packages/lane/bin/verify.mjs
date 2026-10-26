// SPDX-License-Identifier: Apache-2.0
import { createHash } from "node:crypto";
import { readFileSync, lstatSync, realpathSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, resolve, sep } from "node:path";
export const platforms = Object.freeze([
  "darwin-amd64",
  "darwin-arm64",
  "linux-amd64",
  "linux-arm64",
  "windows-amd64",
  "windows-arm64",
]);
export function platformKey(os = process.platform, arch = process.arch) {
  const key = `${os === "win32" ? "windows" : os}-${arch === "x64" ? "amd64" : arch}`;
  if (!platforms.includes(key))
    throw new Error(`unsupported platform ${os}/${arch}`);
  return key;
}
export function platformPackage(key) {
  if (!platforms.includes(key)) throw new Error(`unsupported platform ${key}`);
  const [os, arch] = key.split("-");
  return {
    name: `@delinoio/lane-${key}`,
    os: os === "windows" ? "win32" : os,
    cpu: arch === "amd64" ? "x64" : arch,
  };
}
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
  if (
    !/^[a-f0-9]{40}$/.test(manifest.sourceRevision ?? "") ||
    !/^[a-f0-9]{40}$/.test(manifest.launcherRevision ?? "")
  )
    throw new Error("invalid source revision manifest");
  const entry = manifest.artifacts?.[key];
  if (!entry) throw new Error(`missing artifact for ${key}`);
  const info = platformPackage(key);
  if (pkg.optionalDependencies?.[info.name] !== pkg.version)
    throw new Error(`invalid optional dependency for ${key}`);
  let platformRoot;
  try {
    platformRoot = dirname(
      createRequire(resolve(root, "package.json")).resolve(
        `${info.name}/package.json`,
      ),
    );
  } catch {
    throw new Error(
      `missing platform package ${info.name}; reinstall with optional dependencies enabled`,
    );
  }
  const metadata = JSON.parse(
    readFileSync(resolve(platformRoot, "package.json"), "utf8"),
  );
  if (
    metadata.name !== info.name ||
    metadata.version !== pkg.version ||
    JSON.stringify(metadata.os) !== JSON.stringify([info.os]) ||
    JSON.stringify(metadata.cpu) !== JSON.stringify([info.cpu])
  )
    throw new Error(`platform package mismatch for ${key}`);
  const expected = key.startsWith("windows-") ? "lane.exe" : "lane";
  if (
    entry.path !== expected ||
    entry.platform !== key ||
    entry.version !== pkg.version ||
    !/^[a-f0-9]{64}$/.test(entry.sha256)
  )
    throw new Error(`invalid artifact manifest for ${key}`);
  const file = resolve(platformRoot, entry.path);
  if (
    !lstatSync(file).isFile() ||
    !realpathSync(file).startsWith(`${realpathSync(platformRoot)}${sep}`)
  )
    throw new Error("artifact must be a regular packaged file");
  const bytes = readFileSync(file);
  if (
    bytes.length !== entry.size ||
    createHash("sha256").update(bytes).digest("hex") !== entry.sha256
  )
    throw new Error(`checksum mismatch for ${key}`);
  return file;
}
