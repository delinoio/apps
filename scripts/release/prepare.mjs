// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  appendFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { makePlan, compareVersion, Component } from "./plan.mjs";
import { synchronizeVersions } from "./versions.mjs";
const run = (cwd, command, args) =>
  execFileSync(command, args, { cwd, encoding: "utf8" }).trim();
const git = (cwd, ...args) => run(cwd, "git", args);
const planPath = ".release/plan.json";
export function prepare({
  directory,
  runId,
  recovery = "",
  options = {},
  loadRecords,
}) {
  assert.match(runId, /^[1-9]\d*$/);
  assert.equal(
    git(directory, "status", "--porcelain"),
    "",
    "Release requires a clean checkout",
  );
  git(directory, "fetch", "origin", "main", "--tags");
  const marker = `Release-Run: ${runId}`;
  const matches = git(
    directory,
    "log",
    "origin/main",
    "--format=%H",
    "--extended-regexp",
    `--grep=^${marker}$`,
  )
    .split("\n")
    .filter(Boolean);
  assert.ok(matches.length <= 1, "Ambiguous release history");
  const revision = recovery || matches[0];
  if (revision) {
    assert.match(revision, /^[a-f0-9]{40}$/);
    git(directory, "merge-base", "--is-ancestor", revision, "origin/main");
    const plan = JSON.parse(git(directory, "show", `${revision}:${planPath}`));
    assert.equal(plan.schema, 1);
    assert.equal(git(directory, "rev-parse", `${revision}^`), plan.base);
    assert.ok(
      git(directory, "show", "-s", "--format=%B", revision)
        .split("\n")
        .includes(`Release-Run: ${plan.runId}`),
    );
    const newer = git(
      directory,
      "log",
      `${revision}..origin/main`,
      "--format=%H",
      "--",
      planPath,
    )
      .split("\n")
      .filter(Boolean);
    for (const source of newer) {
      const later = JSON.parse(git(directory, "show", `${source}:${planPath}`));
      assert.ok(
        !later.components.some((entry) =>
          plan.components.some((old) => old.component === entry.component),
        ) && !later.servers.some((server) => plan.servers.includes(server)),
        "A newer release supersedes these targets; use its revision or a fix-forward changeset",
      );
    }
    // Replay the recorded plan, including channel options, even after main advances.
    return { ...plan, revision };
  }
  assert.equal(
    git(directory, "rev-parse", "HEAD"),
    git(directory, "rev-parse", "origin/main"),
    "Main advanced; start a new run",
  );
  const read = (file) => readFileSync(resolve(directory, file), "utf8");
  const { records, inventory } = loadRecords
    ? loadRecords()
    : {
        records: JSON.parse(
          run(directory, "go", ["run", "./tools/changeset", "status"]),
        ),
        inventory: JSON.parse(
          run(directory, "go", ["run", "./tools/changeset", "packages"]),
        ),
      };
  assert.ok(
    records.length,
    "No pending changesets; recover an existing release by its revision",
  );
  const versions = existsSync(resolve(directory, planPath))
    ? JSON.parse(read(planPath)).versions
    : {};
  for (const component of Object.values(Component))
    for (const tag of git(directory, "tag", "--list", `${component}@v*`).split(
      "\n",
    )) {
      const match = tag.match(
        new RegExp(
          `^${component}@v((?:0|[1-9]\\d*)\\.(?:0|[1-9]\\d*)\\.(?:0|[1-9]\\d*))$`,
        ),
      );
      if (
        match &&
        (!versions[component] ||
          compareVersion(match[1], versions[component]) > 0)
      )
        versions[component] = match[1];
    }
  const { changes, ...selection } = makePlan(
    records,
    inventory,
    read,
    versions,
  );
  synchronizeVersions(changes, read, selection.components);
  const plan = {
    schema: 1,
    runId,
    base: git(directory, "rev-parse", "HEAD"),
    options,
    ...selection,
  };
  if (selection.components.some((c) => c.mobile))
    assert.match(
      options.mobileBuildNumber ?? "",
      /^[1-9]\d{0,3}$/,
      "DeliTools requires a store build number (1..9999)",
    );
  for (const [file, content] of Object.entries(changes))
    writeFileSync(resolve(directory, file), content);
  mkdirSync(resolve(directory, ".release"), { recursive: true });
  writeFileSync(
    resolve(directory, planPath),
    JSON.stringify(plan, null, 2) + "\n",
  );
  for (const id of plan.records) {
    assert.match(id, /^[a-z0-9]+(?:-[a-z0-9]+)*$/);
    unlinkSync(resolve(directory, `.changeset/${id}.md`));
  }
  git(
    directory,
    "add",
    "--",
    planPath,
    ...Object.keys(changes),
    ...plan.records.map((id) => `.changeset/${id}.md`),
  );
  git(
    directory,
    "commit",
    "-m",
    "chore(release): consume changesets and prepare selected components",
    "-m",
    marker,
  );
  const committed = git(directory, "rev-parse", "HEAD");
  // Never force or rebase a release: every published artifact must use this commit.
  git(directory, "push", "origin", "HEAD:refs/heads/main");
  return { ...plan, revision: committed };
}
if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  assert.equal(process.env.GITHUB_REPOSITORY, "delinoio/apps");
  assert.equal(process.env.GITHUB_REF, "refs/heads/main");
  const plan = prepare({
    directory: process.cwd(),
    runId: process.env.GITHUB_RUN_ID,
    recovery: process.env.RELEASE_REVISION,
    options: {
      mobileBuildNumber: process.env.MOBILE_BUILD_NUMBER ?? "",
      mobilePlatform: process.env.MOBILE_PLATFORM ?? "all",
      mobilePublish: process.env.MOBILE_PUBLISH === "true",
    },
  });
  const outputs = {
    revision: plan.revision,
    components: JSON.stringify(plan.components),
    servers: JSON.stringify(plan.servers),
    options: JSON.stringify(plan.options),
  };
  for (const [key, value] of Object.entries(outputs))
    appendFileSync(process.env.GITHUB_OUTPUT, `${key}=${value}\n`);
  appendFileSync(
    process.env.GITHUB_STEP_SUMMARY,
    `Release source: ${plan.revision}\n\nComponents: ${plan.components.map((c) => c.tag).join(", ")}\n\nServer deployment is mocked; image publication and frontend deployment remain real.\n`,
  );
}
