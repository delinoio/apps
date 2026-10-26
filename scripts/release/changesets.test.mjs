// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import yaml from "js-yaml";
import {
  apps,
  servers,
  planRelease,
  prepareChangesets,
} from "./changesets.mjs";
const inventory = Object.fromEntries([
  ...Object.keys(apps).map((id) => [`@apps/${id}`, `apps/${id}/package.json`]),
  ...servers.map((id) => [`@servers/${id}`, `servers/${id}/package.json`]),
  ["@packages/ui", "packages/ui/package.json"],
]);
const read = (file) =>
  `${JSON.stringify({ name: Object.keys(inventory).find((name) => inventory[name] === file), version: "1.2.3" }, null, 2)}\n`;
const record = (packages, id = "example") => ({
  id,
  packages,
  summary: "Internal detail",
  public: "Public note",
});
for (const app of Object.keys(apps).filter((id) => id !== "delitools")) {
  test(`${app} has an independent version and its API publication closure`, () => {
    const plan = planRelease(
      [record({ [`@apps/${app}`]: "minor" })],
      inventory,
      read,
    );
    assert.deepEqual(Object.keys(plan.changes), [`apps/${app}/package.json`]);
    assert.equal(
      JSON.parse(plan.changes[`apps/${app}/package.json`]).version,
      "1.3.0",
    );
    assert.deepEqual(
      plan.web.map(({ app }) => app),
      [app],
    );
    assert.deepEqual(plan.servers, apps[app][2]);
    assert.equal(plan.core, apps[app][2].length > 0);
    assert.equal(plan.records[0].summary, undefined);
    assert.equal(plan.records[0].public, "Public note");
  });
}
test("server-only release does not select or bump frontends", () => {
  const plan = planRelease(
    [record({ "@servers/lane-api": "patch" })],
    inventory,
    read,
  );
  assert.deepEqual(plan.web, []);
  assert.deepEqual(Object.keys(plan.changes), [
    "servers/lane-api/package.json",
  ]);
  assert.deepEqual(plan.servers, ["lane-api"]);
  assert.equal(plan.core, true);
});
test("highest requested bump wins and none publishes without a version change", () => {
  const plan = planRelease(
    [
      record({ "@apps/devbase": "patch" }),
      record({ "@apps/devbase": "major" }, "other"),
      record({ "@apps/lane": "none" }, "lane"),
    ],
    inventory,
    read,
  );
  assert.equal(
    JSON.parse(plan.changes["apps/devbase/package.json"]).version,
    "2.0.0",
  );
  assert.equal(plan.changes["apps/lane/package.json"], undefined);
  assert.deepEqual(
    plan.web.map(({ app }) => app),
    ["devbase", "lane"],
  );
});
test("unsupported publication fails before consuming its record", () => {
  assert.throws(
    () => planRelease([record({ "@packages/ui": "patch" })], inventory, read),
    /No release target/,
  );
  assert.throws(
    () => planRelease([record({ missing: "patch" })], inventory, read),
    /Unknown package/,
  );
});
test("DeliTools keeps native version sources synchronized", () => {
  const plan = planRelease(
    [record({ "@apps/delitools": "patch" })],
    inventory,
    (file) => readFileSync(file, "utf8"),
  );
  assert.deepEqual(Object.keys(plan.changes).sort(), [
    "Cargo.lock",
    "apps/delitools/package.json",
    "apps/delitools/src-tauri/Cargo.toml",
    "apps/delitools/src-tauri/tauri.conf.json",
  ]);
  assert.deepEqual(plan.servers, ["delitools-api"]);
});
const git = (cwd, ...args) =>
  execFileSync("git", args, {
    cwd,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  }).trim();
function fixture(t) {
  const base = mkdtempSync(join(tmpdir(), "changeset-release-"));
  t.after(() => rmSync(base, { recursive: true, force: true }));
  const remote = join(base, "remote.git");
  const directory = join(base, "checkout");
  git(base, "init", "--bare", remote);
  git(base, "init", "-b", "main", directory);
  git(directory, "config", "user.name", "Release fixture");
  git(directory, "config", "user.email", "fixture@example.invalid");
  git(directory, "config", "core.hooksPath", "/dev/null");
  for (const file of [
    "apps/devbase/package.json",
    ".changeset/example.md",
    ".changeset/README.md",
  ]) {
    mkdirSync(join(directory, file, ".."), { recursive: true });
    writeFileSync(
      join(directory, file),
      file.endsWith("package.json") ? read(file) : "fixture\n",
    );
  }
  git(directory, "add", ".");
  git(directory, "commit", "-m", "Source");
  git(directory, "remote", "add", "origin", remote);
  git(directory, "push", "origin", "main");
  return {
    directory,
    runId: "123",
    load: () => ({
      records: [record({ "@apps/devbase": "patch" })],
      inventory,
    }),
  };
}
test("one commit consumes records, persists the plan and can resume after main advances", (t) => {
  const options = fixture(t);
  const result = prepareChangesets(options);
  assert.equal(
    git(options.directory, "rev-parse", "origin/main"),
    result.revision,
  );
  assert.equal(git(options.directory, "rev-list", "--count", "HEAD"), "2");
  assert.equal(
    git(options.directory, "ls-tree", "HEAD", ".changeset/example.md"),
    "",
  );
  assert.ok(git(options.directory, "ls-tree", "HEAD", ".changeset/README.md"));
  assert.equal(git(options.directory, "tag", "--list"), "");
  writeFileSync(join(options.directory, "unrelated"), "later");
  git(options.directory, "add", "unrelated");
  git(options.directory, "commit", "-m", "Later main");
  git(options.directory, "push", "origin", "main");
  assert.deepEqual(
    prepareChangesets({
      ...options,
      load: () => {
        throw Error("Must reuse the recorded plan");
      },
    }),
    result,
  );
});
test("empty pending records do not create a commit", (t) => {
  const options = fixture(t);
  assert.equal(
    prepareChangesets({ ...options, load: () => ({ records: [], inventory }) })
      .empty,
    true,
  );
  assert.equal(git(options.directory, "rev-list", "--count", "HEAD"), "1");
});
test("a concurrent main update rejects publication without consuming remote records", (t) => {
  const options = fixture(t);
  const other = join(options.directory, "..", "other");
  const result = () =>
    prepareChangesets({
      ...options,
      load: () => {
        git(
          options.directory,
          "clone",
          "--branch",
          "main",
          join(options.directory, "..", "remote.git"),
          other,
        );
        git(other, "config", "user.name", "Concurrent fixture");
        git(other, "config", "user.email", "fixture@example.invalid");
        writeFileSync(join(other, "concurrent"), "main advanced");
        git(other, "add", "concurrent");
        git(
          other,
          "-c",
          "core.hooksPath=/dev/null",
          "commit",
          "-m",
          "Concurrent main",
        );
        git(other, "push", "origin", "main");
        return options.load();
      },
    });
  assert.throws(result, /failed/i);
  assert.ok(git(other, "ls-tree", "HEAD", ".changeset/example.md"));
  assert.equal(git(other, "ls-tree", "HEAD", ".release/manifest.json"), "");
});
test("reusable release graph orders servers before web and uses prepared SHA everywhere", () => {
  const workflow = yaml.load(
    readFileSync(".github/workflows/release.yml", "utf8"),
  );
  assert.deepEqual(Object.keys(workflow.on), ["workflow_dispatch"]);
  assert.deepEqual(workflow.jobs.servers.needs, ["prepare", "core"]);
  assert.deepEqual(workflow.jobs.web.needs, ["prepare", "core", "servers"]);
  for (const id of ["core", "servers", "web"]) {
    assert.equal(
      workflow.jobs[id].with.revision,
      "${{ needs.prepare.outputs.revision }}",
    );
    assert.ok(workflow.jobs[id].uses.endsWith(".yml"));
  }
  for (const id of ["servers", "web"]) {
    assert.match(workflow.jobs[id].if, /always\(\) && !cancelled\(\)/);
    assert.doesNotMatch(workflow.jobs[id].if, /failure/);
  }
  const web = yaml.load(
    readFileSync(".github/workflows/release-web.yml", "utf8"),
  ).jobs.deploy;
  assert.equal(web.environment, "${{ inputs.environment }}");
  assert.ok(web.steps.some((step) => step.run === "pnpm test"));
  assert.equal(web.steps[0].with.ref, "${{ inputs.revision }}");
});

test("a failed, cancelled or unexpectedly skipped selected server blocks web publication", () => {
  const workflow = yaml.load(
    readFileSync(".github/workflows/release.yml", "utf8"),
  );
  // Evaluate the small GitHub boolean expression against relevant job outcomes.
  // This exercises the actual gate rather than a separate scheduler model.
  const condition = workflow.jobs.web.if;
  function allowed({
    core = "true",
    servers = '["lane-api"]',
    coreResult = "success",
    serverResult = "success",
    cancelled = false,
  } = {}) {
    const values = {
      "needs.prepare.result": "success",
      "needs.prepare.outputs.core": core,
      "needs.prepare.outputs.servers": servers,
      "needs.prepare.outputs.web": '[{"app":"lane"}]',
      "needs.core.result": coreResult,
      "needs.servers.result": serverResult,
    };
    const expression = condition
      .replace(/always\(\)/g, "true")
      .replace(/cancelled\(\)/g, String(cancelled))
      .replace(/needs\.[a-z.]+/g, (key) => {
        assert.ok(key in values);
        return JSON.stringify(values[key]);
      });
    return Function(`"use strict"; return (${expression});`)();
  }
  assert.equal(allowed(), true);
  assert.equal(
    allowed({
      core: "false",
      servers: "[]",
      coreResult: "skipped",
      serverResult: "skipped",
    }),
    true,
  );
  for (const result of ["failure", "cancelled", "skipped"]) {
    assert.equal(allowed({ coreResult: result }), false);
    assert.equal(allowed({ serverResult: result }), false);
  }
  assert.equal(allowed({ cancelled: true }), false);
});
