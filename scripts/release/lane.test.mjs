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
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, resolve } from "node:path";
import test from "node:test";
import { setTimeout as delay } from "node:timers/promises";
import yaml from "js-yaml";
import { assemble, publish, releaseIdentity } from "./lane.mjs";
import {
  platforms,
  platformPackage,
  verifiedExecutable,
} from "../../packages/lane/bin/verify.mjs";

const identity = { tag: "lane@v1.2.3", revision: "a".repeat(40) };
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
      JSON.stringify({
        name: "@delinoio/lane",
        version,
        optionalDependencies: Object.fromEntries(
          platforms.map((key) => [platformPackage(key).name, version]),
        ),
      }),
    );
    for (const name of ["LICENSE", "NOTICE"])
      write(resolve(pkg, name), `${name} fixture\n`);
    for (const target of targets.filter(
      (target) => target !== options.missing,
    )) {
      const name = target.startsWith("windows") ? "lane.exe" : "lane";
      const platformRoot = resolve(
        directory,
        "sources",
        host,
        target,
        "package",
      );
      const info = platformPackage(target);
      const bytes = `synthetic ${target} executable; not native acceptance\n`;
      const entry = {
        platform: target,
        version,
        path: name,
        size: Buffer.byteLength(bytes),
        sha256: digest(bytes),
      };
      write(
        resolve(platformRoot, "package.json"),
        JSON.stringify({
          name: info.name,
          version,
          os: [info.os],
          cpu: [info.cpu],
        }),
      );
      write(
        resolve(platformRoot, name),
        options.corrupt === target ? "corrupted" : bytes,
      );
      chmodSync(resolve(platformRoot, name), 0o755);
      write(
        resolve(platformRoot, "manifest.json"),
        JSON.stringify({ ...entry, sourceRevision: identity.revision }),
      );
      for (const notice of ["LICENSE", "NOTICE"])
        write(
          resolve(platformRoot, notice),
          readFileSync(resolve(pkg, notice)),
        );
      write(
        resolve(platformRoot, "THIRD_PARTY_NOTICES/modules.json"),
        JSON.stringify([
          { module: host, version: "1.0.0", source: "fixture", license: "MIT" },
        ]),
      );
      manifest.artifacts[target] = entry;
      mkdirSync(candidate, { recursive: true });
      execFileSync("tar", [
        "-czf",
        resolve(candidate, `delinoio-lane-${target}-${version}.tgz`),
        "-C",
        resolve(platformRoot, ".."),
        "package",
      ]);
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

test("complete assembly keeps a lightweight launcher and six verified platform packages", (t) => {
  const f = fixture(t);
  assemble(f.inputs, f.destination, identity);
  assert.equal(readdirSync(f.destination).length, 18);
  const extracted = resolve(f.directory, "installed");
  mkdirSync(extracted);
  execFileSync("tar", [
    "-xzf",
    resolve(f.destination, `delinoio-lane-${version}.tgz`),
    "-C",
    extracted,
  ]);
  const pkg = resolve(extracted, "package");
  assert.deepEqual(readdirSync(pkg).sort(), [
    "LICENSE",
    "NOTICE",
    "manifest.json",
    "package.json",
  ]);
  for (const platform of platforms) {
    const destination = resolve(
      pkg,
      "node_modules/@delinoio",
      `lane-${platform}`,
    );
    mkdirSync(destination, { recursive: true });
    execFileSync("tar", [
      "-xzf",
      resolve(f.destination, `delinoio-lane-${platform}-${version}.tgz`),
      "--strip-components=1",
      "-C",
      destination,
    ]);
    assert.match(
      readFileSync(verifiedExecutable(pkg, platform), "utf8"),
      /synthetic/,
    );
    assert.ok(
      JSON.parse(
        readFileSync(resolve(destination, "THIRD_PARTY_NOTICES/modules.json")),
      ).length,
    );
    const direct = resolve(f.directory, "direct", platform);
    mkdirSync(direct, { recursive: true });
    execFileSync("tar", [
      "-xzf",
      resolve(f.destination, `lane-${platform}-${version}.tar.gz`),
      "-C",
      direct,
    ]);
    assert.equal(
      statSync(
        resolve(
          direct,
          `lane-${platform}`,
          platform.startsWith("windows-") ? "lane.exe" : "lane",
        ),
      ).mode & 0o777,
      0o755,
    );
  }
  const sums = readFileSync(resolve(f.destination, "SHA256SUMS"), "utf8")
    .trim()
    .split("\n");
  for (const line of sums) {
    const [hash, name] = line.split("  ");
    assert.equal(digest(readFileSync(resolve(f.destination, name))), hash);
  }
});

test("reassembly of identical candidates produces byte-identical release files", async (t) => {
  const f = fixture(t);
  assemble(f.inputs, f.destination, identity);
  await delay(1100);
  const second = resolve(f.directory, "assembled-again");
  assemble(f.inputs, second, identity);
  for (const name of readdirSync(f.destination))
    assert.deepEqual(
      readFileSync(resolve(second, name)),
      readFileSync(resolve(f.destination, name)),
      name,
    );
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
  process.stdout.write(JSON.stringify([state.exists ? [state] : []]));
} else if (args[1] === 'create') {
  if (state.exists) process.exit(2);
  state.exists = true; state.tag_name = args[2]; state.target_commitish = args[args.indexOf('--target') + 1]; state.creates++; save();
} else if (args[1] === 'upload') {
  if (state.failOnce && state.assets.length === 11) {
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
    invoke: (destination = f.destination) =>
      spawnSync(
        process.execPath,
        [
          new URL("./lane.mjs", import.meta.url).pathname,
          "publish",
          destination,
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
  "a partial upload retry reassembles candidates and reuses immutable assets",
  { skip: process.platform === "win32" },
  async (t) => {
    const f = publicationFixture(t);
    assert.notEqual(f.invoke().status, 0);
    let state = JSON.parse(readFileSync(f.state));
    assert.equal(state.draft, true);
    assert.equal(state.assets.length, 11);
    await delay(1100);
    const reassembled = resolve(f.directory, "reassembled");
    assemble(f.inputs, reassembled, identity);
    const retried = f.invoke(reassembled);
    assert.equal(retried.status, 0, retried.stderr);
    state = JSON.parse(readFileSync(f.state));
    assert.equal(state.creates, 1);
    assert.equal(state.assets.length, 18);
    assert.equal(
      state.draft,
      true,
      "component completion publishes the draft after other deployments",
    );
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
    assert.equal(after.assets.length, 11);
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
  const shared = read("deploy-component");
  assert.equal(shared.jobs.lane.uses, "./.github/workflows/deploy-lane.yml");
  assert.equal(shared.jobs.lane.with.revision, "${{ inputs.revision }}");
  const workflow = read("deploy-lane");
  assert.deepEqual(Object.keys(workflow.on), ["workflow_call"]);
  assert.equal(workflow.permissions.contents, "read");
  assert.equal(workflow.jobs.build.strategy["fail-fast"], false);
  assert.equal(workflow.jobs.build.strategy.matrix.include.length, 5);
  const buildSteps = workflow.jobs.build.steps;
  assert.ok(
    !buildSteps.some((step) => step.run?.includes("lane-windows-sdk.ps1")),
  );
  assert.ok(
    !Object.keys(workflow.jobs.build.env).some((key) =>
      key.startsWith("LANE_WINDOWS_APP_SDK_"),
    ),
  );
  const desktop = buildSteps.find(
    (step) =>
      step.name ===
      "Build desktop with the matching packaged synchronization helper",
  );
  assert.match(desktop.run, /pnpm --dir "apps\/lane" build:desktop/);
  assert.equal(workflow.jobs.assemble.needs, "build");
  assert.equal(workflow.jobs.publish.needs, "assemble");
  assert.equal(workflow.jobs.publish.environment, "delino-site-production");
  assert.deepEqual(workflow.jobs.publish.permissions, { contents: "write" });
  assert.match(workflow.jobs.build.if, /refs\/heads\/main/);
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
