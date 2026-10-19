import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { hookInstallArguments, installGitHooks } from "./install-git-hooks.mjs";

function fixture(context) {
  const root = mkdtempSync(join(tmpdir(), "fixkey hooks "));
  context.after(() => rmSync(root, { recursive: true, force: true }));
  const repository = join(root, "repository with spaces");
  mkdirSync(repository);
  const git = (...args) => {
    const result = spawnSync("git", args, {
      cwd: repository,
      encoding: "utf8",
      shell: false,
    });
    assert.equal(result.status, 0, result.stderr);
    return result.stdout.trim();
  };
  git("init", "--initial-branch=main");
  git("config", "user.name", "Hook fixture");
  git("config", "user.email", "hooks@example.invalid");
  return { root, repository, git };
}

test("skips directories without Git metadata", (context) => {
  const { root } = fixture(context);
  assert.equal(hookInstallArguments(root), null);
  assert.equal(
    installGitHooks(root, () => assert.fail("must not install hooks")),
    0,
  );
});

test("runs the pinned Node entrypoint without a shell and propagates failure", (context) => {
  const { repository } = fixture(context);
  assert.deepEqual(hookInstallArguments(repository), ["install"]);
  const status = installGitHooks(repository, (command, args, options) => {
    assert.equal(command, process.execPath);
    assert.match(args[0], /lefthook[\\/]bin[\\/]index\.js$/u);
    assert.deepEqual(args.slice(1), ["install"]);
    assert.equal(options.cwd, repository);
    assert.equal(options.shell, false);
    return { status: 7 };
  });
  assert.equal(status, 7);
  assert.equal(
    installGitHooks(repository, () => ({ status: null, signal: "SIGTERM" })),
    1,
  );
  assert.throws(
    () =>
      installGitHooks(repository, () => ({ error: new Error("spawn failed") })),
    /spawn failed/u,
  );
});

test("forces only the shared hooks directory for absolute and relative paths", (context) => {
  const { repository, git } = fixture(context);
  mkdirSync(join(repository, ".git", "hooks"), { recursive: true });
  for (const hooks of [".git/hooks", join(repository, ".git", "hooks")]) {
    git("config", "core.hooksPath", hooks);
    assert.deepEqual(hookInstallArguments(repository), ["install", "--force"]);
  }
  git("config", "core.hooksPath", "custom hooks");
  assert.deepEqual(hookInstallArguments(repository), ["install"]);
});

test("uses the common hooks directory in linked worktrees", (context) => {
  const { root, repository, git } = fixture(context);
  git("-c", "core.hooksPath=", "commit", "--allow-empty", "-m", "fixture");
  const worktree = join(root, "linked worktree");
  git("worktree", "add", "-b", "linked", worktree);
  git("config", "core.hooksPath", join(repository, ".git", "hooks"));
  assert.deepEqual(hookInstallArguments(worktree), ["install", "--force"]);
  git("config", "core.hooksPath", join(root, "unrelated hooks"));
  assert.deepEqual(hookInstallArguments(worktree), ["install"]);
});
