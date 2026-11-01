// SPDX-License-Identifier: Apache-2.0
// Shared packaging steps for local builds and release assembly: the platform
// npm packages, the deterministic direct CLI archives, the launcher package and
// the checksum list.
import { createHash } from "node:crypto";
import {
  cpSync,
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { executableName, platformPackage, platforms } from "../bin/verify.mjs";
import { writeArchive } from "./archive.mjs";
import { npm } from "./npm.mjs";

export const packageRoot = fileURLToPath(new URL("../", import.meta.url));
export const repositoryRoot = resolve(packageRoot, "../..");
export const noticeFiles = Object.freeze([
  "LICENSE",
  "NOTICE",
  "THIRD_PARTY_NOTICES",
]);

export const archiveName = (key, version) => `anydash-${key}-${version}.tar.gz`;
export const platformArchiveName = (key, version) =>
  `delinoio-anydash-${key}-${version}.tgz`;
export const launcherArchiveName = (version) =>
  `delinoio-anydash-${version}.tgz`;

export const sha256 = (bytes) =>
  createHash("sha256").update(bytes).digest("hex");
export const sha256File = (path) => sha256(readFileSync(path));
const writeJson = (path, value) =>
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`);

export function readPackage(root = packageRoot) {
  const pkg = JSON.parse(readFileSync(resolve(root, "package.json"), "utf8"));
  if (!/^\d+\.\d+\.\d+$/.test(pkg.version))
    throw new Error("invalid release version");
  const expected = Object.fromEntries(
    platforms.map((key) => [platformPackage(key).name, pkg.version]),
  );
  if (JSON.stringify(pkg.optionalDependencies) !== JSON.stringify(expected))
    throw new Error("Exact matching platform optionalDependencies required");
  return pkg;
}

/**
 * Lays out one platform artifact directory: the executable, its manifest and
 * checksum, the repository license and the third-party notices.
 */
export function stageArtifact({
  key,
  version,
  revision,
  executable,
  notices,
  directory,
}) {
  const name = executableName(key);
  rmSync(directory, { recursive: true, force: true });
  mkdirSync(directory, { recursive: true });
  cpSync(executable, resolve(directory, name));
  const bytes = readFileSync(resolve(directory, name));
  const entry = {
    platform: key,
    version,
    path: name,
    size: bytes.length,
    sha256: sha256(bytes),
  };
  for (const notice of ["LICENSE", "NOTICE"])
    cpSync(resolve(repositoryRoot, notice), resolve(directory, notice));
  cpSync(notices, resolve(directory, "THIRD_PARTY_NOTICES"), {
    recursive: true,
  });
  writeJson(resolve(directory, "manifest.json"), {
    ...entry,
    sourceRevision: revision,
  });
  writeFileSync(resolve(directory, "SHA256SUMS"), `${entry.sha256}  ${name}\n`);
  return entry;
}

/** Writes the direct CLI archive and the platform npm package. */
export function packArtifact({ key, version, directory, destination, stage }) {
  const name = executableName(key);
  writeArchive(
    directory,
    `anydash-${key}`,
    resolve(destination, archiveName(key, version)),
    [name],
  );
  const pkg = readPackage();
  rmSync(stage, { recursive: true, force: true });
  cpSync(directory, stage, { recursive: true });
  const info = platformPackage(key);
  writeJson(resolve(stage, "package.json"), {
    name: info.name,
    version,
    description: `AnyDash CLI executable for ${info.os}/${info.cpu}`,
    license: pkg.license,
    os: [info.os],
    cpu: [info.cpu],
    files: [name, "manifest.json", "SHA256SUMS", ...noticeFiles],
    repository: pkg.repository,
  });
  npm(
    ["pack", "--ignore-scripts", "--pack-destination", destination, "--quiet"],
    {
      cwd: stage,
      stdio: ["ignore", "ignore", "inherit"],
    },
  );
  const archive = resolve(destination, platformArchiveName(key, version));
  if (!existsSync(archive))
    throw new Error(`npm pack did not create ${archive}`);
  return [archiveName(key, version), platformArchiveName(key, version)];
}

/** Packs the launcher, which carries no executables, with the combined manifest. */
export function packLauncher({ manifest, destination, stage }) {
  const pkg = readPackage();
  if (manifest.version !== pkg.version)
    throw new Error("Launcher and artifact versions must match");
  rmSync(stage, { recursive: true, force: true });
  mkdirSync(stage, { recursive: true });
  for (const name of ["bin", "README.md"])
    cpSync(resolve(packageRoot, name), resolve(stage, name), {
      recursive: true,
    });
  for (const name of ["LICENSE", "NOTICE"])
    cpSync(resolve(repositoryRoot, name), resolve(stage, name));
  const { scripts, ...published } = pkg;
  void scripts;
  writeJson(resolve(stage, "package.json"), published);
  writeJson(resolve(stage, "manifest.json"), manifest);
  npm(
    ["pack", "--ignore-scripts", "--pack-destination", destination, "--quiet"],
    {
      cwd: stage,
      stdio: ["ignore", "ignore", "inherit"],
    },
  );
  return launcherArchiveName(pkg.version);
}

export function writeChecksums(directory, files) {
  writeFileSync(
    resolve(directory, "SHA256SUMS"),
    [...files]
      .sort()
      .map((name) => `${sha256File(resolve(directory, name))}  ${name}\n`)
      .join(""),
  );
}

/** Parses a SHA256SUMS file and checks every listed file in `directory`. */
export function verifyChecksums(directory) {
  const result = new Map();
  for (const line of readFileSync(resolve(directory, "SHA256SUMS"), "utf8")
    .trim()
    .split("\n")) {
    const match = /^([a-f0-9]{64})  ([A-Za-z0-9._@-]+)$/.exec(line);
    if (!match) throw new Error("invalid checksum entry");
    if (result.has(match[2]))
      throw new Error(`duplicate checksum: ${match[2]}`);
    if (sha256File(resolve(directory, match[2])) !== match[1])
      throw new Error(`checksum mismatch: ${match[2]}`);
    result.set(match[2], match[1]);
  }
  return result;
}
