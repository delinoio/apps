// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import {
  mkdtempSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createRequire } from "node:module";
import {
  releaseTargets,
  expectedArchives,
  minimumEvidence,
  assemble,
  verifySet,
  assertAuthority,
} from "./release-set.mjs";
import { sha256, root } from "./build.mjs";
const sourceCommit = "a".repeat(40);
const v = "0.1.0";
const json = (path, value) => writeFileSync(path, JSON.stringify(value));
function fixture(t) {
  const directory = mkdtempSync(join(tmpdir(), "perfine-release-set-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const input = join(directory, "input");
  const output = join(directory, "output");
  for (const item of releaseTargets) {
    const dest = join(input, item.target);
    mkdirSync(join(dest, "standalone"), { recursive: true });
    const archives = {};
    for (const name of expectedArchives(v, item)) {
      // Bytes are deliberately fixtures; these tests do not execute a CLI.
      writeFileSync(join(dest, name), name);
      archives[name] = sha256(name);
    }
    writeFileSync(
      join(dest, "SHA256SUMS"),
      Object.entries(archives)
        .map(([name, hash]) => `${hash}  ${name}\n`)
        .join(""),
    );
    const common = {
      schemaVersion: 1,
      sourceCommit,
      version: v,
      target: item.target,
    };
    json(join(dest, "standalone/provenance.json"), common);
    json(join(dest, "smoke-result.json"), {
      ...common,
      platform: item.platform,
      arch: item.arch,
      status: "passed",
      archives,
    });
    const host = {
      platform: item.platform,
      arch: item.arch,
      version: item.platform === "darwin" ? "26.0" : "10.0.26100",
      libc: "2.39",
      windowsCaption: "Microsoft Windows Server 2025",
    };
    json(join(dest, "validation.json"), {
      ...common,
      host,
      minimumOs: minimumEvidence(host),
      dockerWorkflow: "unperformed",
      liveProvider: "unperformed",
    });
    if (item.platform === "linux") {
      mkdirSync(join(dest, "action"));
      writeFileSync(join(dest, "action/fixture.txt"), "fixture");
    }
  }
  return { input, output };
}
test("all four target smoke records are bound to source, version and archive bytes", (t) => {
  const { input, output } = fixture(t);
  const record = assemble(input, output, sourceCommit, v);
  assert.equal(record.npmPackages.length, 5);
  assert.equal(Object.keys(record.archives).length, 9);
  assert.deepEqual(verifySet(output), record);
  assert.equal(record.validation[3].minimumOs.status, "unverified");
  assert.throws(
    () => assemble(input, output, sourceCommit, v),
    /already exists/,
  );
});
test("missing target or failed smoke blocks preparation before output exists", (t) => {
  const { input, output } = fixture(t);
  const path = join(input, releaseTargets[3].target, "smoke-result.json");
  const record = JSON.parse(readFileSync(path));
  record.status = "failed";
  json(path, record);
  assert.throws(() => assemble(input, output, sourceCommit, v), /Every target/);
  rmSync(join(input, releaseTargets[3].target), { recursive: true });
  assert.throws(() => assemble(input, output, sourceCommit, v), /ENOENT/);
});
test("replaced archives and stale source evidence cannot enter a release", (t) => {
  const { input, output } = fixture(t);
  const dest = join(input, releaseTargets[0].target);
  const smoke = JSON.parse(readFileSync(join(dest, "smoke-result.json")));
  smoke.sourceCommit = "b".repeat(40);
  json(join(dest, "smoke-result.json"), smoke);
  assert.throws(() => assemble(input, output, sourceCommit, v));
  smoke.sourceCommit = sourceCommit;
  json(join(dest, "smoke-result.json"), smoke);
  writeFileSync(
    join(dest, expectedArchives(v, releaseTargets[0])[1]),
    "tampered",
  );
  assert.throws(
    () => assemble(input, output, sourceCommit, v),
    /checksum mismatch/,
  );
});
test("minimum OS checks never promote newer macOS or Windows Server to minimum acceptance", () => {
  assert.equal(
    minimumEvidence({ platform: "darwin", arch: "arm64", version: "15.7" })
      .status,
    "verified",
  );
  assert.equal(
    minimumEvidence({ platform: "darwin", arch: "arm64", version: "26.0" })
      .status,
    "unverified",
  );
  assert.equal(
    minimumEvidence({
      platform: "win32",
      arch: "x64",
      version: "10.0.26200",
      windowsCaption: "Microsoft Windows Server 2025",
    }).status,
    "unverified",
  );
  assert.equal(
    minimumEvidence({
      platform: "win32",
      arch: "x64",
      version: "10.0.26200",
      windowsCaption: "Microsoft Windows 11 Pro",
    }).status,
    "verified",
  );
  assert.equal(
    minimumEvidence({
      platform: "linux",
      arch: "x64",
      version: "6.8",
      libc: "2.40",
    }).status,
    "unverified",
  );
});
test("npm preflight requires namespace membership and write access to every existing name", () => {
  assert.doesNotThrow(() =>
    assertAuthority("alice", { alice: "owner" }, { perfine: "read-write" }, [
      "perfine",
    ]),
  );
  assert.throws(
    () =>
      assertAuthority("alice", { bob: "owner" }, { perfine: "read-write" }, [
        "perfine",
      ]),
    /belong/,
  );
  assert.throws(
    () =>
      assertAuthority(
        "alice",
        { alice: "developer" },
        { perfine: "read-only" },
        ["perfine"],
      ),
    /publish authority/,
  );
});
test("only owning manual release admits macOS and publication requires the complete preparation set", () => {
  const require = createRequire(import.meta.url);
  const yaml = require("js-yaml");
  const workflow = yaml.load(
    readFileSync(join(root, ".github/workflows/release-perfine.yml"), "utf8"),
  );
  assert.deepEqual(Object.keys(workflow.on), ["workflow_dispatch"]);
  assert.equal(workflow.on.workflow_dispatch.inputs.publish.default, false);
  assert.match(workflow.jobs.source.if, /refs\/heads\/main/);
  assert.equal(workflow.jobs.build.strategy["fail-fast"], false);
  assert.deepEqual(
    workflow.jobs.build.strategy.matrix.include.map((x) => x.target),
    releaseTargets.map((x) => x.target),
  );
  assert.deepEqual(workflow.jobs["prepare-set"].needs, ["source", "build"]);
  assert.deepEqual(workflow.jobs.publish.needs, [
    "source",
    "build",
    "prepare-set",
  ]);
  assert.equal(workflow.jobs.publish.environment, "perfine-production");
  const steps = workflow.jobs.publish.steps;
  const preflight = steps.findIndex((x) => x.run?.includes(" preflight "));
  const action = steps.findIndex((x) => x.run?.includes(" publish-action"));
  assert.ok(preflight >= 0 && preflight < action);
  assert.doesNotMatch(JSON.stringify(workflow), /--clobber|--force/);
});
