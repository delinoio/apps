// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { appendFileSync, readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

export function validatePublication({ payload, git, getRun }) {
  assert.match(payload.revision ?? "", /^[a-f0-9]{40}$/);
  assert.match(payload.prepare_run_id ?? "", /^[1-9]\d*$/);
  const revision = payload.revision;
  // A trusted workflow validates the target before any target code is executed.
  git("merge-base", "--is-ancestor", revision, "origin/main");
  const plan = JSON.parse(git("show", `${revision}:.release/plan.json`));
  const latest = JSON.parse(git("show", "origin/main:.release/plan.json"));
  assert.equal(plan.schema, 1);
  assert.equal(
    git("rev-parse", `${revision}^`),
    plan.base,
    "Not the prepared release commit",
  );
  assert.equal(plan.runId, payload.prepare_run_id);
  assert.deepEqual(plan, latest, "A newer release superseded this request");
  const run = getRun(plan.runId);
  assert.equal(String(run.id), plan.runId);
  assert.equal(run.repository.full_name, "delinoio/apps");
  assert.equal(run.path, ".github/workflows/release.yml");
  assert.equal(run.event, "repository_dispatch");
  assert.equal(run.head_branch, "main");
  assert.equal(run.status, "completed");
  assert.equal(run.conclusion, "success", "Image preparation must succeed");
  assert.ok(Array.isArray(plan.components));
  return { revision, components: plan.components };
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const event = JSON.parse(readFileSync(process.env.GITHUB_EVENT_PATH, "utf8"));
  assert.equal(event.action, "release-publish");
  assert.ok(process.env.RELEASE_DISPATCH_ACTOR_ID);
  assert.equal(String(event.sender.id), process.env.RELEASE_DISPATCH_ACTOR_ID);
  assert.equal(event.sender.type, "Bot");
  const result = validatePublication({
    payload: event.client_payload,
    git: (...args) => execFileSync("git", args, { encoding: "utf8" }).trim(),
    getRun: (id) =>
      JSON.parse(
        execFileSync("gh", ["api", `repos/delinoio/apps/actions/runs/${id}`], {
          encoding: "utf8",
        }),
      ),
  });
  for (const [key, value] of Object.entries(result))
    appendFileSync(
      process.env.GITHUB_OUTPUT,
      `${key}=${typeof value === "string" ? value : JSON.stringify(value)}\n`,
    );
  console.info(
    JSON.stringify({
      event: "release_publication_validated",
      revision: result.revision,
    }),
  );
}
