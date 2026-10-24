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
export function linuxBinary(file) {
  if (!lstatSync(file).isFile())
    throw new Error("Binary must be a regular file");
  const bytes = readFileSync(file);
  if (
    bytes.length < 64 ||
    bytes.subarray(0, 4).toString("hex") !== "7f454c46" ||
    bytes[4] !== 2 ||
    bytes[5] !== 1 ||
    bytes.readUInt16LE(18) !== 62
  )
    throw new Error("Linux x86_64 ELF binary required");
  return bytes;
}
export function stage({
  binary,
  notices,
  output,
  sourceCommit,
  releaseVersion,
  repositoryRoot = root,
}) {
  version(releaseVersion);
  commit(sourceCommit);
  const bytes = linuxBinary(binary);
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
    target: "x86_64-unknown-linux-gnu",
    binarySha256: sha256(bytes),
  };
  for (const kind of ["npm", "standalone", "action"]) {
    const dest = join(output, kind);
    mkdirSync(join(dest, "runtime"), { recursive: true });
    writeFileSync(join(dest, "runtime/perfine"), bytes, { mode: 0o755 });
    writeFileSync(
      join(dest, "runtime/SHA256SUMS"),
      `${sha256(bytes)}  perfine\n`,
    );
    for (const name of ["LICENSE", "NOTICE"])
      cpSync(join(repositoryRoot, name), join(dest, name));
    cpSync(notices, join(dest, "THIRD_PARTY_NOTICES"));
    writeFileSync(
      join(dest, "provenance.json"),
      `${JSON.stringify(provenance, null, 2)}\n`,
    );
  }
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
  const archive = `perfine-v${releaseVersion}-linux-x86_64.tar.gz`;
  execFileSync("tar", [
    "-czf",
    join(output, archive),
    "-C",
    join(output, "standalone"),
    ".",
  ]);
  execFileSync(
    "npm",
    ["pack", "--ignore-scripts", "--pack-destination", output],
    { cwd: join(output, "npm"), stdio: "pipe" },
  );
  const files = [archive, `perfine-${releaseVersion}.tgz`];
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
  const binary =
    process.env.PERFINE_BINARY ?? join(root, "target/release/perfine");
  linuxBinary(binary);
  const notices =
    process.env.PERFINE_NOTICES ??
    join(root, "packages/perfine/dist/THIRD_PARTY_NOTICES");
  if (!existsSync(notices)) {
    const { collectNotices } = await import("./notices.mjs");
    mkdirSync(dirname(notices), { recursive: true });
    writeFileSync(notices, collectNotices(root));
  }
  console.log(
    JSON.stringify(
      stage({
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
