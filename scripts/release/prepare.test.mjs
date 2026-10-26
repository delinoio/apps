// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { prepare } from "./prepare.mjs";
const git = (cwd, ...args) =>
  execFileSync("git", args, {
    cwd,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  }).trim();
function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), "release-git-")),
    repo = join(root, "repo"),
    remote = join(root, "remote.git");
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(repo);
  git(root, "init", "--bare", remote);
  git(repo, "init", "-b", "main");
  git(repo, "config", "user.name", "Release fixture");
  git(repo, "config", "user.email", "fixture@example.invalid");
  mkdirSync(join(repo, "apps/devbase"), { recursive: true });
  mkdirSync(join(repo, ".changeset"));
  writeFileSync(
    join(repo, "apps/devbase/package.json"),
    '{\n  "name": "@apps/devbase",\n  "version": "1.0.0"\n}\n',
  );
  writeFileSync(
    join(repo, ".changeset/fixture.md"),
    '---\n"@apps/devbase": minor\n---\nInternal\n',
  );
  git(repo, "add", ".");
  git(repo, "commit", "-m", "fixture");
  git(repo, "remote", "add", "origin", remote);
  git(repo, "push", "origin", "main");
  const options = {
    directory: repo,
    runId: "123",
    loadRecords: () => ({
      records: [{ id: "fixture", packages: { "@apps/devbase": "minor" } }],
      inventory: { "@apps/devbase": "apps/devbase/package.json" },
    }),
  };
  return { root, repo, remote, options };
}
test("one commit consumes changesets and pushes the exact deployment source without tags", (t) => {
  const { repo, remote, options } = fixture(t);
  const result = prepare(options);
  assert.equal(git(remote, "rev-parse", "main"), result.revision);
  assert.equal(git(repo, "tag", "--list"), "");
  assert.equal(git(repo, "rev-list", "--count", "HEAD"), "2");
  assert.equal(git(repo, "ls-files", ".changeset"), "");
  assert.equal(
    JSON.parse(readFileSync(join(repo, "apps/devbase/package.json"))).version,
    "1.1.0",
  );
});
test("remote main race refuses release without any remote tag or release commit", (t) => {
  const { root, repo, remote, options } = fixture(t);
  const other = join(root, "other");
  git(root, "clone", "-b", "main", remote, other);
  git(other, "config", "user.name", "Other fixture");
  git(other, "config", "user.email", "other@example.invalid");
  writeFileSync(join(other, "advanced"), "main");
  git(other, "add", ".");
  git(other, "commit", "-m", "advance");
  git(other, "push", "origin", "main");
  const before = git(remote, "rev-parse", "main");
  assert.throws(() => prepare(options), /Main advanced/);
  assert.equal(git(remote, "rev-parse", "main"), before);
  assert.equal(git(remote, "tag", "--list"), "");
  assert.equal(git(repo, "status", "--porcelain"), "");
});
test("no pending changesets fails without creating a release commit", (t) => {
  const { options, repo, remote } = fixture(t);
  const before = git(remote, "rev-parse", "main");
  assert.throws(
    () =>
      prepare({
        ...options,
        loadRecords: () => ({ records: [], inventory: {} }),
      }),
    /No pending changesets/,
  );
  assert.equal(git(remote, "rev-parse", "main"), before);
  assert.equal(git(repo, "status", "--porcelain"), "");
});
