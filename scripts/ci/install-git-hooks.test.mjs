// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  realpathSync,
  rmSync,
  symlinkSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { createRequire } from "node:module";
import { delimiter, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const root = fileURLToPath(new URL("../..", import.meta.url));
const require = createRequire(import.meta.url);
const lefthook = require("lefthook/get-exe.js").getExePath();
const skipMessage = /skipping Lefthook installation/;

function environment() {
  const env = { ...process.env };
  // A developer's global core.hooksPath or an enclosing Git hook invocation must
  // not change which branch of the installer runs.
  for (const name of Object.keys(env)) {
    if (name.startsWith("GIT_") || name === "LEFTHOOK_CONFIG") {
      delete env[name];
    }
  }
  env.GIT_CONFIG_GLOBAL = process.platform === "win32" ? "NUL" : "/dev/null";
  env.GIT_CONFIG_NOSYSTEM = "1";
  env.PATH = [dirname(lefthook), env.PATH].join(delimiter);
  return env;
}

function fixture(t) {
  const base = realpathSync(mkdtempSync(join(tmpdir(), "install-hooks-")));
  t.after(() => rmSync(base, { recursive: true, force: true }));
  const env = environment();
  const git = (cwd, ...args) =>
    execFileSync("git", args, { cwd, env, encoding: "utf8" }).trim();
  const initRepository = (path) => {
    mkdirSync(path, { recursive: true });
    git(path, "init", "--quiet");
    git(path, "config", "user.email", "fixture@example.invalid");
    git(path, "config", "user.name", "Hook installer fixture");
    git(path, "commit", "--quiet", "--allow-empty", "-m", "fixture");
  };
  // Copy the files a source archive of this repository would contain.
  const copyProject = (path) => {
    mkdirSync(join(path, "scripts"), { recursive: true });
    copyFileSync(join(root, "lefthook.yml"), join(path, "lefthook.yml"));
    copyFileSync(
      join(root, "scripts/install-git-hooks.sh"),
      join(path, "scripts/install-git-hooks.sh"),
    );
  };
  const install = (cwd, script = "scripts/install-git-hooks.sh") =>
    spawnSync("sh", [script], { cwd, env, encoding: "utf8" });
  const installedHooks = (hooksDirectory) =>
    existsSync(hooksDirectory)
      ? readdirSync(hooksDirectory).filter((name) => !name.endsWith(".sample"))
      : [];
  return { base, git, initRepository, copyProject, install, installedHooks };
}

test("installs hooks in a normal clone", (t) => {
  const { base, initRepository, copyProject, install } = fixture(t);
  const clone = join(base, "clone");
  initRepository(clone);
  copyProject(clone);

  const result = install(clone);

  assert.equal(result.status, 0, result.stderr);
  assert.ok(existsSync(join(clone, ".git/hooks/pre-commit")));
});

test("installs hooks when run from the scripts directory", (t) => {
  const { base, initRepository, copyProject, install } = fixture(t);
  const clone = join(base, "clone");
  initRepository(clone);
  copyProject(clone);

  const result = install(join(clone, "scripts"), "./install-git-hooks.sh");

  assert.equal(result.status, 0, result.stderr);
  assert.ok(existsSync(join(clone, ".git/hooks/pre-commit")));
});

test("installs hooks in a clone reached through a symlinked path", (t) => {
  const { base, initRepository, copyProject, install } = fixture(t);
  const clone = join(base, "clone");
  initRepository(clone);
  copyProject(clone);
  const link = join(base, "link");
  symlinkSync(clone, link, process.platform === "win32" ? "junction" : "dir");

  const result = install(link);

  assert.equal(result.status, 0, result.stderr);
  assert.doesNotMatch(result.stdout, skipMessage);
  assert.ok(existsSync(join(clone, ".git/hooks/pre-commit")));
});

test("installs hooks in a linked worktree that uses the shared hooks directory", (t) => {
  const { base, git, initRepository, copyProject, install } = fixture(t);
  const main = join(base, "main");
  initRepository(main);
  const linked = join(base, "linked");
  git(main, "worktree", "add", "--quiet", "-b", "linked", linked);
  copyProject(linked);
  const sharedHooks = join(main, ".git/hooks");
  git(main, "config", "core.hooksPath", sharedHooks);

  const result = install(linked);

  assert.equal(result.status, 0, result.stderr);
  assert.doesNotMatch(result.stdout, skipMessage);
  assert.ok(existsSync(join(sharedHooks, "pre-commit")));
});

test("keeps Lefthook's protection for an unrelated custom hooks path", (t) => {
  const { base, git, initRepository, copyProject, install } = fixture(t);
  const clone = join(base, "clone");
  initRepository(clone);
  copyProject(clone);
  const customHooks = join(base, "custom-hooks");
  mkdirSync(customHooks);
  git(clone, "config", "core.hooksPath", customHooks);

  const result = install(clone);

  assert.notEqual(result.status, 0);
  assert.equal(existsSync(join(customHooks, "pre-commit")), false);
});

test("skips an archive extracted inside another repository", (t) => {
  const { base, initRepository, copyProject, install, installedHooks } =
    fixture(t);
  const outer = join(base, "outer");
  initRepository(outer);
  const archive = join(outer, "archive");
  copyProject(archive);

  const result = install(archive);

  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, skipMessage);
  assert.equal(existsSync(join(outer, "lefthook.yml")), false);
  assert.deepEqual(installedHooks(join(outer, ".git/hooks")), []);
});

test("skips an archive nested in another repository when started from the outer directory", (t) => {
  const { base, initRepository, copyProject, install, installedHooks } =
    fixture(t);
  const outer = join(base, "outer");
  initRepository(outer);
  copyProject(join(outer, "archive"));

  const result = install(outer, "archive/scripts/install-git-hooks.sh");

  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, skipMessage);
  assert.equal(existsSync(join(outer, "lefthook.yml")), false);
  assert.deepEqual(installedHooks(join(outer, ".git/hooks")), []);
});

test("skips an absolute script path run from an unrelated repository", (t) => {
  const { base, initRepository, copyProject, install, installedHooks } =
    fixture(t);
  const unrelated = join(base, "unrelated");
  initRepository(unrelated);
  const archive = join(base, "archive");
  copyProject(archive);

  const result = install(
    unrelated,
    join(archive, "scripts/install-git-hooks.sh"),
  );

  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, skipMessage);
  assert.equal(existsSync(join(unrelated, "lefthook.yml")), false);
  assert.deepEqual(installedHooks(join(unrelated, ".git/hooks")), []);
});

test("skips an archive without any Git metadata", (t) => {
  const { base, copyProject, install } = fixture(t);
  const archive = join(base, "archive");
  copyProject(archive);

  const result = install(archive);

  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, skipMessage);
  assert.equal(existsSync(join(archive, ".git")), false);
});
