// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { execFileSync, spawn, spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  renameSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { createRequire } from "node:module";
import { delimiter, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import {
  allFormatFiles,
  changedFormatFiles,
  ciEnabled,
  runFormatter,
  stagedFormatFiles,
} from "./format-changed.mjs";

const root = fileURLToPath(new URL("../..", import.meta.url));
const require = createRequire(import.meta.url);
const lefthook = require("lefthook/get-exe.js").getExePath();

// A developer's global or system Git configuration (core.hooksPath,
// commit.gpgsign, ...) and the GIT_* variables of an enclosing Git hook
// invocation (GIT_DIR, GIT_INDEX_FILE, ...) must not change how the fixture
// repositories behave. Mutates and returns `env`.
function isolateGitEnvironment(env) {
  for (const name of Object.keys(env)) {
    if (name.startsWith("GIT_") || name === "LEFTHOOK_CONFIG") {
      delete env[name];
    }
  }
  env.GIT_CONFIG_GLOBAL = process.platform === "win32" ? "NUL" : "/dev/null";
  env.GIT_CONFIG_NOSYSTEM = "1";
  return env;
}

// The exported selectors run git without an explicit environment, so they
// inherit this process's environment; isolate it before any test runs.
isolateGitEnvironment(process.env);

function fixture(t, initialCommit = true, parentEnv = process.env) {
  const cwd = mkdtempSync(join(tmpdir(), "rstack-format-"));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  const env = isolateGitEnvironment({ ...parentEnv, LEFTHOOK_BIN: lefthook });
  delete env.CI;
  delete env.TURBO_SCM_BASE;
  delete env.TURBO_SCM_HEAD;
  env.PATH = [
    dirname(process.execPath),
    join(root, "node_modules/.bin"),
    env.PATH,
  ].join(delimiter);
  const git = (...args) =>
    execFileSync("git", args, { cwd, env, encoding: "utf8" }).trim();
  git("init", "--quiet");
  git("config", "user.email", "fixture@example.invalid");
  git("config", "user.name", "Formatting fixture");
  git("config", "core.hooksPath", ".no-hooks");
  const write = (name, text) => {
    mkdirSync(dirname(join(cwd, name)), { recursive: true });
    writeFileSync(join(cwd, name), text);
  };
  write("package.json", '{"type":"module"}\n');
  copyFileSync(join(root, "rstack.config.ts"), join(cwd, "rstack.config.ts"));
  symlinkSync(
    join(root, "node_modules"),
    join(cwd, "node_modules"),
    process.platform === "win32" ? "junction" : "dir",
  );
  write(".gitignore", "node_modules\n");
  const commit = () => {
    git("add", ".");
    git("commit", "--quiet", "-m", "fixture");
    return git("rev-parse", "HEAD");
  };
  const installHook = () => {
    copyFileSync(join(root, "lefthook.yml"), join(cwd, "lefthook.yml"));
    write(
      "scripts/ci/format-changed.mjs",
      readFileSync(join(root, "scripts/ci/format-changed.mjs"), "utf8"),
    );
    git("config", "--unset", "core.hooksPath");
    execFileSync(lefthook, ["install"], { cwd, env, encoding: "utf8" });
  };
  return {
    cwd,
    write,
    git,
    commit,
    installHook,
    env,
    base: initialCommit ? commit() : null,
  };
}

test("fixture ignores hostile global Git configuration and GIT_* variables", (t) => {
  const hostile = mkdtempSync(join(tmpdir(), "rstack-hostile-"));
  t.after(() => rmSync(hostile, { recursive: true, force: true }));
  const globalConfig = join(hostile, "gitconfig");
  writeFileSync(
    globalConfig,
    `[core]\n\thooksPath = ${join(hostile, "hooks")}\n[commit]\n\tgpgsign = true\n`,
  );
  const bare = join(hostile, "bare.git");
  execFileSync("git", ["init", "--quiet", "--bare", bare], {
    env: isolateGitEnvironment({ PATH: process.env.PATH }),
  });
  const f = fixture(t, true, {
    ...process.env,
    GIT_CONFIG_GLOBAL: globalConfig,
    GIT_DIR: bare,
    GIT_INDEX_FILE: join(hostile, "index"),
    GIT_WORK_TREE: hostile,
  });
  f.installHook();
  f.write("hostile.ts", "const value={answer:42}\n");
  f.git("add", "hostile.ts");
  f.git("commit", "--quiet", "-m", "hostile environment");
  assert.equal(
    f.git("show", "HEAD:hostile.ts"),
    "const value = { answer: 42 };",
  );
});

test("pre-commit formats and stages initial-commit files with literal paths", (t) => {
  const f = fixture(t, false);
  f.installHook();
  const names = [
    "space name.tsx",
    "한글[1].ts",
    "-flag.ts",
    "!negation.ts",
    "apps/example/src/desktop.test.tsx",
  ];
  if (process.platform !== "win32") names.push("line\nname.ts", "star*.ts");
  for (const name of names) f.write(name, "const value={answer:42}\n");
  f.write("vendor/ignored.ts", "deliberately invalid {{{\n");
  f.write("untracked.ts", "const untracked={answer:42}\n");
  f.git("add", "--", ...names, "vendor/ignored.ts");
  assert.deepEqual(
    stagedFormatFiles(f.cwd),
    [...names, "vendor/ignored.ts"].sort(),
  );
  f.git("commit", "--quiet", "-m", "initial formatted sources");
  for (const name of names) {
    assert.equal(
      f.git("show", `HEAD:${name}`),
      "const value = { answer: 42 };",
    );
    assert.equal(
      readFileSync(join(f.cwd, name), "utf8"),
      "const value = { answer: 42 };\n",
    );
  }
  assert.equal(
    f.git("show", "HEAD:vendor/ignored.ts"),
    "deliberately invalid {{{",
  );
  assert.equal(
    readFileSync(join(f.cwd, "untracked.ts"), "utf8"),
    "const untracked={answer:42}\n",
  );
  assert.equal(f.git("diff", "--name-only"), "");
});

test("pre-commit preserves unstaged edits in a partially staged file", (t) => {
  const f = fixture(t);
  const padding = Array.from(
    { length: 15 },
    (_, index) => `// Separator ${index}\n`,
  ).join("");
  const original = `const value = { answer: 1 };\n${padding}const tail = 1;\n`;
  f.write("partial.ts", original);
  f.commit();
  f.installHook();
  const staged = `const value={answer:42}\n${padding}const tail = 1;\n`;
  f.write("partial.ts", staged);
  f.git("add", "partial.ts");
  f.write("partial.ts", staged.replace("const tail = 1;", "const tail = 2;"));
  f.git("commit", "--quiet", "-m", "format only staged content");
  const formatted = `const value = { answer: 42 };\n${padding}const tail = 1;\n`;
  assert.equal(f.git("show", "HEAD:partial.ts"), formatted.trim());
  assert.equal(
    readFileSync(join(f.cwd, "partial.ts"), "utf8"),
    formatted.replace("const tail = 1;", "const tail = 2;"),
  );
  assert.equal(f.git("diff", "--cached", "--name-only"), "");
  assert.equal(f.git("diff", "--name-only"), "partial.ts");
});

test("pre-commit rejects a conflicting unstaged edit and preserves both versions", (t) => {
  const f = fixture(t);
  f.write("conflict.ts", "const value = { answer: 1 };\n");
  const head = f.commit();
  f.installHook();
  const staged = "const value={answer:42}\n";
  const unstaged = "const value={answer:43}\n";
  f.write("conflict.ts", staged);
  f.git("add", "conflict.ts");
  f.write("conflict.ts", unstaged);
  const result = spawnSync(
    "git",
    ["commit", "--quiet", "-m", "format conflicting content"],
    { cwd: f.cwd, env: f.env, encoding: "utf8" },
  );
  assert.notEqual(result.status, 0, result.stdout + result.stderr);
  assert.equal(f.git("rev-parse", "HEAD"), head);
  assert.equal(readFileSync(join(f.cwd, "conflict.ts"), "utf8"), unstaged);
  assert.equal(f.git("show", ":conflict.ts"), staged.trim());
  assert.equal(f.git("stash", "list"), "");
  const info = join(f.cwd, f.git("rev-parse", "--git-path", "info"));
  assert.deepEqual(
    readdirSync(info).filter((name) => name.endsWith(".patch")),
    [],
  );
});

test("pre-commit formats initial commits when CI is explicitly false", (t) => {
  const f = fixture(t, false);
  f.env.CI = "false";
  f.installHook();
  f.write("initial.ts", "const value={answer:42}\n");
  f.git("add", "initial.ts");
  f.git("commit", "--quiet", "-m", "initial formatted source");
  assert.equal(
    f.git("show", "HEAD:initial.ts"),
    "const value = { answer: 42 };",
  );
  assert.equal(
    readFileSync(join(f.cwd, "initial.ts"), "utf8"),
    "const value = { answer: 42 };\n",
  );
  assert.equal(f.git("diff", "--name-only"), "");
});

test("pre-commit preserves unstaged edits when CI is explicitly false", (t) => {
  const f = fixture(t);
  f.env.CI = "False";
  const padding = Array.from(
    { length: 15 },
    (_, index) => `// Separator ${index}\n`,
  ).join("");
  f.write(
    "partial.ts",
    `const value = { answer: 1 };\n${padding}const tail = 1;\n`,
  );
  f.commit();
  f.installHook();
  const staged = `const value={answer:42}\n${padding}const tail = 1;\n`;
  f.write("partial.ts", staged);
  f.git("add", "partial.ts");
  f.write("partial.ts", staged.replace("const tail = 1;", "const tail = 2;"));
  f.git("commit", "--quiet", "-m", "format only staged content");
  const formatted = `const value = { answer: 42 };\n${padding}const tail = 1;\n`;
  assert.equal(f.git("show", "HEAD:partial.ts"), formatted.trim());
  assert.equal(
    readFileSync(join(f.cwd, "partial.ts"), "utf8"),
    formatted.replace("const tail = 1;", "const tail = 2;"),
  );
  assert.equal(f.git("diff", "--name-only"), "partial.ts");
});

test("pre-commit skips deletion-only and unsupported-file commits", (t) => {
  const f = fixture(t);
  f.write("removed.ts", "const value = 1;\n");
  f.commit();
  f.installHook();
  f.git("rm", "removed.ts");
  assert.deepEqual(stagedFormatFiles(f.cwd), []);
  f.git("commit", "--quiet", "-m", "delete source");
  f.write("notes.txt", "plain text\n");
  f.git("add", "notes.txt");
  assert.deepEqual(stagedFormatFiles(f.cwd), []);
  f.git("commit", "--quiet", "-m", "unsupported source");
  assert.equal(f.git("show", "HEAD:notes.txt"), "plain text");
});

test("pre-commit rejects parser errors without creating a commit", (t) => {
  const f = fixture(t);
  f.installHook();
  f.write("broken.ts", "const broken = ;\n");
  f.git("add", "broken.ts");
  const result = spawnSync(
    "git",
    ["commit", "--quiet", "-m", "invalid source"],
    {
      cwd: f.cwd,
      env: f.env,
      encoding: "utf8",
    },
  );
  assert.notEqual(result.status, 0);
  assert.equal(f.git("rev-parse", "HEAD"), f.base);
  assert.match(result.stdout + result.stderr, /ci_format_result/);
  assert.equal(f.git("show", ":broken.ts"), "const broken = ;");
});

test(
  "staged selection skips symlinks and fails for a staged file removed from the worktree",
  { skip: process.platform === "win32" },
  (t) => {
    const f = fixture(t);
    f.write("target.ts", "const value = 1;\n");
    symlinkSync("target.ts", join(f.cwd, "linked.ts"));
    f.git("add", "target.ts", "linked.ts");
    assert.deepEqual(stagedFormatFiles(f.cwd), ["target.ts"]);
    rmSync(join(f.cwd, "target.ts"));
    assert.throws(() => stagedFormatFiles(f.cwd), /ENOENT.*target\.ts/u);
  },
);

test(
  "staged symlinks stay exempt when their worktree entry is missing, and a staged type change is judged by its new mode",
  { skip: process.platform === "win32" },
  (t) => {
    const f = fixture(t);
    f.write("target.ts", "const value = 1;\n");
    f.write("retyped.ts", "const value = 2;\n");
    f.commit();
    symlinkSync("target.ts", join(f.cwd, "linked.ts"));
    // Replace a tracked regular file with a symlink: a staged type change.
    rmSync(join(f.cwd, "retyped.ts"));
    symlinkSync("target.ts", join(f.cwd, "retyped.ts"));
    f.git("add", "linked.ts", "retyped.ts");
    assert.deepEqual(stagedFormatFiles(f.cwd), []);
    // Remove the placeholders, as a sparse checkout or `rm` after `git add`
    // would: a staged symlink is never formatted, so it needs no worktree file.
    rmSync(join(f.cwd, "linked.ts"));
    rmSync(join(f.cwd, "retyped.ts"));
    assert.deepEqual(stagedFormatFiles(f.cwd), []);
    // A missing regular file is still reported next to the exempt symlinks.
    f.write("added.ts", "const   value = 3;\n");
    f.git("add", "added.ts");
    rmSync(join(f.cwd, "added.ts"));
    assert.throws(() => stagedFormatFiles(f.cwd), /added\.ts/u);
    // A staged type change from a symlink back to a regular file is formatted.
    f.git("rm", "--cached", "--quiet", "--", "added.ts");
    f.git("commit", "--quiet", "-m", "symlinks");
    f.write("retyped.ts", "const   value = 4;\n");
    f.git("add", "retyped.ts");
    assert.deepEqual(stagedFormatFiles(f.cwd), ["retyped.ts"]);
  },
);

test("staged selection fails naming each kind of staged path missing from the worktree", (t) => {
  const f = fixture(t);
  f.write("modified.ts", "const value = 1;\n");
  f.write("renamed.ts", "const value = 2;\n");
  f.commit();
  f.write("modified.ts", "const value = 3;\n");
  f.write("added.ts", "const   value = 4;\n");
  f.git("mv", "renamed.ts", "moved.ts");
  f.git("add", "modified.ts", "added.ts");
  assert.deepEqual(stagedFormatFiles(f.cwd), [
    "added.ts",
    "modified.ts",
    "moved.ts",
  ]);
  for (const name of ["added.ts", "modified.ts", "moved.ts"]) {
    rmSync(join(f.cwd, name));
    assert.throws(
      () => stagedFormatFiles(f.cwd),
      (error) => {
        assert.match(error.message, /missing from the working tree/u);
        assert.ok(error.message.includes(name), error.message);
        return true;
      },
    );
    // Restoring the file lets the selection succeed again.
    f.git("checkout-index", "--force", "--", name);
  }
  assert.equal(stagedFormatFiles(f.cwd).length, 3);
});

test("staged deletions and unsupported missing paths do not fail the selection", (t) => {
  const f = fixture(t);
  f.write("removed.ts", "const value = 1;\n");
  f.write("notes.txt", "plain text\n");
  f.write("kept.ts", "const value = 2;\n");
  f.commit();
  f.git("rm", "removed.ts");
  f.write("notes.txt", "changed text\n");
  f.write("new-notes.txt", "new text\n");
  f.git("add", "notes.txt", "new-notes.txt");
  rmSync(join(f.cwd, "new-notes.txt"));
  assert.deepEqual(stagedFormatFiles(f.cwd), []);
  f.write("kept.ts", "const   value = 3;\n");
  f.git("add", "kept.ts");
  assert.deepEqual(stagedFormatFiles(f.cwd), ["kept.ts"]);
});

// Same list as `extensions` in format-changed.mjs; the exclusion probe must work
// for each of them, so a missing extension here would hide a broken probe.
const formatExtensions = [
  ".js",
  ".jsx",
  ".mjs",
  ".cjs",
  ".ts",
  ".tsx",
  ".mts",
  ".cts",
  ".css",
  ".json",
  ".jsonc",
  ".json5",
  ".yaml",
  ".yml",
  ".md",
  ".mdx",
];

// Staged paths that the shared rstack.config.ts exclusions skip even when they
// are present. Their missing worktree file must not fail the selection.
const excludedStagedPaths = [
  "pnpm-lock.yaml",
  "apps/web/pnpm-lock.yaml",
  "package-lock.json",
  "vendor/lib.ts",
  "packages/a/third-party/lib.js",
  "src/schema.generated.ts",
  "protos/gen/api.json",
  ".agents/skills/example/SKILL.md",
];

test("staged selection skips paths excluded by the shared configuration when they are missing from the worktree", (t) => {
  const f = fixture(t);
  for (const name of excludedStagedPaths) f.write(name, "a:   1\n");
  f.write("kept.ts", "const value = 1;\n");
  f.commit();
  for (const name of excludedStagedPaths) f.write(name, "a:    2\n");
  f.write("kept.ts", "const   value = 2;\n");
  f.git("add", ".");
  // Present excluded paths are selected and left to Rstack, as before.
  assert.deepEqual(
    stagedFormatFiles(f.cwd),
    [...excludedStagedPaths, "kept.ts"].sort(),
  );
  for (const name of excludedStagedPaths) rmSync(join(f.cwd, name));
  assert.deepEqual(stagedFormatFiles(f.cwd), ["kept.ts"]);
  // An excluded missing path never hides a missing path Rstack would format.
  rmSync(join(f.cwd, "kept.ts"));
  assert.throws(
    () => stagedFormatFiles(f.cwd),
    /missing from the working tree.*kept\.ts/u,
  );
  // A path excluded by its name is still excluded behind a replaced parent.
  f.git("checkout-index", "--force", "--", "kept.ts");
  rmSync(join(f.cwd, "vendor"), { recursive: true, force: true });
  f.write("vendor", "not a directory\n");
  assert.deepEqual(stagedFormatFiles(f.cwd), ["kept.ts"]);
});

test("the exclusion probe covers every supported extension", (t) => {
  const f = fixture(t);
  for (const extension of formatExtensions) {
    f.write(`vendor/probe${extension}`, "x\n");
    f.write(`probe${extension}`, "x\n");
  }
  f.git("add", ".");
  for (const extension of formatExtensions) {
    // The excluded path alone is missing: nothing fails, the present path stays.
    rmSync(join(f.cwd, `vendor/probe${extension}`));
    assert.ok(
      stagedFormatFiles(f.cwd).includes(`probe${extension}`),
      extension,
    );
    rmSync(join(f.cwd, `probe${extension}`));
    assert.throws(
      () => stagedFormatFiles(f.cwd),
      (error) => {
        // Only the formattable path fails; the excluded sibling is skipped.
        assert.ok(
          error.message.includes(`cannot be formatted: probe${extension}.`),
          error.message,
        );
        assert.ok(!error.message.includes("vendor/"), error.message);
        return true;
      },
      extension,
    );
    f.git("checkout-index", "--force", "--", `probe${extension}`);
  }
});

test(
  "staged selection applies every exemption before the missing-file failure, like the unstaged selectors",
  { skip: process.platform === "win32" },
  (t) => {
    const f = fixture(t);
    f.write("target.ts", "const value = 1;\n");
    f.write("renamed.ts", "const value = 2;\n");
    f.write("removed.ts", "const value = 3;\n");
    f.write("pnpm-lock.yaml", "a:   1\n");
    const head = f.commit();
    symlinkSync("target.ts", join(f.cwd, "linked.ts"));
    f.write("target.ts", "const   value = 5;\n");
    f.write("notes.txt", "plain\n");
    f.write("vendor/lib.ts", "const   value = 4;\n");
    f.write("pnpm-lock.yaml", "a:    2\n");
    f.git("mv", "renamed.ts", "vendor/renamed.ts");
    f.git("rm", "--quiet", "removed.ts");
    f.git("add", ".");
    for (const name of [
      "linked.ts",
      "notes.txt",
      "vendor/lib.ts",
      "vendor/renamed.ts",
      "pnpm-lock.yaml",
    ]) {
      rmSync(join(f.cwd, name), { force: true });
    }
    // Symlink, unsupported extension, shared exclusions, rename into an excluded
    // directory and a staged deletion are all exempt although nothing is on disk.
    assert.deepEqual(stagedFormatFiles(f.cwd), ["target.ts"]);
    // The unstaged selectors treat the same missing paths identically.
    assert.ok(!allFormatFiles(f.cwd).includes("vendor/lib.ts"));
    assert.deepEqual(changedFormatFiles(f.cwd, head, head, true), [
      "target.ts",
    ]);
    // Only a missing path that Rstack would format fails.
    rmSync(join(f.cwd, "target.ts"));
    assert.throws(
      () => stagedFormatFiles(f.cwd),
      /missing from the working tree.*target\.ts/u,
    );
    assert.deepEqual(changedFormatFiles(f.cwd, head, head, true), []);
  },
);

test("CLI --staged accepts a staged excluded file missing from the worktree", (t) => {
  const f = fixture(t);
  const script = join(root, "scripts/ci/format-changed.mjs");
  f.write("pnpm-lock.yaml", "a:   1\n");
  f.git("add", "pnpm-lock.yaml");
  rmSync(join(f.cwd, "pnpm-lock.yaml"));
  for (const args of [["--staged"], ["--staged", "--write"]]) {
    const result = spawnSync(process.execPath, [script, ...args], {
      cwd: f.cwd,
      env: f.env,
      encoding: "utf8",
    });
    const output = `${args.join(" ")}: ${result.stdout}${result.stderr}`;
    assert.equal(result.status, 0, output);
    assert.doesNotMatch(result.stderr, /ci_format_error/u, output);
    assert.match(result.stdout, /"candidates":0/u, output);
  }
});

test("CLI --staged exits 2 naming a staged file removed from the worktree", (t) => {
  const f = fixture(t);
  const script = join(root, "scripts/ci/format-changed.mjs");
  f.write("b.ts", "const   value = 1;\n");
  f.git("add", "b.ts");
  rmSync(join(f.cwd, "b.ts"));
  for (const args of [["--staged"], ["--staged", "--write"]]) {
    const result = spawnSync(process.execPath, [script, ...args], {
      cwd: f.cwd,
      env: f.env,
      encoding: "utf8",
    });
    const output = `${args.join(" ")}: ${result.stdout}${result.stderr}`;
    assert.equal(result.status, 2, output);
    assert.match(result.stderr, /ci_format_error/u, output);
    assert.match(result.stderr, /b\.ts/u, output);
  }
});

// Lefthook hides unstaged changes (including a plain `rm` after `git add`) by
// restoring the staged content before the hook runs, so the hook only sees a
// staged path missing from the worktree when Git itself omits it from the
// unstaged diff. A skip-worktree entry, as in a sparse checkout, does that.
test("pre-commit rejects a staged skip-worktree file missing from the worktree and keeps deletions working", (t) => {
  const f = fixture(t);
  f.write("tracked.ts", "const value = 1;\n");
  f.write("sparse.ts", "const value = 1;\n");
  const head = f.commit();
  f.installHook();
  const commit = () =>
    spawnSync("git", ["commit", "--quiet", "-m", "x"], {
      cwd: f.cwd,
      env: f.env,
      encoding: "utf8",
    });
  f.write("sparse.ts", "const   value = 2;\n");
  f.git("add", "sparse.ts");
  f.git("update-index", "--skip-worktree", "sparse.ts");
  rmSync(join(f.cwd, "sparse.ts"));
  const rejected = commit();
  const output = rejected.stdout + rejected.stderr;
  assert.notEqual(rejected.status, 0, output);
  assert.equal(f.git("rev-parse", "HEAD"), head);
  assert.match(output, /ci_format_error/u);
  assert.match(output, /sparse\.ts/u);
  assert.equal(f.git("show", ":sparse.ts"), "const   value = 2;");
  // Restoring the staged state of the path and deleting another file works.
  f.git("update-index", "--no-skip-worktree", "sparse.ts");
  f.git("reset", "--quiet", "--", "sparse.ts");
  f.git("checkout", "--quiet", "--", "sparse.ts");
  f.git("rm", "--quiet", "tracked.ts");
  const accepted = commit();
  assert.equal(accepted.status, 0, accepted.stdout + accepted.stderr);
  assert.notEqual(f.git("rev-parse", "HEAD"), head);
});

test("pre-commit accepts a staged skip-worktree lockfile missing from the worktree", (t) => {
  const f = fixture(t);
  f.write("pnpm-lock.yaml", "lockfileVersion: '9.0'\n");
  f.write("tracked.ts", "const value = 1;\n");
  const head = f.commit();
  f.installHook();
  f.write("pnpm-lock.yaml", "lockfileVersion:   '9.1'\n");
  f.git("add", "pnpm-lock.yaml");
  f.git("update-index", "--skip-worktree", "pnpm-lock.yaml");
  rmSync(join(f.cwd, "pnpm-lock.yaml"));
  const accepted = spawnSync("git", ["commit", "--quiet", "-m", "lockfile"], {
    cwd: f.cwd,
    env: f.env,
    encoding: "utf8",
  });
  const output = accepted.stdout + accepted.stderr;
  assert.equal(accepted.status, 0, output);
  assert.doesNotMatch(output, /ci_format_error/u);
  assert.notEqual(f.git("rev-parse", "HEAD"), head);
  // Rstack never rewrites an excluded lockfile.
  assert.equal(
    f.git("show", "HEAD:pnpm-lock.yaml"),
    "lockfileVersion:   '9.1'",
  );
});

test("real Rstack checks whole changed files without rewriting, then fixes them", async (t) => {
  const f = fixture(t);
  const names = ["space name.ts", "한글[1].ts", "-flag.ts", "!negation.ts"];
  if (process.platform !== "win32") names.push("line\nname.ts", "star*.ts");
  for (const name of names) f.write(name, "const value={answer:42}\n");
  f.write("unrelated.go", "not Go\n");
  const head = f.commit();
  const paths = changedFormatFiles(f.cwd, f.base, head);
  assert.deepEqual(paths, names.sort());
  const before = paths.map((name) => readFileSync(join(f.cwd, name), "utf8"));
  assert.equal(await runFormatter(f.cwd, paths), 1);
  assert.deepEqual(
    paths.map((name) => readFileSync(join(f.cwd, name), "utf8")),
    before,
  );
  assert.equal(await runFormatter(f.cwd, paths, "write"), 0);
  assert.equal(await runFormatter(f.cwd, paths), 0);
  // Validation checks merge/worktree bytes, not the selected head blob.
  f.write(names[0], "const broken = ;\n");
  assert.equal(await runFormatter(f.cwd, paths), 2);
});

test("deletions, renames, empty ranges, local edits and invalid comparisons", async (t) => {
  const f = fixture(t);
  f.write("old.ts", "const old = 1;\n");
  f.write("deleted.ts", "const deleted = 1;\n");
  const base = f.commit();
  renameSync(join(f.cwd, "old.ts"), join(f.cwd, "new.ts"));
  rmSync(join(f.cwd, "deleted.ts"));
  const head = f.commit();
  assert.deepEqual(changedFormatFiles(f.cwd, base, head), ["new.ts"]);
  assert.deepEqual(changedFormatFiles(f.cwd, head, head), []);
  assert.equal(await runFormatter(f.cwd, []), 0);
  f.write("local.md", "# Local\n");
  f.write("new.ts", "const changed=2\n");
  assert.deepEqual(changedFormatFiles(f.cwd, head, head, true), [
    "local.md",
    "new.ts",
  ]);
  assert.throws(() => changedFormatFiles(f.cwd, "", head), /comparison SHA/);
  assert.throws(() => changedFormatFiles(f.cwd, "a".repeat(40), head));
});

// A path staged with new content whose working tree was then restored to the
// HEAD bytes ("MM" in `git status`) differs from HEAD only in the index.
function stageThenRevert(f, name, committed, staged) {
  f.write(name, staged);
  f.git("add", name);
  f.write(name, committed);
}

test("default local selection includes a path that differs from HEAD only in the index", (t) => {
  const f = fixture(t);
  f.write("f.ts", "const value = 1;\n");
  f.write("kept.ts", "const kept = 1;\n");
  f.write("gone.ts", "const gone = 1;\n");
  const head = f.commit();
  stageThenRevert(f, "f.ts", "const value = 1;\n", "const   value = 2;\n");
  assert.equal(f.git("status", "--porcelain", "--", "f.ts"), "MM f.ts");
  assert.deepEqual(changedFormatFiles(f.cwd, head, head, true), ["f.ts"]);
  // Staged deletions, unsupported paths and unchanged files stay unselected.
  f.git("rm", "--quiet", "gone.ts");
  f.write("notes.txt", "plain\n");
  f.git("add", "notes.txt");
  assert.deepEqual(changedFormatFiles(f.cwd, head, head, true), ["f.ts"]);
});

test("default local selection still includes worktree edits and untracked paths", (t) => {
  const f = fixture(t);
  f.write("staged.ts", "const staged = 1;\n");
  f.write("edited.ts", "const edited = 1;\n");
  const head = f.commit();
  stageThenRevert(
    f,
    "staged.ts",
    "const staged = 1;\n",
    "const   staged = 2;\n",
  );
  f.write("edited.ts", "const edited = 2;\n");
  f.write("untracked.md", "# Untracked\n");
  f.write("added.ts", "const added = 1;\n");
  f.git("add", "added.ts");
  assert.deepEqual(changedFormatFiles(f.cwd, head, head, true), [
    "added.ts",
    "edited.ts",
    "staged.ts",
    "untracked.md",
  ]);
});

test("explicit comparisons exclude paths that differ from HEAD only in the index", (t) => {
  const f = fixture(t);
  f.write("f.ts", "const value = 1;\n");
  const head = f.commit();
  stageThenRevert(f, "f.ts", "const value = 1;\n", "const   value = 2;\n");
  assert.deepEqual(changedFormatFiles(f.cwd, head, head), []);
  assert.deepEqual(changedFormatFiles(f.cwd, head, head, false), []);
  const script = join(root, "scripts/ci/format-changed.mjs");
  const run = (args, env) =>
    spawnSync(process.execPath, [script, ...args], {
      cwd: f.cwd,
      env,
      encoding: "utf8",
    });
  for (const [args, env] of [
    [["--base", head, "--head", head], f.env],
    [[], { ...f.env, CI: "true", TURBO_SCM_BASE: head, TURBO_SCM_HEAD: head }],
  ]) {
    const result = run(args, env);
    assert.equal(result.status, 0, result.stdout + result.stderr);
    assert.match(result.stdout, /"candidates":0/u, result.stdout);
    assert.match(result.stdout, /"local":false/u, result.stdout);
  }
});

test("--staged selection keeps reporting an index-only path", (t) => {
  const f = fixture(t);
  f.write("f.ts", "const value = 1;\n");
  f.commit();
  stageThenRevert(f, "f.ts", "const value = 1;\n", "const   value = 2;\n");
  assert.deepEqual(stagedFormatFiles(f.cwd), ["f.ts"]);
});

test("CLI default local run selects an index-only path and format:check fails on unformatted bytes", (t) => {
  const f = fixture(t);
  f.write("f.ts", "const value = 1;\n");
  const head = f.commit();
  // The default local comparison needs an origin/main ref to find the merge base.
  f.git("update-ref", "refs/remotes/origin/main", head);
  const script = join(root, "scripts/ci/format-changed.mjs");
  const run = () =>
    spawnSync(process.execPath, [script], {
      cwd: f.cwd,
      env: f.env,
      encoding: "utf8",
    });
  stageThenRevert(f, "f.ts", "const value = 1;\n", "const   value = 2;\n");
  // The formatter checks working-tree bytes, not the staged blob, so formatted
  // working-tree bytes pass even though the path is now selected.
  const formatted = run();
  assert.equal(formatted.status, 0, formatted.stdout + formatted.stderr);
  assert.match(formatted.stdout, /"candidates":1/u, formatted.stdout);
  assert.match(formatted.stdout, /"local":true/u, formatted.stdout);
  // Unformatted working-tree bytes in the same MM-selected path fail the check.
  f.write("f.ts", "const   value = 3;\n");
  const unformatted = run();
  assert.equal(unformatted.status, 1, unformatted.stdout + unformatted.stderr);
  assert.match(unformatted.stdout, /"candidates":1/u, unformatted.stdout);
});

test("all-file selection skips unstaged deletions and retains existing files", async (t) => {
  const f = fixture(t);
  f.write("deleted.ts", "const deleted = 1;\n");
  f.write("retained.ts", "const retained = 1;\n");
  f.commit();
  rmSync(join(f.cwd, "deleted.ts"));
  const paths = allFormatFiles(f.cwd);
  assert.ok(!paths.includes("deleted.ts"));
  assert.ok(paths.includes("retained.ts"));
  assert.equal(await runFormatter(f.cwd, paths, "write"), 0);
  assert.equal(await runFormatter(f.cwd, paths), 0);
  assert.ok(f.git("status", "--porcelain").includes("D deleted.ts"));
});

test("Rstack owns exclusions and retains Markdown prose and embedded code", async (t) => {
  const f = fixture(t);
  const ignored = [
    "packages/example/src/gen/client.ts",
    "vendor/code.ts",
    "data/fixtures/input.json",
    "data/testdata/input.json",
    "pnpm-lock.yaml",
    "guidance.generated.json",
    "output/dist/code.ts",
  ];
  for (const name of ignored) f.write(name, "deliberately invalid {{{\n");
  f.commit();
  assert.equal(await runFormatter(f.cwd, ignored), 0);
  assert.ok(
    ignored.every(
      (name) =>
        readFileSync(join(f.cwd, name), "utf8") ===
        "deliberately invalid {{{\n",
    ),
  );
  const md =
    "# Guide\n\nA long prose line that stays on this line without being wrapped by formatting.\n\n```js\nconst x={a:1}\n```\n";
  f.write("guide.md", md);
  assert.equal(await runFormatter(f.cwd, ["guide.md"]), 0);
  assert.equal(readFileSync(join(f.cwd, "guide.md"), "utf8"), md);
});

test("authored localization JSON remains eligible for formatting", async (t) => {
  const f = fixture(t);
  const name = "apps/example-chrome-extension/public/_locales/en/messages.json";
  const source = '{"title":{"message":"Browser context"}}\n';
  f.write(name, source);
  const head = f.commit();
  const paths = changedFormatFiles(f.cwd, f.base, head);
  assert.deepEqual(paths, [name]);
  assert.equal(await runFormatter(f.cwd, paths), 1);
  assert.equal(readFileSync(join(f.cwd, name), "utf8"), source);
  assert.equal(await runFormatter(f.cwd, paths, "write"), 0);
  assert.deepEqual(
    JSON.parse(readFileSync(join(f.cwd, name), "utf8")),
    JSON.parse(source),
  );
  assert.equal(await runFormatter(f.cwd, paths), 0);
});

test("CLI rejects incomplete comparisons and CI-wide formatting", (t) => {
  const f = fixture(t);
  const script = join(root, "scripts/ci/format-changed.mjs");
  const env = { ...process.env, CI: "true" };
  delete env.TURBO_SCM_BASE;
  delete env.TURBO_SCM_HEAD;
  for (const args of [
    [],
    ["--base", f.base],
    ["--all", "--base", f.base, "--head", f.base],
    ["--staged"],
  ]) {
    const result = spawnSync(process.execPath, [script, ...args], {
      cwd: f.cwd,
      env,
      encoding: "utf8",
    });
    assert.equal(result.status, 2);
    assert.match(result.stderr, /ci_format_error/);
  }
});

test("CLI rejects staged selection with other selection modes", (t) => {
  const f = fixture(t);
  const script = join(root, "scripts/ci/format-changed.mjs");
  for (const args of [["--all"], ["--base", f.base, "--head", f.base]]) {
    const result = spawnSync(process.execPath, [script, "--staged", ...args], {
      cwd: f.cwd,
      env: f.env,
      encoding: "utf8",
    });
    assert.equal(result.status, 2);
    assert.match(result.stderr, /Staged formatting requires local execution/);
  }
  const result = spawnSync(process.execPath, [script, "--staged"], {
    cwd: f.cwd,
    env: { ...f.env, TURBO_SCM_BASE: f.base, TURBO_SCM_HEAD: f.base },
    encoding: "utf8",
  });
  assert.equal(result.status, 2);
});

test("CI is enabled unless unset, empty or an explicit false", () => {
  for (const value of [
    undefined,
    "",
    "  ",
    "false",
    "FALSE",
    "False",
    " false ",
  ])
    assert.equal(ciEnabled({ CI: value }), false, JSON.stringify(value));
  for (const value of ["true", "TRUE", "1", "0", "no", "falsey"])
    assert.equal(ciEnabled({ CI: value }), true, value);
  assert.equal(ciEnabled({}), false);
});

test("CLI treats an explicit false CI value as local staged execution", (t) => {
  const f = fixture(t);
  const script = join(root, "scripts/ci/format-changed.mjs");
  for (const ci of ["false", "FALSE", ""]) {
    const empty = spawnSync(process.execPath, [script, "--staged"], {
      cwd: f.cwd,
      env: { ...f.env, CI: ci },
      encoding: "utf8",
    });
    assert.equal(empty.status, 0, `${JSON.stringify(ci)}: ${empty.stderr}`);
    assert.match(empty.stdout, /"event":"ci_format_result"/);
    assert.doesNotMatch(empty.stderr, /ci_format_error/);
  }
  f.write("staged.ts", "const value={answer:42}\n");
  f.git("add", "staged.ts");
  const result = spawnSync(process.execPath, [script, "--staged", "--write"], {
    cwd: f.cwd,
    env: { ...f.env, CI: "false" },
    encoding: "utf8",
  });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /"candidates":1/);
  assert.equal(
    readFileSync(join(f.cwd, "staged.ts"), "utf8"),
    "const value = { answer: 42 };\n",
  );
});

test("CLI keeps staged rejections when CI is explicitly false", (t) => {
  const f = fixture(t);
  const script = join(root, "scripts/ci/format-changed.mjs");
  const env = { ...f.env, CI: "false" };
  for (const args of [["--all"], ["--base", f.base, "--head", f.base]]) {
    const result = spawnSync(process.execPath, [script, "--staged", ...args], {
      cwd: f.cwd,
      env,
      encoding: "utf8",
    });
    assert.equal(result.status, 2);
    assert.match(result.stderr, /Staged formatting requires local execution/);
  }
  const result = spawnSync(process.execPath, [script, "--staged"], {
    cwd: f.cwd,
    env: { ...env, TURBO_SCM_BASE: f.base, TURBO_SCM_HEAD: f.base },
    encoding: "utf8",
  });
  assert.equal(result.status, 2);
  assert.match(result.stderr, /Staged formatting requires local execution/);
});

test("CLI keeps CI restrictions for non-false CI values", (t) => {
  const f = fixture(t);
  const script = join(root, "scripts/ci/format-changed.mjs");
  for (const ci of ["true", "TRUE", "1", "0"]) {
    const env = { ...f.env, CI: ci };
    for (const [args, message] of [
      [["--staged"], /Staged formatting requires local execution/],
      [[], /CI formatting requires the planner comparison/],
      [["--all"], /CI formatting requires the planner comparison/],
      [
        ["--all", "--base", f.base, "--head", f.base],
        /CI formatting must use changed files/,
      ],
    ]) {
      const result = spawnSync(process.execPath, [script, ...args], {
        cwd: f.cwd,
        env,
        encoding: "utf8",
      });
      assert.equal(result.status, 2, `${ci} ${args.join(" ")}`);
      assert.match(result.stderr, message);
    }
  }
});

test("explicit false CI behaves exactly like unset CI for non-staged runs", (t) => {
  const f = fixture(t);
  const script = join(root, "scripts/ci/format-changed.mjs");
  for (const args of [["--all"], ["--base", f.base, "--head", f.base], []]) {
    const run = (env) =>
      spawnSync(process.execPath, [script, ...args], {
        cwd: f.cwd,
        env,
        encoding: "utf8",
      });
    const unset = run(f.env);
    const disabled = run({ ...f.env, CI: "false" });
    assert.equal(disabled.status, unset.status, args.join(" "));
    // Rstack reports wall-clock duration; separate runs need not take equal time.
    const normalizeTiming = (output) =>
      output.replace(/\b\d+(?:\.\d+)?(?:ms|s)\b/gu, "<duration>");
    assert.equal(
      normalizeTiming(disabled.stdout),
      normalizeTiming(unset.stdout),
      args.join(" "),
    );
    assert.equal(disabled.stderr, unset.stderr, args.join(" "));
    assert.doesNotMatch(disabled.stderr, /CI formatting/);
  }
  const comparison = spawnSync(
    process.execPath,
    [script, "--base", f.base, "--head", f.base],
    { cwd: f.cwd, env: { ...f.env, CI: "false" }, encoding: "utf8" },
  );
  assert.equal(comparison.status, 0, comparison.stderr);
});

// Parent-path breakage: a tracked `a/b.ts` whose parent `a` became a regular
// file (ENOTDIR) or a self-referencing symlink (ELOOP). Git accepts both.
const brokenParents = [
  {
    name: "ENOTDIR",
    skip: false,
    breakParent: (f) => {
      rmSync(join(f.cwd, "a"), { recursive: true });
      f.write("a", "not a directory\n");
    },
  },
  {
    name: "ELOOP",
    skip: process.platform === "win32",
    breakParent: (f) => {
      rmSync(join(f.cwd, "a"), { recursive: true });
      symlinkSync("a", join(f.cwd, "a"));
    },
  },
];

for (const { name, skip, breakParent } of brokenParents) {
  const setup = (t) => {
    const f = fixture(t);
    f.write("a/b.ts", "const value = 1;\n");
    const head = f.commit();
    f.write("a/b.ts", "const value = 2;\n");
    f.git("add", "a/b.ts");
    breakParent(f);
    return { f, head };
  };

  test(
    `unstaged selectors skip tracked paths whose parent raises ${name}; staged selection fails`,
    { skip },
    (t) => {
      const { f, head } = setup(t);
      assert.doesNotMatch(allFormatFiles(f.cwd).join("\n"), /^a\/b\.ts$/mu);
      assert.deepEqual(changedFormatFiles(f.cwd, f.base, head, true), []);
      // A staged blob that cannot be formatted must block the commit.
      assert.throws(
        () => stagedFormatFiles(f.cwd),
        new RegExp(`${name}.*a/b\\.ts`, "u"),
      );
    },
  );

  test(
    `non-local comparison still throws when a parent raises ${name}`,
    { skip },
    (t) => {
      const { f, head } = setup(t);
      assert.throws(
        () => changedFormatFiles(f.cwd, f.base, head),
        new RegExp(name),
      );
    },
  );

  test(
    `--all does not crash and --staged --write exits 2 naming the path when a parent raises ${name}`,
    { skip },
    (t) => {
      const { f } = setup(t);
      const script = join(root, "scripts/ci/format-changed.mjs");
      const run = (args) =>
        spawnSync(process.execPath, [script, ...args], {
          cwd: f.cwd,
          env: f.env,
          encoding: "utf8",
        });
      // `--all` may exit 1 for unrelated fixture files; 2 is the crash code.
      const all = run(["--all"]);
      const allOutput = `--all: ${all.stdout}${all.stderr}`;
      assert.notEqual(all.status, 2, allOutput);
      assert.doesNotMatch(allOutput, /ci_format_error/u);
      const staged = run(["--staged", "--write"]);
      const stagedOutput = `--staged --write: ${staged.stdout}${staged.stderr}`;
      assert.equal(staged.status, 2, stagedOutput);
      assert.match(staged.stderr, /ci_format_error/u, stagedOutput);
      assert.match(staged.stderr, /a\/b\.ts/u, stagedOutput);
    },
  );
}

test(
  "selectors keep unrelated lstat errors fatal",
  { skip: process.platform === "win32" || process.getuid?.() === 0 },
  (t) => {
    const f = fixture(t);
    f.write("a/b.ts", "const value = 1;\n");
    const head = f.commit();
    f.git("add", "a/b.ts");
    chmodSync(join(f.cwd, "a"), 0o000);
    try {
      assert.throws(() => allFormatFiles(f.cwd), /EACCES/);
      assert.throws(
        () => changedFormatFiles(f.cwd, f.base, head, true),
        /EACCES/,
      );
    } finally {
      chmodSync(join(f.cwd, "a"), 0o755);
    }
  },
);

test("root package.json declares the ES module type used by rstack.config.ts", () => {
  // The fixtures write their own package.json, so they cannot show the Node
  // MODULE_TYPELESS_PACKAGE_JSON warning that a typeless root package causes.
  const manifest = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  assert.equal(manifest.type, "module");
});

test("formatter CLI runs main through a symlinked script path", (t) => {
  const f = fixture(t);
  f.write("unformatted.ts", "const value={answer:42}\n");
  f.git("add", "unformatted.ts");
  const links = mkdtempSync(join(tmpdir(), "rstack-format-link-"));
  t.after(() => rmSync(links, { recursive: true, force: true }));
  symlinkSync(
    join(root, "scripts/ci"),
    join(links, "ci"),
    process.platform === "win32" ? "junction" : "dir",
  );
  for (const script of [
    join(root, "scripts/ci/format-changed.mjs"),
    join(links, "ci/format-changed.mjs"),
  ]) {
    const result = spawnSync(process.execPath, [script, "--staged"], {
      cwd: f.cwd,
      env: f.env,
      encoding: "utf8",
    });
    assert.match(result.stderr + result.stdout, /"event":"ci_format/, script);
  }
});

// Stand-in Rstack for process-boundary tests: records its pid per batch and
// every termination signal it receives, then either exits with STANDIN_EXIT or
// sleeps long enough for the test to signal the parent.
const standIn = `// Test stand-in for the Rstack entry point.
const fs = require("node:fs");
const dir = process.env.STANDIN_DIR;
fs.appendFileSync(dir + "/batches.log", process.pid + "\\n");
if (process.env.STANDIN_EXIT) process.exit(Number(process.env.STANDIN_EXIT));
for (const signal of ["SIGTERM", "SIGINT", "SIGHUP"])
  process.on(signal, () => {
    fs.appendFileSync(dir + "/signals.log", process.pid + " " + signal + "\\n");
    process.exit(0);
  });
setTimeout(() => {
  fs.appendFileSync(dir + "/marker", process.pid + "\\n");
  process.exit(0);
}, 5000);
`;

// An isolated repository with its own copy of the script and a stand-in
// node_modules/rstack; the standard fixture symlinks the real node_modules.
function standInFixture(t, fileCount) {
  const f = fixture(t, false);
  rmSync(join(f.cwd, "node_modules"), { recursive: true });
  f.write(
    "node_modules/rstack/package.json",
    '{"name":"rstack","version":"0.0.0","bin":{"rs":"./bin/rs.js"}}\n',
  );
  f.write("node_modules/rstack/bin/rs.js", standIn);
  f.write(
    "scripts/ci/format-changed.mjs",
    readFileSync(join(root, "scripts/ci/format-changed.mjs"), "utf8"),
  );
  for (let index = 0; index < fileCount; index += 1)
    f.write(`files/f${index}.ts`, `export const value${index} = 1;\n`);
  f.commit();
  const state = join(f.cwd, "state");
  mkdirSync(state);
  const lines = (name) => {
    try {
      return readFileSync(join(state, name), "utf8")
        .split("\n")
        .filter(Boolean);
    } catch {
      return [];
    }
  };
  return { f, state, lines };
}

async function waitFor(predicate, what) {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    if (predicate()) return;
    await new Promise((done) => setTimeout(done, 50));
  }
  assert.fail(`Timed out waiting for ${what}`);
}

function isAlive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error.code !== "ESRCH";
  }
}

const signalCases = [
  { signal: "SIGTERM", exitCode: 143 },
  { signal: "SIGINT", exitCode: 130 },
  { signal: "SIGHUP", exitCode: 129 },
];

for (const { signal, exitCode } of signalCases) {
  test(
    `${signal} to the formatter is forwarded to the Rstack child and stops further batches`,
    { skip: process.platform === "win32" },
    async (t) => {
      // 120 tracked files plus the fixture files are more than two 50-path batches.
      const { f, state, lines } = standInFixture(t, 120);
      const parent = spawn(
        process.execPath,
        [join(f.cwd, "scripts/ci/format-changed.mjs"), "--all"],
        {
          cwd: f.cwd,
          env: { ...f.env, STANDIN_DIR: state },
          stdio: ["ignore", "ignore", "pipe"],
        },
      );
      t.after(() => parent.kill("SIGKILL"));
      let stderr = "";
      parent.stderr.on("data", (chunk) => (stderr += chunk));
      const closed = new Promise((done) =>
        parent.once("close", (code, closeSignal) =>
          done({ code, closeSignal }),
        ),
      );
      await waitFor(() => lines("batches.log").length > 0, "the first batch");
      const child = Number(lines("batches.log")[0]);
      parent.kill(signal);
      const { code, closeSignal } = await closed;
      assert.equal(closeSignal, null, "the parent must exit by itself");
      assert.equal(code, exitCode);
      assert.deepEqual(lines("signals.log"), [`${child} ${signal}`]);
      assert.equal(isAlive(child), false, "the child must be dead");
      assert.equal(lines("batches.log").length, 1, "no further batch starts");
      assert.deepEqual(
        lines("marker"),
        [],
        "the child must not run to the end",
      );
      assert.match(
        stderr,
        new RegExp(`"event":"ci_format_signal","signal":"${signal}"`),
      );
    },
  );
}

for (const status of [0, 1, 2]) {
  test(`formatter exits ${status} when every Rstack batch exits ${status}`, async (t) => {
    const { f, state, lines } = standInFixture(t, 60);
    const result = spawnSync(
      process.execPath,
      [join(f.cwd, "scripts/ci/format-changed.mjs"), "--all"],
      {
        cwd: f.cwd,
        env: { ...f.env, STANDIN_DIR: state, STANDIN_EXIT: String(status) },
        encoding: "utf8",
      },
    );
    assert.equal(result.status, status, result.stderr);
    assert.ok(lines("batches.log").length > 1, "every batch runs");
    assert.match(result.stdout, new RegExp(`"status":${status}`));
  });
}
