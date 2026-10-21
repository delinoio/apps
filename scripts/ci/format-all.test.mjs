import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

test("CI all-file formatting checks unchanged files without a comparison ref", () => {
  const cwd = mkdtempSync(join(tmpdir(), "ci-format-all-"));
  const run = (command, args, env = {}) =>
    spawnSync(command, args, {
      cwd,
      encoding: "utf8",
      env: { ...process.env, TURBO_SCM_BASE: "", TURBO_SCM_HEAD: "", ...env },
    });
  try {
    for (const args of [
      ["init", "--initial-branch=fixture"],
      ["config", "user.name", "Fixture"],
      ["config", "user.email", "fixture@example.invalid"],
    ])
      assert.equal(run("git", args).status, 0);
    writeFileSync(join(cwd, "example.js"), "const answer=42;\n");
    assert.equal(run("git", ["add", "example.js"]).status, 0);
    assert.equal(run("git", ["commit", "-m", "fixture"]).status, 0);
    const script = fileURLToPath(
      new URL("./format-changed.mjs", import.meta.url),
    );
    const result = run(process.execPath, [script, "--all"], { CI: "true" });
    assert.equal(result.status, 1, result.stdout + result.stderr);
    const record = JSON.parse(
      result.stdout
        .split("\n")
        .find((line) => line.startsWith('{"event":"ci_format",')),
    );
    assert.equal(record.base, null);
    assert.equal(record.head, null);
    assert.equal(record.candidates, 1);
    assert.equal(record.local, false);
    assert.equal(run("git", ["status", "--porcelain"]).stdout, "");
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});
