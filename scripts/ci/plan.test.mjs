// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  mkdtempSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { main, plan } from "./plan.mjs";

function fixture(t) {
  const cwd = mkdtempSync(join(tmpdir(), "ci-plan-"));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  const git = (...args) =>
    execFileSync("git", args, { cwd, encoding: "utf8" }).trim();
  git("init", "--quiet");
  git("config", "user.name", "CI fixture");
  git("config", "user.email", "fixture@example.invalid");
  git("config", "core.hooksPath", ".no-hooks");
  const write = (path, content = "fixture\n") => {
    mkdirSync(dirname(join(cwd, path)), { recursive: true });
    writeFileSync(join(cwd, path), content);
  };
  const commit = () => {
    git("add", ".");
    git("commit", "--quiet", "--allow-empty", "-m", "fixture");
    return git("rev-parse", "HEAD");
  };
  write("LICENSE");
  const base = commit();
  return { cwd, git, write, commit, base };
}

test("planner skips empty comparisons and non-validation paths", (t) => {
  const f = fixture(t);
  assert.deepEqual(plan(f.cwd, f.base, f.base), {
    quality: false,
    format: false,
    fixtures: false,
  });
  f.write("assets/image.svg");
  assert.deepEqual(plan(f.cwd, f.base, f.commit()), {
    quality: false,
    format: false,
    fixtures: false,
  });
});

test("documentation changes select formatting without fixtures", (t) => {
  const f = fixture(t);
  f.write("docs/space name\n한국어.md");
  assert.deepEqual(plan(f.cwd, f.base, f.commit()), {
    quality: true,
    format: true,
    fixtures: false,
  });
});

test("every shared fixture input selects tests", async (t) => {
  for (const path of [
    ".github/workflows/CI.yml",
    ".nvmrc",
    "package.json",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
    "rstack.config.ts",
    "lefthook.yml",
    "scripts/install-git-hooks.sh",
    "scripts/ci/plan.mjs",
    "scripts/ci/format-changed.test.mjs",
  ]) {
    await t.test(path, (t) => {
      const f = fixture(t);
      f.write(path);
      const result = plan(f.cwd, f.base, f.commit());
      assert.equal(result.quality, true);
      assert.equal(result.fixtures, true);
    });
  }
});

test("deleted inputs and renamed paths still select affected fixtures", (t) => {
  const f = fixture(t);
  f.write("scripts/ci/old.mjs");
  f.write("old.txt");
  const base = f.commit();
  f.git("rm", "scripts/ci/old.mjs");
  f.git("mv", "old.txt", "new.md");
  assert.deepEqual(plan(f.cwd, base, f.commit()), {
    quality: true,
    format: true,
    fixtures: true,
  });
});

test("deleted documentation has no remaining formatting input", (t) => {
  const f = fixture(t);
  f.write("docs/old.md");
  const base = f.commit();
  f.git("rm", "docs/old.md");
  assert.deepEqual(plan(f.cwd, base, f.commit()), {
    quality: false,
    format: false,
    fixtures: false,
  });
});

test("planner rejects invalid comparisons instead of emitting a skip plan", (t) => {
  const f = fixture(t);
  for (const base of [undefined, "main", "0".repeat(40), "a".repeat(40)]) {
    assert.throws(() => plan(f.cwd, base, f.base));
  }
});

test("planner publishes booleans and records exact comparison and checkout", (t) => {
  const f = fixture(t);
  f.write("docs/guide.md");
  const head = f.commit();
  const output = join(f.cwd, "output");
  const summary = join(f.cwd, "summary");
  main(
    {
      CI_BASE: f.base,
      CI_HEAD: head,
      GITHUB_OUTPUT: output,
      GITHUB_STEP_SUMMARY: summary,
    },
    f.cwd,
  );
  assert.equal(
    readFileSync(output, "utf8"),
    "quality=true\nformat=true\nfixtures=false\n",
  );
  const record = JSON.parse(
    readFileSync(summary, "utf8").slice("CI plan: ".length),
  );
  assert.equal(record.base, f.base);
  assert.equal(record.head, head);
  assert.equal(record.validationRevision, head);
});
