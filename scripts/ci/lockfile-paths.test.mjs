// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { lockfilePaths } from "./lockfile-paths.mjs";
import { select } from "./plan.mjs";

const manager = `---
lockfileVersion: '9.0'
importers:
  .:
    packageManagerDependencies:
      pnpm:
        version: 12.14.0
packages: {}
snapshots: {}
`;
const lockfile = (
  name = "apps/delino-docs",
  version = "19.2.8",
) => `${manager}---
lockfileVersion: '9.0'
settings:
  autoInstallPeers: true
importers:
  .:
    devDependencies: {}
  ${name}:
    dependencies:
      react:
        specifier: ${version}
        version: ${version}
  servers/delcore-api: {}
packages:
  react@19.2.8:
    resolution: {integrity: sha512-fixture}
snapshots:
  react@19.2.8: {}
`;
const baseLockfile = lockfile();
const headLockfile = lockfile("apps/delino-docs", "19.2.9");
const globalSelection = select(["package.json"]);

// Adding an already resolved dependency changes only an importer, as in the
// reported docs navigation change. Package/snapshot changes must remain global.
test("importer-only edits select their owner across pnpm documents", () => {
  assert.deepEqual(lockfilePaths(baseLockfile, headLockfile), [
    "apps/delino-docs/package.json",
  ]);
  assert.deepEqual(
    select(["pnpm-lock.yaml"], { baseLockfile, headLockfile }),
    select(["apps/delino-docs/package.json"]),
  );
  assert.deepEqual(
    select(["pnpm-lock.yaml", "servers/delcore-api/main.go"], {
      baseLockfile,
      headLockfile,
    }),
    select(["apps/delino-docs/package.json", "servers/delcore-api/main.go"]),
  );
  assert.deepEqual(lockfilePaths(baseLockfile, baseLockfile), []);
});

test("added and removed importer blocks select their owner", () => {
  const removed = baseLockfile.replace(
    /  apps\/delino-docs:\n[\s\S]*?(?=^  servers\/delcore-api:)/mu,
    "",
  );
  for (const [base, head] of [
    [removed, baseLockfile],
    [baseLockfile, removed],
  ]) {
    assert.deepEqual(lockfilePaths(base, head), [
      "apps/delino-docs/package.json",
    ]);
  }
});

test("single-document lockfiles and multiple changed importers retain ownership", () => {
  const before = baseLockfile.replace(manager, "");
  const after = headLockfile.replace(manager, "");
  assert.deepEqual(lockfilePaths(before, after), [
    "apps/delino-docs/package.json",
  ]);
  const extra = "  apps/delino-site:\n    dependencies: {}\n";
  const both = after.replace("packages:\n", `${extra}packages:\n`);
  assert.deepEqual(
    select(["pnpm-lock.yaml"], { baseLockfile: before, headLockfile: both }),
    select(["apps/delino-docs/package.json", "apps/delino-site/package.json"]),
  );
  assert.deepEqual(
    select(["pnpm-lock.yaml", "go.mod"], {
      baseLockfile: before,
      headLockfile: both,
    }),
    globalSelection,
  );
});

test("shared importer edits preserve shared consumers", () => {
  assert.deepEqual(
    select(["pnpm-lock.yaml"], {
      baseLockfile: lockfile("packages/ui"),
      headLockfile: lockfile("packages/ui", "19.2.9"),
    }),
    globalSelection,
  );
});

test("root, package-manager, resolution, snapshot and setting edits stay global", () => {
  for (const head of [
    baseLockfile.replace("devDependencies: {}", "dependencies: {}"),
    baseLockfile.replace("12.14.0", "12.14.1"),
    baseLockfile.replace("sha512-fixture", "sha512-changed"),
    baseLockfile.replace("react@19.2.8: {}", "react@19.2.8: {optional: true}"),
    baseLockfile.replace("autoInstallPeers: true", "autoInstallPeers: false"),
  ]) {
    assert.deepEqual(
      select(["pnpm-lock.yaml"], { baseLockfile, headLockfile: head }),
      globalSelection,
    );
  }
});

test("unknown ownership and unsupported or unavailable layouts stay global", () => {
  for (const [base, head] of [
    [undefined, headLockfile],
    [baseLockfile, undefined],
    [baseLockfile, ""],
    [
      baseLockfile.replaceAll("9.0", "10.0"),
      headLockfile.replaceAll("9.0", "10.0"),
    ],
    [
      baseLockfile.replace("apps/delino-docs:", "'apps/delino-docs':"),
      headLockfile.replace("apps/delino-docs:", "'apps/delino-docs':"),
    ],
    [lockfile("apps/unknown"), lockfile("apps/unknown", "19.2.9")],
    [
      baseLockfile,
      headLockfile.replace(
        "  apps/delino-docs:",
        "  apps/delino-docs:\n  apps/delino-docs:",
      ),
    ],
  ]) {
    assert.deepEqual(
      select(["pnpm-lock.yaml"], { baseLockfile: base, headLockfile: head }),
      globalSelection,
    );
  }
});

test("planner CLI compares committed revisions rather than worktree lockfiles", (t) => {
  const cwd = mkdtempSync(join(tmpdir(), "ci-lockfile-"));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  const git = (...args) =>
    execFileSync("git", args, { cwd, encoding: "utf8" }).trim();
  git("init", "--quiet");
  git("config", "user.name", "CI fixture");
  git("config", "user.email", "fixture@example.invalid");
  git("config", "core.hooksPath", "/dev/null");
  writeFileSync(join(cwd, "pnpm-lock.yaml"), baseLockfile);
  git("add", "pnpm-lock.yaml");
  git("commit", "--quiet", "-m", "base");
  const base = git("rev-parse", "HEAD");
  writeFileSync(join(cwd, "pnpm-lock.yaml"), headLockfile);
  git("add", "pnpm-lock.yaml");
  git("commit", "--quiet", "-m", "head");
  const head = git("rev-parse", "HEAD");
  writeFileSync(join(cwd, "pnpm-lock.yaml"), "uncommitted invalid content");
  const output = join(cwd, "output");
  const log = execFileSync(
    process.execPath,
    [fileURLToPath(new URL("./plan.mjs", import.meta.url))],
    {
      cwd,
      encoding: "utf8",
      env: {
        ...process.env,
        GITHUB_EVENT_NAME: "pull_request",
        COMPARISON_BASE: base,
        GITHUB_OUTPUT: output,
      },
    },
  );
  assert.deepEqual(JSON.parse(log), {
    event: "ci_selection",
    base,
    head,
    selected: select(["apps/delino-docs/package.json"]),
  });
  assert.match(readFileSync(output, "utf8"), /^delcore_api=false$/mu);
  assert.match(readFileSync(output, "utf8"), /^delino_docs=true$/mu);
});
