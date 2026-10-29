// SPDX-License-Identifier: Apache-2.0
import { createHash } from "node:crypto";
import {
  cpSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
  chmodSync,
  lstatSync,
  existsSync,
} from "node:fs";
import { execFileSync } from "node:child_process";
import { normalizeLauncherArchive } from "./archive.mjs";
import { dirname, resolve, join } from "node:path";
import { fileURLToPath } from "node:url";
export const root = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../../..",
);
export const sha256 = (bytes) =>
  createHash("sha256").update(bytes).digest("hex");
export function version(value) {
  if (!/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(value))
    throw new Error("Exact X.Y.Z version required");
  return value;
}
export function commit(value) {
  if (!/^[0-9a-f]{40}$/.test(value) || /^0+$/.test(value))
    throw new Error("Exact nonzero commit SHA required");
  return value;
}
import platforms from "../bin/platforms.cjs";
export const { targets, targetInfo, hostTarget } = platforms;
export function nativeBinary(file, target) {
  if (!lstatSync(file).isFile())
    throw new Error("Binary must be a regular file");
  return platforms.validateBinary(readFileSync(file), target);
}
export function linuxBinary(file) {
  return nativeBinary(file, "x86_64-unknown-linux-gnu");
}
export function archiveNames(releaseVersion, target) {
  version(releaseVersion);
  const info = targetInfo(target);
  return {
    entry: `perfine-${releaseVersion}.tgz`,
    platform: `${info.packageName.slice(1).replace("/", "-")}-${releaseVersion}.tgz`,
    standalone: `perfine-v${releaseVersion}-${info.suffix}.tar.gz`,
  };
}
// Run npm's JavaScript entry on Windows without a shell or .cmd argument parsing.
export function npm(args, options = {}) {
  if (process.platform !== "win32") return execFileSync("npm", args, options);
  const commands = execFileSync("where.exe", ["npm.cmd"], { encoding: "utf8" })
    .trim()
    .split(/\r?\n/);
  const cli = commands
    .map((command) => join(dirname(command), "node_modules/npm/bin/npm-cli.js"))
    .find(existsSync);
  if (!cli) throw new Error("Cannot locate npm CLI beside npm.cmd");
  return execFileSync(process.execPath, [cli, ...args], options);
}
export function stage({
  binary,
  notices,
  output,
  sourceCommit,
  releaseVersion,
  repositoryRoot = root,
  target = "x86_64-unknown-linux-gnu",
}) {
  version(releaseVersion);
  commit(sourceCommit);
  output = resolve(output);
  const info = targetInfo(target);
  const bytes = nativeBinary(binary, target);
  if (!readFileSync(notices, "utf8").trim())
    throw new Error("Third-party license texts required");
  if (existsSync(output))
    throw new Error(
      "Output must not exist; preserve earlier release candidates",
    );
  const pkg = JSON.parse(
    readFileSync(join(repositoryRoot, "packages/perfine/package.json")),
  );
  if (pkg.version !== releaseVersion)
    throw new Error("Package version mismatch");
  const provenance = {
    schemaVersion: 1,
    version: releaseVersion,
    sourceRepository: "delinoio/apps",
    sourceCommit,
    target,
    binarySha256: sha256(bytes),
  };
  for (const kind of [
    "platform",
    "standalone",
    ...(info.os === "linux" ? ["action"] : []),
  ]) {
    const dest = join(output, kind);
    mkdirSync(join(dest, "runtime"), { recursive: true });
    writeFileSync(join(dest, "runtime", info.executable), bytes, {
      mode: 0o755,
    });
    writeFileSync(
      join(dest, "runtime/SHA256SUMS"),
      `${sha256(bytes)}  ${info.executable}\n`,
    );
    for (const name of ["LICENSE", "NOTICE"])
      cpSync(join(repositoryRoot, name), join(dest, name));
    cpSync(notices, join(dest, "THIRD_PARTY_NOTICES"));
    writeFileSync(
      join(dest, "provenance.json"),
      `${JSON.stringify(provenance, null, 2)}\n`,
    );
  }
  const expectedDependencies = Object.fromEntries(
    Object.keys(targets).map((key) => [
      targetInfo(key).packageName,
      releaseVersion,
    ]),
  );
  if (
    JSON.stringify(pkg.optionalDependencies) !==
    JSON.stringify(expectedDependencies)
  )
    throw new Error("Exact matching platform optionalDependencies required");
  mkdirSync(join(output, "npm"));
  for (const name of ["LICENSE", "NOTICE"])
    cpSync(join(repositoryRoot, name), join(output, "npm", name));
  writeFileSync(
    join(output, "npm/THIRD_PARTY_NOTICES"),
    "The launcher uses Node.js built-ins only. Native dependency licenses are included in the selected platform package.\n",
  );
  writeFileSync(
    join(output, "npm/provenance.json"),
    `${JSON.stringify({ schemaVersion: 1, version: releaseVersion, sourceRepository: "delinoio/apps", sourceCommit }, null, 2)}\n`,
  );
  writeFileSync(
    join(output, "platform/package.json"),
    `${JSON.stringify({ name: info.packageName, version: releaseVersion, description: `Perfine native executable for ${info.os}/${info.cpu}`, license: "Apache-2.0", os: [info.os], cpu: [info.cpu], ...(info.os === "linux" ? { libc: ["glibc"] } : {}), files: ["runtime", "LICENSE", "NOTICE", "THIRD_PARTY_NOTICES", "provenance.json"], repository: pkg.repository }, null, 2)}\n`,
  );
  // Publish only the staged manifest. The source package has no install-time network hooks.
  delete pkg.scripts;
  writeFileSync(
    join(output, "npm/package.json"),
    `${JSON.stringify(pkg, null, 2)}\n`,
  );
  cpSync(
    join(repositoryRoot, "packages/perfine/bin"),
    join(output, "npm/bin"),
    { recursive: true },
  );
  chmodSync(join(output, "npm/bin/perfine.cjs"), 0o755);
  cpSync(
    join(repositoryRoot, "packages/perfine/README.md"),
    join(output, "npm/README.md"),
  );
  if (info.os === "linux")
    for (const name of [
      "action.cjs",
      "action.yml",
      "README.md",
      "ACTION_NOTICES",
    ])
      cpSync(
        join(repositoryRoot, "actions/perfine", name),
        join(output, "action", name),
      );
  const archives = archiveNames(releaseVersion, target);
  execFileSync(
    "tar",
    [
      "-czf",
      // Relative archive paths keep GNU tar from treating a Windows drive as a remote host.
      archives.standalone,
      "-C",
      "standalone",
      ".",
    ],
    { cwd: output },
  );
  for (const kind of ["npm", "platform"])
    npm(["pack", "--ignore-scripts", "--pack-destination", output], {
      cwd: join(output, kind),
      stdio: "pipe",
    });
  const entryArchive = join(output, archives.entry);
  writeFileSync(
    entryArchive,
    normalizeLauncherArchive(readFileSync(entryArchive)),
  );
  const files = Object.values(archives);
  writeFileSync(
    join(output, "SHA256SUMS"),
    files
      .map((name) => `${sha256(readFileSync(join(output, name)))}  ${name}\n`)
      .join(""),
  );
  return provenance;
}
if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  const target = process.env.PERFINE_TARGET ?? "x86_64-unknown-linux-gnu";
  const info = targetInfo(target);
  const binary =
    process.env.PERFINE_BINARY ??
    join(
      root,
      "target",
      ...(process.env.PERFINE_TARGET ? [target] : []),
      "release",
      info.executable,
    );
  nativeBinary(binary, target);
  const notices =
    process.env.PERFINE_NOTICES ??
    join(root, "packages/perfine/dist/THIRD_PARTY_NOTICES");
  if (process.env.PERFINE_NOTICES === undefined) {
    const { collectNotices } = await import("./notices.mjs");
    mkdirSync(dirname(notices), { recursive: true });
    writeFileSync(notices, collectNotices(root, target));
  }
  console.log(
    JSON.stringify(
      stage({
        target,
        binary,
        notices,
        output:
          process.env.PERFINE_OUTPUT ??
          join(root, "packages/perfine/dist/release"),
        sourceCommit: execFileSync("git", ["rev-parse", "HEAD"], {
          cwd: root,
          encoding: "utf8",
        }).trim(),
        releaseVersion: JSON.parse(
          readFileSync(join(root, "packages/perfine/package.json")),
        ).version,
      }),
    ),
  );
}
