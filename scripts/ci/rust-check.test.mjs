// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import {
  chmodSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import test from "node:test";

test("Rust execution scopes packages, preserves strict flags and rejects empty selection", (t) => {
  const cwd = mkdtempSync(join(tmpdir(), "rust-check-"));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  const output = join(cwd, "args.json");
  const cargo = join(cwd, "cargo");
  writeFileSync(
    cargo,
    `#!/usr/bin/env node\nrequire("node:fs").writeFileSync(process.env.CARGO_TEST_OUTPUT, JSON.stringify(process.argv.slice(2)));\n`,
  );
  chmodSync(cargo, 0o755);
  const run = (check, packages) =>
    spawnSync(
      process.execPath,
      [fileURLToPath(new URL("./rust-check.mjs", import.meta.url)), check],
      {
        encoding: "utf8",
        env: {
          ...process.env,
          PATH: `${cwd}:${process.env.PATH}`,
          CARGO_TEST_OUTPUT: output,
          RUST_PACKAGES: JSON.stringify(packages),
        },
      },
    );
  for (const check of ["test", "clippy"]) {
    const result = run(check, ["delidev-desktop", "delitools-shell"]);
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(JSON.parse(readFileSync(output, "utf8")), [
      check,
      "--locked",
      "-p",
      "delidev-desktop",
      "-p",
      "delitools-shell",
      "--all-targets",
      ...(check === "clippy" ? ["--all-features", "--", "-D", "warnings"] : []),
    ]);
  }
  rmSync(output);
  for (const [check, packages] of [
    ["test", []],
    ["test", ["--workspace"]],
    ["test", null],
    ["invalid", ["delidev-desktop"]],
  ]) {
    assert.notEqual(run(check, packages).status, 0);
  }
  assert.throws(() => readFileSync(output), { code: "ENOENT" });
});
