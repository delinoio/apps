import assert from "node:assert/strict";
import test from "node:test";
import { select } from "./plan.mjs";

test("isolated projects select only their validation", () => {
  assert.deepEqual(select(["apps/delitools/src/main.tsx"]), {
    delcore_api: false,
    git_tracker_api: false,
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
    delcore_api: true,
    git_tracker_api: true,
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
    delcore_api: true,
    git_tracker_api: true,
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
        delcore_api: false,
        git_tracker_api: false,
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
    delcore_api: false,
    git_tracker_api: false,
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
      delcore_api: false,
      git_tracker_api: false,
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

for (const [service, key] of [
  ["delcore-api", "delcore_api"],
  ["git-tracker-api", "git_tracker_api"],
]) {
  test(`${service} changes select only the owning server`, () => {
    for (const path of [
      `servers/${service}/cmd/main.go`,
      `docs/servers-${service}-contract.md`,
    ]) {
      const selection = select([path]);
      assert.equal(selection[key], true);
      assert.equal(
        selection[key === "delcore_api" ? "git_tracker_api" : "delcore_api"],
        false,
      );
      for (const frontend of [
        "fixkey",
        "devbase",
        "delino_site",
        "delino_docs",
        "delidev",
        "delitools",
      ])
        assert.equal(selection[frontend], false);
    }
  });
}
test("server-wide license and instructions select both servers", () => {
  for (const path of [
    "servers/LICENSE",
    "servers/AGENTS.md",
    "servers/NOTICE",
  ]) {
    const selection = select([path]);
    assert.equal(selection.delcore_api, true);
    assert.equal(selection.git_tracker_api, true);
    assert.equal(selection.fixkey, false);
  }
});
