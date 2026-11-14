// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import {
  mkdtempSync,
  mkdirSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { main, plan, workspaceRoots } from "./plan.mjs";

const root = fileURLToPath(new URL("../..", import.meta.url));

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
    go: false,
    e2e: false,
  });
  f.write("assets/image.svg");
  assert.deepEqual(plan(f.cwd, f.base, f.commit()), {
    quality: false,
    format: false,
    fixtures: false,
    go: false,
    e2e: false,
  });
});

test("documentation changes select formatting without fixtures", (t) => {
  const f = fixture(t);
  f.write("docs/space name\n한국어.md");
  assert.deepEqual(plan(f.cwd, f.base, f.commit()), {
    quality: true,
    format: true,
    fixtures: false,
    go: false,
    e2e: false,
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
    "turbo.json",
    "apps/delino-accounts/turbo.json",
    "servers/accounts-api/turbo.json",
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

test("only turbo.json files select Turbo configuration fixtures", (t) => {
  const f = fixture(t);
  f.write("apps/example/not-turbo.json");
  f.write("apps/example/turbo.json.example");
  f.write("apps/example/turbo.json/readme.txt");
  const result = plan(f.cwd, f.base, f.commit());
  assert.equal(result.fixtures, false);
});

test("workspace package manifests select fixtures", async (t) => {
  for (const path of [
    "apps/delino-accounts/package.json",
    "apps/delino-docs/package.json",
    "apps/delino-site/package.json",
    "servers/accounts-api/package.json",
    "packages/new-lib/package.json",
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

test("deleted and renamed workspace manifests select fixtures", (t) => {
  const f = fixture(t);
  f.write("packages/old-lib/package.json");
  f.write("apps/old-app/package.json");
  const base = f.commit();
  f.git("rm", "--quiet", "packages/old-lib/package.json");
  // --no-renames reports a moved manifest as a deletion plus an addition.
  f.git("mv", "apps/old-app/package.json", "docs-only.txt");
  const result = plan(f.cwd, base, f.commit());
  assert.equal(result.quality, true);
  assert.equal(result.fixtures, true);

  const g = fixture(t);
  g.write("apps/old-app/package.json");
  const renameBase = g.commit();
  g.write("apps/new-app/readme.txt", "kept\n");
  g.git("mv", "apps/old-app/package.json", "apps/new-app/package.json");
  assert.equal(plan(g.cwd, renameBase, g.commit()).fixtures, true);
});

test("only direct workspace package manifests select fixtures", (t) => {
  const f = fixture(t);
  f.write("apps/delino-accounts/src/package.json");
  f.write("apps/delino-accounts/src/index.css");
  f.write("apps/package.json");
  f.write("apps/delino-accounts/package.json.example");
  f.write("cmds/example/package.json");
  f.write("docs/package.json");
  const result = plan(f.cwd, f.base, f.commit());
  assert.equal(result.fixtures, false);
});

test("workspace manifest roots match pnpm-workspace.yaml", () => {
  const globs = readFileSync(join(root, "pnpm-workspace.yaml"), "utf8")
    .split("\n")
    .map((line) => line.match(/^\s+-\s+(\S+?)\/\*\s*$/)?.[1])
    .filter(Boolean);
  assert.deepEqual([...workspaceRoots].sort(), globs.sort());
});

test("Go sources and module files select Go checks", async (t) => {
  for (const path of [
    "servers/example/main.go",
    "servers/example/go.mod",
    "cmds/example/go.sum",
    "go.mod",
  ]) {
    await t.test(path, (t) => {
      const f = fixture(t);
      f.write(path);
      assert.equal(plan(f.cwd, f.base, f.commit()).go, true);
    });
  }
  const f = fixture(t);
  f.write("servers/example/README.md");
  f.write("servers/example/go.mod.example");
  assert.equal(plan(f.cwd, f.base, f.commit()).go, false);
});

test("app paths and shared workspace inputs select e2e suites", async (t) => {
  for (const path of [
    "apps/delino-accounts/src/App.tsx",
    "apps/delino-docs/docs/guide/introduction.md",
    "apps/delino-site/e2e.config.ts",
    ".github/workflows/CI.yml",
    ".nvmrc",
    "package.json",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
    "turbo.json",
  ]) {
    await t.test(path, (t) => {
      const f = fixture(t);
      f.write(path);
      assert.equal(plan(f.cwd, f.base, f.commit()).e2e, true);
    });
  }
  const f = fixture(t);
  f.write("apps/AGENTS.md");
  f.write("docs/apps-delino-site-foundation.md");
  f.write("servers/accounts-api/main.go");
  assert.equal(plan(f.cwd, f.base, f.commit()).e2e, false);
});

test("deleted Go files still select Go checks", (t) => {
  const f = fixture(t);
  f.write("servers/example/old.go");
  const base = f.commit();
  f.git("rm", "--quiet", "servers/example/old.go");
  assert.equal(plan(f.cwd, base, f.commit()).go, true);
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
    go: false,
    e2e: false,
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
    go: false,
    e2e: false,
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
    "quality=true\nformat=true\nfixtures=false\ngo=false\ne2e=false\n",
  );
  const record = JSON.parse(
    readFileSync(summary, "utf8").slice("CI plan: ".length),
  );
  assert.equal(record.base, f.base);
  assert.equal(record.head, head);
  assert.equal(record.validationRevision, head);
});

test("planner accepts more than 1 MiB of changed paths", (t) => {
  const f = fixture(t);
  // Build the head tree directly in the object database so that no files are
  // written to disk. The .txt names keep the formatter selection empty.
  const blob = execFileSync("git", ["hash-object", "-w", "--stdin"], {
    cwd: f.cwd,
    encoding: "utf8",
    input: "",
  }).trim();
  const entries = Array.from(
    { length: 5300 },
    (_, index) =>
      `100644 ${blob}\tbulk/${String(index).padStart(5, "0")}-${"x".repeat(190)}.txt\n`,
  ).join("");
  f.git("read-tree", f.base);
  execFileSync("git", ["update-index", "--index-info"], {
    cwd: f.cwd,
    input: entries,
  });
  const tree = f.git("write-tree");
  const head = f.git("commit-tree", tree, "-p", f.base, "-m", "bulk");
  // Guard the fixture so it cannot silently shrink below Node's 1 MiB default.
  const listed = execFileSync(
    "git",
    ["diff", "--name-only", "--no-renames", "-z", f.base, head, "--"],
    { cwd: f.cwd, maxBuffer: 32 * 1024 * 1024 },
  );
  assert.ok(listed.length > 1024 * 1024);
  assert.deepEqual(plan(f.cwd, f.base, head), {
    quality: false,
    format: false,
    fixtures: false,
    go: false,
    e2e: false,
  });
});

test("planner CLI runs through physical and symlinked script paths", (t) => {
  const f = fixture(t);
  f.write("docs/guide.md");
  const head = f.commit();
  const links = mkdtempSync(join(tmpdir(), "ci-plan-link-"));
  t.after(() => rmSync(links, { recursive: true, force: true }));
  symlinkSync(
    join(root, "scripts/ci"),
    join(links, "ci"),
    process.platform === "win32" ? "junction" : "dir",
  );
  for (const script of [
    join(root, "scripts/ci/plan.mjs"),
    join(links, "ci/plan.mjs"),
  ]) {
    const output = join(f.cwd, "cli-output");
    const summary = join(f.cwd, "cli-summary");
    rmSync(output, { force: true });
    const result = spawnSync(process.execPath, [script], {
      cwd: f.cwd,
      encoding: "utf8",
      env: {
        ...process.env,
        CI_BASE: f.base,
        CI_HEAD: head,
        GITHUB_OUTPUT: output,
        GITHUB_STEP_SUMMARY: summary,
      },
    });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /"event":"ci_plan"/, script);
    assert.equal(
      readFileSync(output, "utf8"),
      "quality=true\nformat=true\nfixtures=false\ngo=false\ne2e=false\n",
      script,
    );
  }
});
