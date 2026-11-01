// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import test from "node:test";
import { validatePublication } from "./dispatch.mjs";

const revision = "a".repeat(40);
function fixture() {
  const plan = {
    schema: 1,
    base: "b".repeat(40),
    runId: "123",
    components: [{ component: "lane" }],
  };
  const run = {
    id: 123,
    repository: { full_name: "delinoio/apps" },
    path: ".github/workflows/release.yml",
    event: "repository_dispatch",
    head_branch: "main",
    status: "completed",
    conclusion: "success",
  };
  return {
    plan,
    run,
    options: {
      payload: { revision, prepare_run_id: "123" },
      git: (...args) =>
        args[0] === "show"
          ? JSON.stringify(plan)
          : args[0] === "rev-parse"
            ? plan.base
            : "",
      getRun: () => run,
    },
  };
}
test("publication uses the prepared revision rather than the dispatch head", () => {
  const { options, plan } = fixture();
  assert.deepEqual(validatePublication(options), {
    revision,
    components: plan.components,
  });
});
test("malformed, unrelated and superseded revisions cannot publish", () => {
  const { options, plan } = fixture();
  assert.throws(() =>
    validatePublication({
      ...options,
      payload: { revision: "main", prepare_run_id: "123" },
    }),
  );
  assert.throws(() =>
    validatePublication({
      ...options,
      payload: { revision, prepare_run_id: "456" },
    }),
  );
  assert.throws(() =>
    validatePublication({
      ...options,
      git: () => {
        throw new Error("not an ancestor");
      },
    }),
  );
  assert.throws(
    () =>
      validatePublication({
        ...options,
        git: (...args) =>
          args[0] === "show"
            ? JSON.stringify(
                args[1].startsWith("origin/")
                  ? { ...plan, runId: "456" }
                  : plan,
              )
            : args[0] === "rev-parse"
              ? plan.base
              : "",
      }),
    /superseded/,
  );
});
test("failed, unfinished, foreign and unrelated workflow runs cannot authorize publication", () => {
  for (const override of [
    { conclusion: "failure" },
    { conclusion: "cancelled" },
    { status: "in_progress" },
    { head_branch: "feature" },
    { path: ".github/workflows/CI.yml" },
    { event: "pull_request" },
    { repository: { full_name: "other/repo" } },
    { id: 456 },
  ]) {
    const { options, run } = fixture();
    assert.throws(() =>
      validatePublication({
        ...options,
        getRun: () => ({ ...run, ...override }),
      }),
    );
  }
});
