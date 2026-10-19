import assert from "node:assert/strict";
import test from "node:test";
import { select } from "./plan.mjs";

test("isolated projects select only their validation", () => {
  assert.deepEqual(select(["apps/delitools/src/main.tsx"]), {
    fixkey: false,
    devbase: false,
    delino_site: false,
    delidev: false,
    delitools: true,
    protocol: false,
  });
  assert.deepEqual(select(["protos/delcore/v1/delcore.proto"]), {
    fixkey: false,
    devbase: false,
    delino_site: false,
    delidev: false,
    delitools: false,
    protocol: true,
  });
  assert.deepEqual(select(["pnpm-lock.yaml"]), {
    fixkey: true,
    devbase: true,
    delino_site: true,
    delidev: true,
    delitools: true,
    protocol: true,
  });
});

for (const [project, key] of [
  ["devbase", "devbase"],
  ["delino-site", "delino_site"],
]) {
  test(`${project} changes select only its frontend`, () => {
    for (const path of [
      `apps/${project}/src/main.tsx`,
      `docs/project-${project}.md`,
      `docs/apps-${project}-web-contract.md`,
    ]) {
      assert.deepEqual(select([path]), {
        fixkey: false,
        devbase: key === "devbase",
        delino_site: key === "delino_site",
        delidev: false,
        delitools: false,
        protocol: false,
      });
    }
  });
}

test("consumer examples select protocol validation without native runners", () => {
  assert.deepEqual(select(["examples/delcore-client/main.go"]), {
    fixkey: false,
    devbase: false,
    delino_site: false,
    delidev: false,
    delitools: false,
    protocol: true,
  });
});

for (const path of [
  "apps/fixkey/src/App.tsx",
  "crates/fixkey-core/src/lib.rs",
  "scripts/release/fixkey.mjs",
  ".github/workflows/release-fixkey.yml",
  "docs/project-fixkey.md",
]) {
  test(`standalone FixKey ownership: ${path}`, () => {
    assert.deepEqual(select([path]), {
      fixkey: true,
      devbase: false,
      delino_site: false,
      delidev: false,
      delitools: false,
      protocol: false,
    });
  });
}
