// SPDX-License-Identifier: Apache-2.0
import { createHash } from "node:crypto";
import {
  cpSync,
  existsSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
  chmodSync,
} from "node:fs";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { build } from "esbuild";
import { desktopTarCommand } from "./desktop-archive.mjs";
// Keep the relay's independently bundled runtime on the repository Node 24 line.
// The October 2026 security release supersedes the development machine runtime.
// See https://nodejs.org/en/blog/vulnerability/october-2026-security-releases.
// Checksums establish artifact integrity, not absence of vulnerabilities.
const version = "v24.22.1";
export async function packageChromeRelay({
  root,
  output,
  resources,
  binaryRoot,
}) {
  const platform = process.platform,
    arch = process.arch;
  if (!["arm64", "x64"].includes(arch))
    throw new Error("The Chrome relay requires an arm64 or x64 Node runtime.");
  const cache = join(output, "relay-runtime"),
    destination = join(resources, "relay");
  mkdirSync(cache, { recursive: true });
  mkdirSync(destination, { recursive: true });
  const windows = platform === "win32";
  const acquire = async (runtimeVersion, executableName, licenseName) => {
    const name = `node-${runtimeVersion}-${windows ? "win" : platform}-${arch}`;
    const archive = `${name}.${windows ? "zip" : "tar.gz"}`;
    const base = `https://nodejs.org/dist/${runtimeVersion}/`;
    const response = await fetch(`${base}SHASUMS256.txt`);
    if (!response.ok)
      throw new Error("Could not obtain the Node relay checksum.");
    const sha256 = (await response.text())
      .split("\n")
      .find((line) => line.trim().endsWith(` ${archive}`))
      ?.split(/\s+/)[0];
    if (!sha256 || !/^[a-f0-9]{64}$/.test(sha256))
      throw new Error("The Node relay checksum is missing.");
    const archivePath = join(cache, archive);
    let bytes = existsSync(archivePath) ? readFileSync(archivePath) : undefined;
    if (!bytes || createHash("sha256").update(bytes).digest("hex") !== sha256) {
      const download = await fetch(`${base}${archive}`);
      if (!download.ok)
        throw new Error("Could not download the Node relay runtime.");
      bytes = Buffer.from(await download.arrayBuffer());
    }
    if (createHash("sha256").update(bytes).digest("hex") !== sha256)
      throw new Error("The Node relay runtime checksum does not match.");
    writeFileSync(archivePath, bytes);
    const extracted = spawnSync(
      desktopTarCommand(),
      ["-xf", archivePath, "-C", cache],
      {
        stdio: "inherit",
        shell: false,
      },
    );
    if (extracted.status !== 0)
      throw new Error("Could not extract the Node relay runtime.");
    cpSync(
      join(cache, name, windows ? "node.exe" : "bin/node"),
      join(destination, executableName),
    );
    cpSync(join(cache, name, "LICENSE"), join(destination, licenseName));
    if (!windows) chmodSync(join(destination, executableName), 0o755);
    return { version: runtimeVersion, archive, sha256 };
  };
  const runtime = await acquire(
    version,
    windows ? "node.exe" : "node",
    "NODE-LICENSE",
  );
  // Node 24's official macOS binary requires 13.5, while Lynxtron supports 12.
  // Keep the patched Node 22 compatibility runtime until Lane drops macOS 12.
  if (!windows)
    runtime.compatibilityRuntime = await acquire(
      "v22.23.4",
      "node-macos12",
      "NODE-MACOS12-LICENSE",
    );
  cpSync(
    join(binaryRoot, windows ? "lane-chrome-host.exe" : "lane-chrome-host"),
    join(destination, windows ? "lane-chrome-host.exe" : "lane-chrome-host"),
  );
  if (!windows) {
    chmodSync(join(destination, "node"), 0o755);
    chmodSync(join(destination, "lane-chrome-host"), 0o755);
  }
  await build({
    absWorkingDir: root,
    entryPoints: ["native/desktop/chrome-native-host.mjs"],
    outfile: join(destination, "chrome-native-host.mjs"),
    bundle: true,
    platform: "node",
    format: "esm",
    target: "node22",
  });
  writeFileSync(join(destination, "runtime.json"), JSON.stringify(runtime));
}
