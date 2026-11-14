// SPDX-License-Identifier: Apache-2.0
import { execFileSync } from "node:child_process";
import { appendFileSync } from "node:fs";
import { changedFormatFiles } from "./format-changed.mjs";

const fixtureInputs = new Set([
  ".github/workflows/CI.yml",
  ".nvmrc",
  "package.json",
  "pnpm-lock.yaml",
  "pnpm-workspace.yaml",
  "rstack.config.ts",
  "lefthook.yml",
  "scripts/install-git-hooks.sh",
]);

// Shared runtime and workspace inputs that change every app e2e suite.
const e2eInputs = new Set([
  ".github/workflows/CI.yml",
  ".nvmrc",
  "package.json",
  "pnpm-lock.yaml",
  "pnpm-workspace.yaml",
  "turbo.json",
]);

// Paths inside an app package select the app e2e suites.
export function isE2eInput(path) {
  return e2eInputs.has(path) || /^apps\/[^/]+\//.test(path);
}

// The root and every workspace `turbo.json` configure the tasks that the
// Turbo fixtures inspect.
export function isTurboConfigInput(path) {
  return path === "turbo.json" || path.endsWith("/turbo.json");
}

// A package manifest directly inside a workspace package selects the Turbo
// fixtures, which resolve every workspace package. Keep these roots in sync
// with `pnpm-workspace.yaml`; plan.test.mjs cross-checks them.
export const workspaceRoots = ["apps", "packages", "servers"];

export function isWorkspaceManifest(path) {
  return new RegExp(
    `^(${workspaceRoots.join("|")})/[^/]+/package\\.json$`,
  ).test(path);
}

// Go sources and module files select the repository Go checks.
export function isGoInput(path) {
  return path.endsWith(".go") || /(^|\/)go\.(mod|sum)$/.test(path);
}

export function plan(cwd, base, head) {
  // Reuse the formatter's revision validation and literal-path selection.
  const format = changedFormatFiles(cwd, base, head).length > 0;
  const paths = execFileSync(
    "git",
    ["diff", "--name-only", "--no-renames", "-z", base, head, "--"],
    // Match the formatter's limit so large comparisons do not hit ENOBUFS.
    { cwd, encoding: "utf8", maxBuffer: 32 * 1024 * 1024 },
  ).split("\0");
  const fixtures = paths.some(
    (path) =>
      fixtureInputs.has(path) ||
      path.startsWith("scripts/ci/") ||
      isTurboConfigInput(path) ||
      isWorkspaceManifest(path),
  );
  const go = paths.some(isGoInput);
  const e2e = paths.some(isE2eInput);
  return { quality: format || fixtures, format, fixtures, go, e2e };
}

export function main(env = process.env, cwd = process.cwd()) {
  const result = plan(cwd, env.CI_BASE, env.CI_HEAD);
  const record = {
    event: "ci_plan",
    validationRevision: execFileSync("git", ["rev-parse", "HEAD"], {
      cwd,
      encoding: "utf8",
    }).trim(),
    base: env.CI_BASE,
    head: env.CI_HEAD,
    ...result,
  };
  console.log(JSON.stringify(record));
  appendFileSync(
    env.GITHUB_OUTPUT,
    Object.entries(result)
      .map(([key, value]) => `${key}=${value}\n`)
      .join(""),
  );
  appendFileSync(
    env.GITHUB_STEP_SUMMARY,
    `CI plan: ${JSON.stringify(record)}\n`,
  );
}

// import.meta.main needs Node 24.2+; it also holds for symlinked script paths.
if (import.meta.main) {
  main();
}
