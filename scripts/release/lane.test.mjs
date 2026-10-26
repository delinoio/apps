// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  mkdirSync,
  chmodSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, resolve } from "node:path";
import test from "node:test";
import yaml from "js-yaml";
import { assemble, publish, releaseIdentity } from "./lane.mjs";
import {
  platforms,
  verifiedExecutable,
} from "../../packages/lane/bin/verify.mjs";

const identity = { tag: "delino@v1.2.3", revision: "a".repeat(40) };
const version = "0.1.0";
const digest = (bytes) => createHash("sha256").update(bytes).digest("hex");
function write(path, bytes) {
  mkdirSync(resolve(path, ".."), { recursive: true });
  writeFileSync(path, bytes);
}
function fixture(t, options = {}) {
  const directory = mkdtempSync(resolve(tmpdir(), "lane-release-fixture-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const inputs = resolve(directory, "hosts");
  for (const [host, targets] of [
    ["linux-amd64", ["linux-amd64"]],
    ["linux-arm64", ["linux-arm64"]],
    ["darwin-amd64", ["darwin-amd64"]],
    ["darwin-arm64", ["darwin-arm64"]],
    ["windows-amd64", ["windows-amd64", "windows-arm64"]],
  ]) {
    const candidate = resolve(inputs, host);
    const pkg = resolve(directory, "sources", host, "package");
    const manifest = {
      schemaVersion: 1,
      version,
      sourceRevision:
        options.wrongRevision && host === "linux-arm64"
          ? "b".repeat(40)
          : identity.revision,
      launcherRevision: identity.revision,
      artifacts: {},
    };
    write(
      resolve(pkg, "package.json"),
      JSON.stringify({ name: "@delinoio/lane", version }),
    );
    for (const name of ["LICENSE", "NOTICE"])
      write(resolve(pkg, name), `${name} fixture\n`);
    write(
      resolve(pkg, "THIRD_PARTY_NOTICES/modules.json"),
      JSON.stringify([
        { module: host, version: "1.0.0", source: "fixture", license: "MIT" },
      ]),
    );
    for (const target of targets.filter(
      (target) => target !== options.missing,
    )) {
      const name = target.startsWith("windows") ? "lane.exe" : "lane";
      const path = `artifacts/lane-${target}/${name}`;
      const bytes = `synthetic ${target} executable; not native acceptance\n`;
      const entry = {
        platform: target,
        version,
        path,
        size: Buffer.byteLength(bytes),
        sha256: digest(bytes),
      };
      write(
        resolve(pkg, path),
        options.corrupt === target ? "corrupted" : bytes,
      );
      write(
        resolve(pkg, `artifacts/lane-${target}/manifest.json`),
        JSON.stringify({
          ...entry,
          path: name,
          sourceRevision: identity.revision,
        }),
      );
      for (const notice of [
        "LICENSE",
        "NOTICE",
        "THIRD_PARTY_NOTICES/modules.json",
      ])
        write(
          resolve(pkg, `artifacts/lane-${target}`, notice),
          readFileSync(resolve(pkg, notice)),
        );
      manifest.artifacts[target] = entry;
    }
    write(resolve(pkg, "manifest.json"), JSON.stringify(manifest));
    mkdirSync(candidate, { recursive: true });
    execFileSync("tar", [
      "-czf",
      resolve(candidate, `delinoio-lane-${version}.tgz`),
      "-C",
      resolve(pkg, ".."),
      "package",
    ]);
    if (
      (host.startsWith("darwin") || host === "windows-amd64") &&
      host !== options.missingDesktop
    ) {
      const archive = `lane-desktop-${host}-v1.2.3.tar.gz`;
      const bytes = `synthetic desktop archive for ${host}\n`;
      write(
        resolve(candidate, archive),
        options.corruptDesktop === host ? "corrupted" : bytes,
      );
      write(
        resolve(candidate, `desktop-${host}.json`),
        JSON.stringify({
          ...identity,
          platform: host,
          cliVersion: version,
          archive,
          sha256: digest(bytes),
          signing: host.startsWith("darwin") ? "ad-hoc" : "unsigned",
        }),
      );
    }
  }
  return { inputs, destination: resolve(directory, "release"), directory };
}

test("complete assembly preserves all six verified executables and combines notices", (t) => {
  const f = fixture(t);
  assemble(f.inputs, f.destination, identity);
  assert.equal(readdirSync(f.destination).length, 12);
  const extracted = resolve(f.directory, "installed");
  mkdirSync(extracted);
  execFileSync("tar", [
    "-xzf",
    resolve(f.destination, `delinoio-lane-${version}.tgz`),
    "-C",
    extracted,
  ]);
  const pkg = resolve(extracted, "package");
  for (const platform of platforms)
    assert.match(
      readFileSync(verifiedExecutable(pkg, platform), "utf8"),
      /synthetic/,
    );
  assert.equal(
    JSON.parse(readFileSync(resolve(pkg, "THIRD_PARTY_NOTICES/modules.json")))
      .length,
    5,
  );
  const sums = readFileSync(resolve(f.destination, "SHA256SUMS"), "utf8")
    .trim()
    .split("\n");
  for (const line of sums) {
    const [hash, name] = line.split("  ");
    assert.equal(digest(readFileSync(resolve(f.destination, name))), hash);
  }
});

for (const [name, options, reason] of [
  ["missing cross-built target", { missing: "windows-arm64" }, /all six CLI/],
  ["mixed revision", { wrongRevision: true }, /AssertionError/],
  ["corrupted executable", { corrupt: "linux-arm64" }, /checksum mismatch/],
  ["missing desktop", { missingDesktop: "darwin-amd64" }, /all three desktop/],
  ["corrupted desktop", { corruptDesktop: "windows-amd64" }, /AssertionError/],
]) {
  test(`assembly refuses ${name} before publication`, (t) => {
    const f = fixture(t, options);
    assert.throws(() => assemble(f.inputs, f.destination, identity), reason);
  });
}

test("publication rejects damaged downloads before contacting GitHub", (t) => {
  const f = fixture(t);
  assemble(f.inputs, f.destination, identity);
  write(
    resolve(f.destination, `lane-linux-arm64-${version}.tar.gz`),
    "damaged",
  );
  assert.throws(() => publish(f.destination, identity), /AssertionError/);
});

// This fake gh stores only fixture bytes in a disposable directory. It lets the
// retry path run without credentials, network access or a real GitHub write.
function publicationFixture(t) {
  const f = fixture(t);
  assemble(f.inputs, f.destination, identity);
  const bin = resolve(f.directory, "bin");
  const state = resolve(f.directory, "github.json");
  write(
    state,
    JSON.stringify({
      exists: false,
      draft: true,
      assets: [],
      creates: 0,
      failOnce: true,
    }),
  );
  write(
    resolve(bin, "gh"),
    `#!/usr/bin/env node
const fs = require('node:fs');
const path = require('node:path');
const args = process.argv.slice(2);
const file = process.env.LANE_FAKE_GITHUB;
const state = JSON.parse(fs.readFileSync(file));
const save = () => fs.writeFileSync(file, JSON.stringify(state));
if (args[0] === 'api') {
  if (!state.exists) { process.stderr.write('gh: Not Found (HTTP 404)'); process.exit(1); }
  process.stdout.write(JSON.stringify(state));
} else if (args[1] === 'create') {
  if (state.exists) process.exit(2);
  state.exists = true; state.creates++; save();
} else if (args[1] === 'upload') {
  if (state.failOnce && state.assets.length === 2) {
    state.failOnce = false; save(); process.stderr.write('fixture upload failure'); process.exit(1);
  }
  const source = args.at(-1);
  const name = path.basename(source);
  if (state.assets.some(a => a.name === name)) process.exit(3);
  const stored = path.join(path.dirname(file), 'stored-' + name);
  fs.copyFileSync(source, stored); state.assets.push({name, stored}); save();
} else if (args[1] === 'download') {
  const name = args[args.indexOf('--pattern') + 1];
  fs.copyFileSync(state.assets.find(a => a.name === name).stored, path.join(args[args.indexOf('--dir') + 1], name));
} else if (args[1] === 'edit') { state.draft = false; save(); }
else process.exit(4);
`,
  );
  chmodSync(resolve(bin, "gh"), 0o755);
  return {
    ...f,
    state,
    invoke: () =>
      spawnSync(
        process.execPath,
        [
          new URL("./lane.mjs", import.meta.url).pathname,
          "publish",
          f.destination,
        ],
        {
          encoding: "utf8",
          env: {
            ...process.env,
            PATH: `${bin}${delimiter}${process.env.PATH}`,
            LANE_FAKE_GITHUB: state,
            LANE_RELEASE_TAG: identity.tag,
            LANE_SOURCE_REVISION: identity.revision,
          },
        },
      ),
  };
}

test(
  "a partial upload retry reuses identical assets and publishes only after completion",
  { skip: process.platform === "win32" },
  (t) => {
    const f = publicationFixture(t);
    assert.notEqual(f.invoke().status, 0);
    let state = JSON.parse(readFileSync(f.state));
    assert.equal(state.draft, true);
    assert.equal(state.assets.length, 2);
    const retried = f.invoke();
    assert.equal(retried.status, 0, retried.stderr);
    state = JSON.parse(readFileSync(f.state));
    assert.equal(state.creates, 1);
    assert.equal(state.assets.length, 12);
    assert.equal(state.draft, false);
    assert.equal(
      f.invoke().status,
      0,
      "a completed release can be checked without overwriting assets",
    );
  },
);

test(
  "a retry refuses a changed published asset and leaves the draft unpublished",
  { skip: process.platform === "win32" },
  (t) => {
    const f = publicationFixture(t);
    assert.notEqual(f.invoke().status, 0);
    const state = JSON.parse(readFileSync(f.state));
    write(state.assets[0].stored, "different published bytes");
    const retried = f.invoke();
    assert.notEqual(retried.status, 0);
    assert.match(retried.stderr, /published asset differs/);
    const after = JSON.parse(readFileSync(f.state));
    assert.equal(after.draft, true);
    assert.equal(after.creates, 1);
    assert.equal(after.assets.length, 2);
  },
);

test("release identity rejects branch names and malformed revisions", () => {
  assert.deepEqual(
    releaseIdentity({
      LANE_RELEASE_TAG: identity.tag,
      LANE_SOURCE_REVISION: identity.revision,
    }),
    identity,
  );
  assert.throws(() =>
    releaseIdentity({
      LANE_RELEASE_TAG: "main",
      LANE_SOURCE_REVISION: identity.revision,
    }),
  );
  assert.throws(() =>
    releaseIdentity({
      LANE_RELEASE_TAG: identity.tag,
      LANE_SOURCE_REVISION: "main",
    }),
  );
});

test("binary release is gated by validated source, complete assembly and protected publication", () => {
  const read = (name) =>
    yaml.load(
      readFileSync(
        new URL(`../../.github/workflows/${name}.yml`, import.meta.url),
        "utf8",
      ),
    );
  const shared = read("release-delino");
  assert.equal(shared.jobs.binaries.needs, "source");
  assert.equal(
    shared.jobs.binaries.uses,
    "./.github/workflows/release-lane.yml",
  );
  assert.equal(
    shared.jobs.binaries.with.revision,
    "${{ needs.source.outputs.revision }}",
  );
  const workflow = read("release-lane");
  assert.deepEqual(Object.keys(workflow.on), ["workflow_call"]);
  assert.equal(workflow.permissions.contents, "read");
  assert.equal(workflow.jobs.build.strategy["fail-fast"], false);
  assert.equal(workflow.jobs.build.strategy.matrix.include.length, 5);
  assert.equal(workflow.jobs.assemble.needs, "build");
  assert.equal(workflow.jobs.publish.needs, "assemble");
  assert.equal(workflow.jobs.publish.environment, "delino-site-production");
  assert.deepEqual(workflow.jobs.publish.permissions, { contents: "write" });
  assert.match(workflow.jobs.build.if, /refs\/tags\/delino@v/);
  assert.match(workflow.jobs.build.if, /refs\/heads\/main/);
  for (const job of Object.values(workflow.jobs)) {
    const checkout = job.steps.find((step) =>
      step.uses?.startsWith("actions/checkout@"),
    );
    assert.equal(checkout.with.ref, "${{ inputs.revision }}");
    assert.equal(checkout.with["persist-credentials"], false);
    assert.equal(job.steps.at(-1).if, "always()");
  }
});
