// SPDX-License-Identifier: Apache-2.0
import { createHash } from "node:crypto";
import { readFileSync, lstatSync, realpathSync } from "node:fs";
import { resolve, sep } from "node:path";
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
  const entry = manifest.artifacts?.[key];
  if (!entry) throw new Error(`missing artifact for ${key}`);
  const expected = `artifacts/lane-${key}/${key.startsWith("windows-") ? "lane.exe" : "lane"}`;
  if (
    entry.path !== expected ||
    entry.platform !== key ||
    entry.version !== pkg.version ||
    !/^[a-f0-9]{64}$/.test(entry.sha256)
  )
    throw new Error(`invalid artifact manifest for ${key}`);
  const file = resolve(root, entry.path);
  if (
    !lstatSync(file).isFile() ||
    !realpathSync(file).startsWith(`${realpathSync(root)}${sep}`)
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
