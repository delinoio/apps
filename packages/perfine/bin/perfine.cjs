#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
const { createHash } = require("node:crypto");
const { readFileSync, lstatSync } = require("node:fs");
const { join, dirname } = require("node:path");
const { createRequire } = require("node:module");
const { spawn } = require("node:child_process");
const { constants } = require("node:os");
const { hostTarget, validateBinary } = require("./platforms.cjs");
const recovery =
  "Reinstall perfine with optional dependencies enabled and the matching platform package available. No runtime download or source build is attempted.";
function verifiedBinary(root, os = process.platform, cpu = process.arch) {
  const info = hostTarget(os, cpu);
  try {
    const entry = JSON.parse(readFileSync(join(root, "package.json")));
    const platformRoot = dirname(
      createRequire(join(root, "package.json")).resolve(
        `${info.packageName}/package.json`,
      ),
    );
    const metadata = JSON.parse(
      readFileSync(join(platformRoot, "package.json")),
    );
    if (
      entry.optionalDependencies?.[info.packageName] !== entry.version ||
      metadata.name !== info.packageName ||
      metadata.version !== entry.version ||
      JSON.stringify(metadata.os) !== JSON.stringify([os]) ||
      JSON.stringify(metadata.cpu) !== JSON.stringify([cpu])
    )
      throw new Error(
        "Platform package name, version or architecture mismatch",
      );
    const binary = join(platformRoot, "runtime", info.executable);
    if (!lstatSync(binary).isFile())
      throw new Error("Packaged binary must be a regular file");
    const bytes = readFileSync(binary);
    const actual = createHash("sha256").update(bytes).digest("hex");
    if (
      readFileSync(join(platformRoot, "runtime/SHA256SUMS"), "utf8") !==
      `${actual}  ${info.executable}\n`
    )
      throw new Error("Perfine binary checksum mismatch");
    const provenance = JSON.parse(
      readFileSync(join(platformRoot, "provenance.json")),
    );
    const entryProvenance = JSON.parse(
      readFileSync(join(root, "provenance.json")),
    );
    if (
      entryProvenance.schemaVersion !== 1 ||
      entryProvenance.version !== entry.version ||
      entryProvenance.sourceRepository !== "delinoio/apps" ||
      entryProvenance.sourceCommit !== provenance.sourceCommit
    )
      throw new Error("Entry and platform provenance mismatch");
    if (
      provenance.schemaVersion !== 1 ||
      provenance.target !== info.target ||
      provenance.version !== entry.version ||
      provenance.sourceRepository !== "delinoio/apps" ||
      !/^[a-f0-9]{40}$/.test(provenance.sourceCommit) ||
      /^0+$/.test(provenance.sourceCommit) ||
      provenance.binarySha256 !== actual
    )
      throw new Error("Platform provenance mismatch");
    validateBinary(bytes, info.target);
    return binary;
  } catch (error) {
    throw new Error(
      `Cannot use ${info.packageName}: ${error.message}. ${recovery}`,
    );
  }
}
function launch(binary, args, { spawnProcess = spawn, parent = process } = {}) {
  const child = spawnProcess(binary, args, { stdio: "inherit", shell: false });
  const handlers = new Map(
    ["SIGINT", "SIGTERM"].map((signal) => [signal, () => child.kill(signal)]),
  );
  for (const [signal, handler] of handlers) parent.on(signal, handler);
  const cleanup = () => {
    for (const [signal, handler] of handlers)
      parent.removeListener(signal, handler);
  };
  child.once("error", (error) => {
    cleanup();
    console.error(
      `Cannot start packaged Perfine binary: ${error.message}. ${recovery}`,
    );
    parent.exitCode = 1;
  });
  child.once("exit", (code, signal) => {
    cleanup();
    parent.exitCode = code ?? 128 + (constants.signals[signal] ?? 1);
  });
  return child;
}
module.exports = { verifiedBinary, launch };
if (require.main === module) {
  try {
    launch(verifiedBinary(join(__dirname, "..")), process.argv.slice(2));
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
