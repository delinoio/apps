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
// Fixture Managed CPA manifests keep release tests independent of whether the
// console and Go server workspaces are present in this checkout.
const mcpaManifests = {
  "apps/mcpa/package.json":
    '{\n  "name": "@apps/mcpa",\n  "version": "0.1.0",\n  "private": true\n}\n',
  "servers/mcpa-api/package.json":
    '{\n  "name": "@servers/mcpa-api",\n  "private": true\n}\n',
};
const mcpaInventory = {
  ...inventory,
  "@apps/mcpa": "apps/mcpa/package.json",
  "@servers/mcpa-api": "servers/mcpa-api/package.json",
};
const mcpaRead = (file) => mcpaManifests[file] ?? read(file);
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
test("Release graph separates image preparation from authorized component publication", () => {
  const workflow = (name) => yaml.load(read(`.github/workflows/${name}.yml`));
  const release = workflow("release"),
    component = workflow("deploy-component");
  assert.deepEqual(Object.keys(release.on), ["repository_dispatch"]);
  assert.deepEqual(release.on.repository_dispatch.types, [
    "release-prepare",
    "release-publish",
  ]);
  for (const job of [release.jobs.prepare, release.jobs["publish-plan"]]) {
    assert.match(job.if, /RELEASE_DISPATCH_ACTOR_ID/);
    assert.match(job.if, /sender.type == 'Bot'/);
  }
  assert.match(release.jobs.prepare.if, /release-prepare/);
  assert.match(release.jobs["publish-plan"].if, /release-publish/);
  assert.equal(release.jobs.servers, undefined);
  assert.match(release["run-name"], /client_payload.request_id/);
  const receipt = release.jobs.prepare.steps.find(
    (step) => step.name === "Preserve preparation receipt",
  );
  assert.equal(receipt.with.name, "release-preparation");
  assert.equal(receipt.with["if-no-files-found"], "error");
  assert.equal(
    release.jobs.prepare.steps[0].name,
    "Validate the preparation request identity",
  );

  assert.equal(
    component.jobs.mobile.with.build_number,
    "${{ format('{0}', github.run_number) }}",
  );
  assert.equal(component.jobs.mobile.with.platform, "all");
  assert.equal(component.jobs.mobile.with.publish, false);
  assert.equal(release.jobs.images.needs, "prepare");
  assert.equal(release.jobs.components.needs, "publish-plan");
  assert.equal(release.jobs.components.strategy["fail-fast"], false);
  assert.deepEqual(component.jobs.complete.needs, [
    "status",
    "web",
    "lane",
    "perfine",
    "anydash",
    "mobile",
    "anydash-mobile",
  ]);
  // Each mobile workflow serves only its own component.
  assert.match(component.jobs.mobile.if, /component == 'delitools'/);
  assert.match(component.jobs["anydash-mobile"].if, /component == 'anydash'/);
  assert.match(component.jobs.anydash.if, /component == 'anydash'/);
  assert.equal(component.jobs.anydash.with.publish, true);
  assert.equal(component.jobs["anydash-mobile"].with.publish, false);
  assert.equal(
    component.jobs["anydash-mobile"].with.build_number,
    "${{ format('{0}', github.run_number) }}",
  );
  assert.match(component.jobs.complete.if, /needs.anydash.result == 'success'/);
  assert.match(
    component.jobs.complete.if,
    /needs\['anydash-mobile'\].result == 'success'/,
  );
  assert.match(component.jobs.complete.if, /needs.web.result == 'success'/);
  assert.match(component.jobs.complete.if, /cancelled/);
  for (const name of [
    "deploy-component",
    "deploy-web",
    "deploy-images",
    "deploy-lane",
    "deploy-perfine",
    "deploy-delitools-mobile",
    "deploy-anydash",
    "deploy-anydash-mobile",
  ])
    assert.deepEqual(Object.keys(workflow(name).on), ["workflow_call"]);
  const images = workflow("deploy-images").jobs.images;
  assert.equal(images.strategy["max-parallel"], undefined);
  assert.equal(images.permissions.packages, "write");
  assert.equal(workflow("deploy-web").jobs.deploy.permissions, undefined);
});

test("shared protocol records include Rust and Go server consumers outside the npm graph", () => {
  const result = makePlan(
    [{ id: "fixture", packages: { "@repo/protos": "patch" } }],
    mcpaInventory,
    mcpaRead,
  );
  assert.deepEqual(result.servers, [
    "anydash-api",
    "delcore-api",
    "delitools-api",
    "devbase-api",
    "git-tracker-api",
    "lane-api",
    "mcpa-api",
    "perfine-api",
  ]);
  assert.deepEqual(Object.keys(result.changes), ["protos/package.json"]);
});

test("Managed CPA console and Go server release as the mcpa component", () => {
  const release = (packages) =>
    makePlan([{ id: "fixture", packages }], mcpaInventory, mcpaRead);
  const server = release({ "@servers/mcpa-api": "none" });
  assert.deepEqual(server.servers, ["delcore-api", "mcpa-api"]);
  assert.deepEqual(server.components, [
    {
      component: "mcpa",
      apps: [],
      binaries: false,
      mobile: false,
      version: "0.1.1",
      tag: "mcpa@v0.1.1",
    },
  ]);
  const console = release({ "@apps/mcpa": "minor" });
  assert.deepEqual(console.servers, ["delcore-api", "mcpa-api"]);
  assert.deepEqual(console.components[0].apps, ["mcpa"]);
  assert.equal(console.components[0].tag, "mcpa@v0.2.0");
  assert.deepEqual(Object.keys(console.changes), ["apps/mcpa/package.json"]);
});

test("release credentials stay in the three repository-wide environments", () => {
  const workflow = (name) => yaml.load(read(`.github/workflows/${name}.yml`));
  const web = workflow("deploy-web").jobs.deploy;
  assert.equal(web.environment, "delino-site-production");
  assert.equal(
    workflow("deploy-perfine").jobs.publish.environment,
    "release-publish",
  );
  for (const name of ["deploy-delitools-mobile", "deploy-anydash-mobile"]) {
    const mobile = workflow(name).jobs;
    for (const platform of ["android", "ios"]) {
      assert.equal(mobile[platform].environment, "release-signing");
      assert.equal(
        mobile[`${platform}-publish`].environment,
        "release-publish",
      );
      assert.equal(mobile[`${platform}-publish`].needs, platform);
    }
  }
  const anydash = workflow("deploy-anydash").jobs;
  assert.equal(anydash.native.environment, "release-signing");
  assert.equal(anydash.assemble.environment, "release-signing");
  assert.equal(anydash.publish.environment, "release-publish");
  assert.equal(anydash.publish.if, "inputs.publish");
  assert.equal(
    anydash["cli-linux"].environment,
    undefined,
    "unsigned Linux builds hold no credentials",
  );
  for (const job of Object.values(anydash))
    if (job.permissions?.contents === "write")
      assert.equal(job, anydash.publish);
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

test("AnyDash app and CLI records publish binaries, mobile and the web host together", () => {
  const app = plan({ "@apps/anydash": "minor" });
  assert.deepEqual(app.servers, ["anydash-api", "delcore-api"]);
  assert.equal(app.components.length, 1);
  const { version, tag, ...selection } = app.components[0];
  assert.deepEqual(selection, {
    component: "anydash",
    apps: ["anydash"],
    binaries: true,
    mobile: true,
  });
  assert.equal(tag, `anydash@v${version}`);
  const cli = plan({ "@delinoio/anydash": "patch" });
  assert.deepEqual(cli.servers, []);
  // The web host serves the install scripts; mobile builds are not selected.
  assert.deepEqual(cli.components[0].apps, ["anydash"]);
  assert.equal(cli.components[0].binaries, true);
  assert.equal(cli.components[0].mobile, false);
  const server = plan({ "@servers/anydash-api": "minor" });
  assert.deepEqual(server.servers, ["anydash-api", "delcore-api"]);
  assert.deepEqual(server.components[0].apps, []);
  assert.equal(server.components[0].binaries, false);
  assert.deepEqual(Object.keys(server.changes), [
    "servers/anydash-api/package.json",
  ]);
});

test("AnyDash CLI, launcher and platform packages follow the component release version", () => {
  const result = plan(
    { "@apps/anydash": "minor", "@delinoio/anydash": "patch" },
    { anydash: "3.1.0" },
  );
  assert.equal(result.components[0].version, "3.2.0");
  synchronizeVersions(result.changes, read, result.components);
  const launcher = JSON.parse(result.changes["packages/anydash/package.json"]);
  assert.equal(launcher.version, "3.2.0");
  assert.ok(
    Object.values(launcher.optionalDependencies).every(
      (value) => value === "3.2.0",
    ),
  );
  assert.match(
    result.changes["cmds/anydash/Cargo.toml"],
    /^version = "3\.2\.0"$/m,
  );
  assert.match(
    result.changes["Cargo.lock"],
    /name = "anydash"\nversion = "3\.2\.0"/,
  );
  // The app keeps its own package version; desktop builds embed the release version.
  assert.notEqual(
    JSON.parse(result.changes["apps/anydash/package.json"]).version,
    "3.2.0",
  );
});
