// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { appendFileSync, readFileSync } from "node:fs";
const gh = (...args) => execFileSync("gh", args, { encoding: "utf8" }).trim();
const { RELEASE_REVISION: revision, RELEASE_COMPONENT: component } =
  process.env;
assert.match(revision ?? "", /^[a-f0-9]{40}$/);
const plan = JSON.parse(readFileSync(".release/plan.json", "utf8"));
const entry = plan.components.find((c) => c.component === component);
assert.ok(entry, "Component absent from release plan");
assert.match(
  entry.tag,
  /^(delino|delitools|devbase|perfine|lane)@v\d+\.\d+\.\d+$/,
);
assert.equal(
  execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim(),
  revision,
);
// Listing refs distinguishes a missing tag from an authentication/network failure.
const refs = JSON.parse(
  gh("api", `repos/delinoio/apps/git/matching-refs/tags/${entry.tag}`),
);
const existing = refs.find((ref) => ref.ref === `refs/tags/${entry.tag}`);
if (existing)
  assert.equal(
    existing.object.sha,
    revision,
    "Never move a component success tag",
  );
if (process.argv[2] === "status") {
  appendFileSync(process.env.GITHUB_OUTPUT, `tagged=${Boolean(existing)}\n`);
} else {
  if (!existing)
    gh(
      "api",
      "repos/delinoio/apps/git/refs",
      "-f",
      `ref=refs/tags/${entry.tag}`,
      "-f",
      `sha=${revision}`,
    );
  if (entry.binaries) {
    const release = JSON.parse(
      gh(
        "release",
        "view",
        entry.tag,
        "--repo",
        "delinoio/apps",
        "--json",
        "isDraft",
      ),
    );
    if (release.isDraft)
      gh(
        "release",
        "edit",
        entry.tag,
        "--repo",
        "delinoio/apps",
        "--draft=false",
      );
  }
  console.info(
    JSON.stringify({
      event: "component_release_complete",
      component,
      tag: entry.tag,
      revision,
      publicationAuthorization: "environment-approval",
    }),
  );
}
