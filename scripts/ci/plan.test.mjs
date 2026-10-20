import assert from "node:assert/strict";
import test from "node:test";
import { select } from "./plan.mjs";

test("isolated projects select only their validation", () => {
  assert.deepEqual(select(["apps/delitools/src/main.tsx"]), {
    git_tracker: false,
    fixkey: false,
    devbase: false,
    delino_site: false,
    delino_docs: false,
    delidev: false,
    delitools: true,
    protocol: false,
  });
  assert.deepEqual(select(["protos/delcore/v1/delcore.proto"]), {
    git_tracker: true,
    fixkey: false,
    devbase: false,
    delino_site: false,
    delino_docs: false,
    delidev: false,
    delitools: false,
    protocol: true,
  });
  assert.deepEqual(select(["pnpm-lock.yaml"]), {
    git_tracker: true,
    fixkey: true,
    devbase: true,
    delino_site: true,
    delino_docs: true,
    delidev: true,
    delitools: true,
    protocol: true,
  });
});

for (const [project, key] of [
  ["devbase", "devbase"],
  ["delino-site", "delino_site"],
  ["delino-docs", "delino_docs"],
]) {
  test(`${project} changes select only its frontend`, () => {
    for (const path of [
      `apps/${project}/src/main.tsx`,
      `apps/${project}/docs/services/index.md`,
      `docs/project-${project}.md`,
      `docs/apps-${project}-web-contract.md`,
    ]) {
      assert.deepEqual(select([path]), {
        git_tracker: false,
        fixkey: false,
        devbase: key === "devbase",
        delino_site: key === "delino_site",
        delino_docs: key === "delino_docs",
        delidev: false,
        delitools: false,
        protocol: false,
      });
    }
  });
}

test("consumer examples select protocol validation without native runners", () => {
  assert.deepEqual(select(["examples/delcore-client/main.go"]), {
    git_tracker: false,
    fixkey: false,
    devbase: false,
    delino_site: false,
    delino_docs: false,
    delidev: false,
    delitools: false,
    protocol: true,
  });
});

for (const path of [
  "apps/fixkey-app/src/App.tsx",
  "crates/fixkey-core/src/lib.rs",
  "scripts/release/fixkey.mjs",
  ".github/workflows/release-fixkey.yml",
  "docs/project-fixkey.md",
]) {
  test(`standalone FixKey ownership: ${path}`, () => {
    assert.deepEqual(select([path]), {
      git_tracker: false,
      fixkey: true,
      devbase: false,
      delino_site: false,
      delino_docs: false,
      delidev: false,
      delitools: false,
      protocol: false,
    });
  });
}

test("Git Tracker clients and Action avoid unrelated runners", () => {
  for (const path of [
    "apps/git-tracker/src/main.tsx",
    "actions/git-tracker/main.mjs",
    "docs/project-git-tracker.md",
  ]) {
    const selection = select([path]);
    assert.equal(selection.git_tracker, true);
    assert.equal(selection.fixkey, false);
    assert.equal(selection.protocol, false);
  }
});
