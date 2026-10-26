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
export function prepare({ directory, runId, loadRecords }) {
  assert.match(runId, /^[1-9]\d*$/);
  assert.equal(
    git(directory, "status", "--porcelain"),
    "",
    "Release requires a clean checkout",
  );
  git(directory, "fetch", "origin", "main", "--tags");
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
  assert.ok(records.length, "No pending changesets");
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
    ...selection,
  };
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
  });
  const outputs = {
    revision: plan.revision,
    components: JSON.stringify(plan.components),
    servers: JSON.stringify(plan.servers),
  };
  for (const [key, value] of Object.entries(outputs))
    appendFileSync(process.env.GITHUB_OUTPUT, `${key}=${value}\n`);
  appendFileSync(
    process.env.GITHUB_STEP_SUMMARY,
    `Release source: ${plan.revision}\n\nComponents: ${plan.components.map((c) => c.tag).join(", ")}\n\nServer deployment is mocked; image publication and frontend deployment remain real.\n`,
  );
}
