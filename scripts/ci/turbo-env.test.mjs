// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../..", import.meta.url));
// The package entry point runs under Node directly, unlike the `.bin` shell
// shim, which needs `sed` and `dirname` on PATH.
const turbo = join(root, "node_modules", "turbo", "bin", "turbo");

// The values are deliberately invalid or non-default so a dry run can only show
// them reaching a task through its declared pass-through variables.
const shellEnv = {
  ADDR: "127.0.0.1:9123",
  LOG_LEVEL: "debug",
  PUBLIC_API_ORIGIN: "ftp://bad",
};

// Expected `passThroughEnv` of each workspace `dev` task. `null` means that the
// task declares none, so Turbo's strict mode removes every shell variable.
const expectedPassThroughEnv = {
  "accounts-api#dev": ["ADDR", "LOG_LEVEL"],
  "delino-accounts#dev": ["PUBLIC_API_ORIGIN"],
  "delino-docs#dev": null,
  "delino-site#dev": null,
};

// Workspace directories named by the patterns of `pnpm-workspace.yaml`.
function workspaceDirectories(base) {
  const manifest = readFileSync(join(base, "pnpm-workspace.yaml"), "utf8");
  const parents = [
    ...manifest.matchAll(/^\s*-\s*["']?([^*"'\s]+)\/\*["']?\s*$/gm),
  ].map((match) => match[1]);
  assert.ok(parents.length > 0, "pnpm-workspace.yaml lists no workspace globs");
  return parents.flatMap((parent) =>
    existsSync(join(base, parent))
      ? readdirSync(join(base, parent), { withFileTypes: true })
          .filter((entry) => entry.isDirectory())
          .map((entry) => `${parent}/${entry.name}`)
      : [],
  );
}

// Copy only the files that define the workspace graph and the task
// configuration, so the dry run neither needs installed dependencies nor can
// touch the checkout.
function copyTurboConfiguration(t) {
  const cwd = mkdtempSync(join(tmpdir(), "turbo-env-"));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  const copy = (path) => {
    if (!existsSync(join(root, path))) {
      return;
    }
    mkdirSync(dirname(join(cwd, path)), { recursive: true });
    copyFileSync(join(root, path), join(cwd, path));
  };
  for (const path of [
    "package.json",
    "turbo.json",
    "pnpm-workspace.yaml",
    "pnpm-lock.yaml",
  ]) {
    copy(path);
  }
  for (const directory of workspaceDirectories(root)) {
    copy(`${directory}/package.json`);
    copy(`${directory}/turbo.json`);
  }
  return cwd;
}

// Run the pinned Turbo as a dry run, which resolves task definitions without
// running any task or needing `pnpm` on PATH.
function dryRunDev(cwd) {
  const result = spawnSync(
    process.execPath,
    [turbo, "run", "dev", "--dry=json"],
    {
      cwd,
      encoding: "utf8",
      env: {
        PATH: dirname(process.execPath),
        HOME: cwd,
        ...shellEnv,
        TURBO_TELEMETRY_DISABLED: "1",
        DO_NOT_TRACK: "1",
        TURBO_NO_UPDATE_NOTIFIER: "1",
      },
    },
  );
  assert.equal(
    result.status,
    0,
    `turbo run dev --dry=json failed: ${result.stderr}${result.error ?? ""}`,
  );
  // Turbo may print warnings, such as a missing local installation, on stderr
  // or stdout, so the JSON document is read from the first opening brace.
  const start = result.stdout.indexOf("{");
  let dry;
  try {
    dry = JSON.parse(result.stdout.slice(start));
  } catch (error) {
    assert.fail(
      `turbo run dev --dry=json did not print JSON: ${error.message}\nstdout: ${result.stdout}\nstderr: ${result.stderr}`,
    );
  }
  return dry;
}

// Check the dry run and name each `dev` task that does not match the contract.
function devTaskProblems(dry) {
  const problems = [];
  if (dry.envMode !== "strict") {
    problems.push(`envMode is ${dry.envMode}, expected strict`);
  }
  const tasks = new Map(dry.tasks.map((task) => [task.taskId, task]));
  for (const [taskId, expected] of Object.entries(expectedPassThroughEnv)) {
    const definition = tasks.get(taskId)?.resolvedTaskDefinition;
    if (!definition) {
      problems.push(`${taskId} is missing from the dry run`);
      continue;
    }
    const actual = definition.passThroughEnv ?? null;
    if (JSON.stringify(actual) !== JSON.stringify(expected)) {
      problems.push(
        `${taskId} passThroughEnv is ${JSON.stringify(actual)}, expected ${JSON.stringify(expected)}`,
      );
    }
    if (definition.cache !== false || definition.persistent !== true) {
      problems.push(
        `${taskId} must stay uncached and persistent, got cache=${definition.cache} persistent=${definition.persistent}`,
      );
    }
    if ((definition.env ?? []).length > 0) {
      problems.push(
        `${taskId} must not declare env, which would enter its hash: ${JSON.stringify(definition.env)}`,
      );
    }
  }
  for (const taskId of tasks.keys()) {
    if (!(taskId in expectedPassThroughEnv)) {
      problems.push(`${taskId} is not an expected dev task`);
    }
  }
  return problems;
}

test("dev tasks pass only their documented variables through strict mode", (t) => {
  const cwd = copyTurboConfiguration(t);
  assert.deepEqual(devTaskProblems(dryRunDev(cwd)), []);
});

test("removing a pass-through entry fails and names the dev task", async (t) => {
  for (const { path, entry, taskId } of [
    {
      path: "servers/accounts-api/turbo.json",
      entry: "ADDR",
      taskId: "accounts-api#dev",
    },
    {
      path: "servers/accounts-api/turbo.json",
      entry: "LOG_LEVEL",
      taskId: "accounts-api#dev",
    },
    {
      path: "apps/delino-accounts/turbo.json",
      entry: "PUBLIC_API_ORIGIN",
      taskId: "delino-accounts#dev",
    },
  ]) {
    await t.test(`${path} without ${entry}`, (t) => {
      const cwd = copyTurboConfiguration(t);
      const file = join(cwd, path);
      const config = JSON.parse(readFileSync(file, "utf8"));
      config.tasks.dev.passThroughEnv = config.tasks.dev.passThroughEnv.filter(
        (name) => name !== entry,
      );
      writeFileSync(file, JSON.stringify(config));
      const problems = devTaskProblems(dryRunDev(cwd));
      assert.ok(
        problems.some((problem) => problem.startsWith(taskId)),
        `expected a problem for ${taskId}, got ${JSON.stringify(problems)}`,
      );
    });
  }
});
