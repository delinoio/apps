// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import test from "node:test";
import yaml from "js-yaml";
import { bumpVersion } from "./project.mjs";
import { makePlan } from "./plan.mjs";
import { synchronizeVersions } from "./versions.mjs";
const read = (file) =>
  readFileSync(new URL(`../../${file}`, import.meta.url), "utf8");
const inventory = JSON.parse(
  execFileSync("go", ["run", "./tools/changeset", "packages"], {
    encoding: "utf8",
  }),
);
const plan = (packages, versions) =>
  makePlan([{ id: "fixture", packages }], inventory, read, versions);

test("SemVer bumps reset lower components and reject overflow", () => {
  assert.equal(bumpVersion("1.2.3", "patch"), "1.2.4");
  assert.equal(bumpVersion("1.2.3", "minor"), "1.3.0");
  assert.equal(bumpVersion("1.2.3", "major"), "2.0.0");
  for (const version of ["01.2.3", "1.2.3-beta", "1.2", "2147483647.0.0"])
    assert.throws(() => bumpVersion(version, "major"));
});
test("DevBase release includes server prerequisites without bumping another app", () => {
  const result = plan({ "@apps/devbase": "minor" });
  assert.deepEqual(result.servers, ["delcore-api", "devbase-api"]);
  assert.deepEqual(Object.keys(result.changes), ["apps/devbase/package.json"]);
  assert.deepEqual(
    result.components.map((c) => c.component),
    ["devbase"],
  );
});
test("Docs and site versions are independent despite their shared delino tag", () => {
  const result = plan({ "@apps/delino-docs": "minor" }, { delino: "9.0.0" });
  assert.deepEqual(Object.keys(result.changes), [
    "apps/delino-docs/package.json",
  ]);
  assert.deepEqual(result.components[0].apps, ["delino-docs"]);
  assert.equal(result.components[0].tag, "delino@v9.1.0");
  assert.equal(
    JSON.parse(result.changes["apps/delino-docs/package.json"]).version,
    bumpVersion(
      JSON.parse(read("apps/delino-docs/package.json")).version,
      "minor",
    ),
  );
});
test("Server-only and none releases do not bump or deploy frontends", () => {
  const result = plan({ "@servers/lane-api": "none" });
  assert.deepEqual(result.changes, {});
  assert.deepEqual(result.components[0].apps, []);
  assert.equal(result.components[0].binaries, false);
  assert.deepEqual(result.servers, ["delcore-api", "lane-api"]);
});
test("Shared dependencies redeploy consuming apps without version inflation", () => {
  const result = plan({ "@packages/ui": "patch" });
  assert.deepEqual(Object.keys(result.changes), ["packages/ui/package.json"]);
  assert.ok(result.components.flatMap((c) => c.apps).includes("devbase"));
});
test("Combined changes select both frontend and independent server changes", () => {
  const result = plan({
    "@apps/devbase": "patch",
    "@servers/delitools-api": "minor",
  });
  assert.deepEqual(result.servers, [
    "delcore-api",
    "delitools-api",
    "devbase-api",
  ]);
  assert.deepEqual(
    result.components.map((c) => c.component),
    ["delitools", "devbase"],
  );
});
test("Highest bump wins once per package and consumes all records", () => {
  const result = makePlan(
    [
      { id: "one", packages: { "@apps/devbase": "patch" } },
      { id: "two", packages: { "@apps/devbase": "major" } },
    ],
    inventory,
    read,
  );
  assert.equal(
    JSON.parse(result.changes["apps/devbase/package.json"]).version,
    bumpVersion(JSON.parse(read("apps/devbase/package.json")).version, "major"),
  );
  assert.deepEqual(result.records, ["one", "two"]);
});
test("Native mirrors and independent frontend versions remain consistent", () => {
  for (const name of ["@apps/delitools", "@delinoio/lane", "perfine"]) {
    const result = plan({ [name]: "minor" });
    synchronizeVersions(result.changes, read, result.components);
    assert.ok(result.changes["Cargo.lock"]);
    if (name === "@apps/delitools")
      assert.equal(
        JSON.parse(result.changes["apps/delitools/src-tauri/tauri.conf.json"])
          .version,
        JSON.parse(result.changes["apps/delitools/package.json"]).version,
      );
    if (name === "@delinoio/lane")
      assert.equal(result.changes["apps/lane/package.json"], undefined);
    if (name === "perfine")
      assert.equal(
        JSON.parse(result.changes["packages/perfine/package.json"]).version,
        result.components[0].version,
      );
  }
});
test("Internal notes never enter public release notes", () => {
  const result = makePlan(
    [
      {
        id: "one",
        packages: { "@apps/devbase": "none" },
        summary: "private engineering context",
        public: "User note",
      },
    ],
    inventory,
    read,
  );
  assert.deepEqual(result.publicNotes, [
    { packages: ["@apps/devbase"], note: "User note" },
  ]);
});
test("Release graph enforces image publication, mock dependency order, and per-component success", () => {
  const workflow = (name) => yaml.load(read(`.github/workflows/${name}.yml`));
  const release = workflow("release"),
    component = workflow("deploy-component"),
    servers = workflow("deploy-servers");
  assert.deepEqual(Object.keys(release.on), ["workflow_dispatch"]);
  assert.equal(release.on.workflow_dispatch, null);
  assert.equal(
    component.jobs.mobile.with.build_number,
    "${{ format('{0}', github.run_number) }}",
  );
  assert.equal(component.jobs.mobile.with.platform, "all");
  assert.equal(component.jobs.mobile.with.publish, false);
  assert.deepEqual(release.jobs.servers.needs, ["prepare", "images"]);
  assert.deepEqual(release.jobs.components.needs, ["prepare", "servers"]);
  assert.equal(release.jobs.components.strategy["fail-fast"], false);
  assert.equal(servers.jobs.dependents.needs, "delcore");
  assert.deepEqual(component.jobs.complete.needs, [
    "status",
    "web",
    "lane",
    "perfine",
    "mobile",
  ]);
  assert.match(component.jobs.complete.if, /needs.web.result == 'success'/);
  assert.match(component.jobs.complete.if, /cancelled/);
  for (const name of [
    "deploy-component",
    "deploy-web",
    "deploy-images",
    "deploy-servers",
    "deploy-lane",
    "deploy-perfine",
    "deploy-delitools-mobile",
  ])
    assert.deepEqual(Object.keys(workflow(name).on), ["workflow_call"]);
  const images = workflow("deploy-images").jobs.images;
  assert.equal(images.strategy["max-parallel"], undefined);
  assert.equal(images.permissions.packages, "write");
  assert.equal(workflow("deploy-web").jobs.deploy.permissions, undefined);
});

test("shared protocol records include Rust server consumers outside the npm graph", () => {
  const result = plan({ "@repo/protos": "patch" });
  assert.deepEqual(result.servers, [
    "delcore-api",
    "delitools-api",
    "devbase-api",
    "git-tracker-api",
    "lane-api",
    "perfine-api",
  ]);
  assert.deepEqual(Object.keys(result.changes), ["protos/package.json"]);
});

test("release credentials stay in the three repository-wide environments", () => {
  const workflow = (name) => yaml.load(read(`.github/workflows/${name}.yml`));
  const web = workflow("deploy-web").jobs.deploy;
  assert.equal(web.environment, "delino-site-production");
  assert.equal(
    workflow("deploy-perfine").jobs.publish.environment,
    "release-publish",
  );
  const mobile = workflow("deploy-delitools-mobile").jobs;
  for (const platform of ["android", "ios"]) {
    assert.equal(mobile[platform].environment, "release-signing");
    assert.equal(mobile[`${platform}-publish`].environment, "release-publish");
    assert.equal(mobile[`${platform}-publish`].needs, platform);
  }
  const prepare = web.steps.find((step) => step.id === "config");
  assert.equal(
    prepare.env.CLOUDFLARE_API_TOKEN,
    "${{ secrets.CLOUDFLARE_API_TOKEN }}",
  );
  const deploy = web.steps.find((step) =>
    step.name?.startsWith("Deploy the release build"),
  );
  assert.match(deploy.with.command, /steps.config.outputs.project/);
});
