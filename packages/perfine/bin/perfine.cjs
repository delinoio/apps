#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
const { createHash } = require("node:crypto");
const { readFileSync, lstatSync } = require("node:fs");
const { join } = require("node:path");
const { spawn } = require("node:child_process");
function verifiedBinary(root) {
  const binary = join(root, "runtime", "perfine");
  if (!lstatSync(binary).isFile())
    throw new Error("Packaged binary must be a regular file");
  const expected = readFileSync(join(root, "runtime", "SHA256SUMS"), "utf8");
  const actual = createHash("sha256")
    .update(readFileSync(binary))
    .digest("hex");
  if (expected !== `${actual}  perfine\n`)
    throw new Error("Perfine binary checksum mismatch");
  return binary;
}
module.exports = { verifiedBinary };
if (require.main === module) {
  try {
    if (process.platform !== "linux" || process.arch !== "x64")
      throw new Error("Perfine supports Linux x86_64 only");
    const child = spawn(
      verifiedBinary(join(__dirname, "..")),
      process.argv.slice(2),
      { stdio: "inherit", shell: false },
    );
    for (const signal of ["SIGINT", "SIGTERM"])
      process.on(signal, () => child.kill(signal));
    child.on("error", () => {
      console.error("Cannot start packaged Perfine binary");
      process.exitCode = 1;
    });
    child.on("exit", (code, signal) => {
      process.exitCode = code ?? (signal === "SIGINT" ? 130 : 143);
    });
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
