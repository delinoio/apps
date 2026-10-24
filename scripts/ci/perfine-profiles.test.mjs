// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { profileRuns, preserveResult } from "./perfine-profiles.mjs";

test("six languages measure both cases with a same-source repeat and candidate", () => {
  for (const language of ["rust", "c", "cpp", "zig", "go", "javascript"]) {
    for (const route of ["go", "javascript"].includes(language)
      ? ["default"]
      : ["callgrind", "gungraun"]) {
      const runs = profileRuns(language, route);
      assert.equal(runs.length, 6);
      for (const offset of [0, 3]) {
        assert.deepEqual(runs[offset].args, runs[offset + 1].args);
        assert.equal(
          runs[offset + 2].args[runs[offset + 2].args.indexOf("--variant") + 1],
          "candidate",
        );
        assert.ok(runs[offset].args.includes(offset ? "protected" : "bulk"));
      }
    }
  }
  assert.throws(() => profileRuns("unknown", "default"));
  assert.throws(() => profileRuns("javascript", "gungraun"));
});

test("fixture evidence preserves failed logs and profiles without build outputs or escaped files", (t) => {
  const root = mkdtempSync(join(tmpdir(), "profile-evidence-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const output = join(root, "run");
  mkdirSync(output);
  writeFileSync(
    join(output, "result.json"),
    JSON.stringify({ status: "failed", profile_artifacts: ["callgrind.out"] }),
  );
  writeFileSync(join(output, "callgrind.out"), "fixture-profile");
  writeFileSync(join(output, "command-01.stderr.txt"), "fixture-error");
  writeFileSync(join(output, "binary"), "do-not-upload");
  const destination = join(root, "evidence");
  assert.equal(preserveResult(output, destination).status, "failed");
  assert.equal(
    readFileSync(join(destination, "command-01.stderr.txt"), "utf8"),
    "fixture-error",
  );
  assert.throws(() => readFileSync(join(destination, "binary")));
  writeFileSync(join(root, "outside"), "outside");
  symlinkSync(join(root, "outside"), join(output, "escaped"));
  writeFileSync(
    join(output, "result.json"),
    JSON.stringify({ profile_artifacts: ["escaped"] }),
  );
  assert.throws(() => preserveResult(output, destination), /escapes/);
});
