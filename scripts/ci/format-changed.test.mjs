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
function fixture(t, initialCommit = true) {
  const cwd = mkdtempSync(join(tmpdir(), "rstack-format-"));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  const env = { ...process.env, LEFTHOOK_BIN: lefthook };
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
  "staged selection skips symlinks and unstaged deletions",
  { skip: process.platform === "win32" },
  (t) => {
    const f = fixture(t);
    f.write("target.ts", "const value = 1;\n");
    symlinkSync("target.ts", join(f.cwd, "linked.ts"));
    f.git("add", "target.ts", "linked.ts");
    assert.deepEqual(stagedFormatFiles(f.cwd), ["target.ts"]);
    rmSync(join(f.cwd, "target.ts"));
    assert.deepEqual(stagedFormatFiles(f.cwd), []);
  },
);

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
    `selectors skip tracked paths whose parent raises ${name}`,
    { skip },
    (t) => {
      const { f, head } = setup(t);
      assert.doesNotMatch(allFormatFiles(f.cwd).join("\n"), /^a\/b\.ts$/mu);
      assert.deepEqual(stagedFormatFiles(f.cwd), []);
      assert.deepEqual(changedFormatFiles(f.cwd, f.base, head, true), []);
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
    `--staged --write and --all exit 0 when a parent raises ${name}`,
    { skip },
    (t) => {
      const { f } = setup(t);
      const script = join(root, "scripts/ci/format-changed.mjs");
      for (const args of [["--staged", "--write"], ["--all"]]) {
        const result = spawnSync(process.execPath, [script, ...args], {
          cwd: f.cwd,
          env: f.env,
          encoding: "utf8",
        });
        // `--all` may exit 1 for unrelated fixture files; 2 is the crash code.
        const output = `${args.join(" ")}: ${result.stdout}${result.stderr}`;
        assert.notEqual(result.status, 2, output);
        assert.doesNotMatch(output, /ci_format_error/u);
      }
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
// sleeps long enough for the test to signal the parent. The signal handlers are
// installed before the batch is recorded: the test signals as soon as it sees
// batches.log, and a signal that arrived earlier would kill the stand-in by the
// default action without a signals.log entry.
const standIn = `// Test stand-in for the Rstack entry point.
const fs = require("node:fs");
const dir = process.env.STANDIN_DIR;
for (const signal of ["SIGTERM", "SIGINT", "SIGHUP"])
  process.on(signal, () => {
    fs.appendFileSync(dir + "/signals.log", process.pid + " " + signal + "\\n");
    process.exit(0);
  });
fs.appendFileSync(dir + "/batches.log", process.pid + "\\n");
if (process.env.STANDIN_EXIT) process.exit(Number(process.env.STANDIN_EXIT));
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
