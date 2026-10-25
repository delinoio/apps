// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import {
  mkdtempSync,
  readFileSync,
  writeFileSync,
  mkdirSync,
  rmSync,
  readdirSync,
  cpSync,
  realpathSync,
} from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { execFileSync } from "node:child_process";
import { createRequire } from "node:module";
import {
  stage,
  sha256,
  root,
  linuxBinary,
  targetInfo,
  targets,
  nativeBinary,
} from "./build.mjs";
import { exactTag, verifyTree, inventory } from "./release.mjs";
const require = createRequire(import.meta.url);
const sourceCommit = "a".repeat(40);
function fixture(t, target = "x86_64-unknown-linux-gnu") {
  const temp = realpathSync(mkdtempSync(join(tmpdir(), "perfine-pack-test-")));
  t.after(() => rmSync(temp, { recursive: true, force: true }));
  const bytes = Buffer.alloc(128);
  bytes.write("\x7fELF");
  bytes[4] = 2;
  bytes[5] = 1;
  bytes.writeUInt16LE(62, 18);
  if (target.includes("apple")) {
    bytes.fill(0);
    bytes.writeUInt32LE(0xfeedfacf, 0);
    bytes.writeUInt32LE(
      target.startsWith("aarch64") ? 0x0100000c : 0x01000007,
      4,
    );
    bytes.writeUInt32LE(2, 12);
  } else if (target.includes("windows")) {
    bytes.fill(0);
    bytes.write("MZ");
    bytes.writeUInt32LE(64, 60);
    bytes.writeUInt32LE(0x4550, 64);
    bytes.writeUInt16LE(0x8664, 68);
    bytes.writeUInt16LE(2, 86);
    bytes.writeUInt16LE(0x20b, 88);
  }
  const binary = join(temp, "binary");
  writeFileSync(binary, bytes);
  const notices = join(temp, "notices");
  writeFileSync(notices, "Fixture only. No native or provider acceptance.\n");
  const output = join(temp, "release");
  stage({
    binary,
    notices,
    output,
    sourceCommit,
    releaseVersion: "0.1.0",
    target,
  });
  return { temp, output, binary };
}
test("actual npm tarball and standalone archive contain exact binary, checksums, notices and provenance", (t) => {
  const { output, temp } = fixture(t);
  const extract = join(temp, "extract");
  mkdirSync(extract);
  execFileSync("tar", [
    "-xzf",
    join(output, "perfine-0.1.0.tgz"),
    "-C",
    extract,
  ]);
  const pkg = join(extract, "package");
  const metadata = JSON.parse(readFileSync(join(pkg, "package.json")));
  assert.equal(metadata.name, "perfine");
  assert.equal(metadata.version, "0.1.0");
  assert.equal(metadata.scripts, undefined);
  assert.equal(metadata.bin.perfine, "bin/perfine.cjs");
  for (const file of [
    "LICENSE",
    "NOTICE",
    "THIRD_PARTY_NOTICES",
    "provenance.json",
  ])
    assert.ok(readFileSync(join(pkg, file)).length);
  const { verifiedBinary } = require(join(pkg, "bin/perfine.cjs"));
  const platform = join(pkg, "node_modules/@delinoio/perfine-linux-x64");
  cpSync(join(output, "platform"), platform, { recursive: true });
  assert.equal(
    verifiedBinary(pkg, "linux", "x64"),
    join(platform, "runtime/perfine"),
  );
  writeFileSync(join(platform, "runtime/perfine"), "tampered");
  assert.throws(() => verifiedBinary(pkg, "linux", "x64"), /checksum/);
  const standalone = join(temp, "standalone");
  mkdirSync(standalone);
  execFileSync("tar", [
    "-xzf",
    join(output, "perfine-v0.1.0-linux-x86_64.tar.gz"),
    "-C",
    standalone,
  ]);
  assert.equal(
    JSON.parse(readFileSync(join(standalone, "provenance.json"))).sourceCommit,
    sourceCommit,
  );
  assert.match(
    readFileSync(join(standalone, "runtime/SHA256SUMS"), "utf8"),
    /^[a-f0-9]{64}  perfine\n$/,
  );
  for (const line of readFileSync(join(output, "SHA256SUMS"), "utf8")
    .trim()
    .split("\n")) {
    const [hash, name] = line.split("  ");
    assert.equal(hash, sha256(readFileSync(join(output, name))));
  }
  assert.ok(!readdirSync(pkg).includes("dist"));
});
test("real distribution Git commit is verified against exact version, source and layout", (t) => {
  const { output } = fixture(t);
  const action = join(output, "action");
  const git = (...args) =>
    execFileSync("git", args, { cwd: action, encoding: "utf8" }).trim();
  git("init", "-q");
  git("add", ".");
  git(
    "-c",
    "user.name=Fixture",
    "-c",
    "user.email=fixture@example.invalid",
    "-c",
    "commit.gpgsign=false",
    "commit",
    "-qm",
    "fixture",
  );
  const sha = git("rev-parse", "HEAD");
  assert.equal(
    verifyTree(action, sha, "v0.1.0", sourceCommit).actionCommit,
    sha,
  );
  assert.throws(
    () => verifyTree(action, sha, "v0.1.1", sourceCommit),
    /provenance/,
  );
  assert.throws(
    () => verifyTree(action, sha, "v0.1.0", "b".repeat(40)),
    /provenance/,
  );
  writeFileSync(join(action, "runtime/perfine"), "tampered");
  git("add", ".");
  git(
    "-c",
    "user.name=Fixture",
    "-c",
    "user.email=fixture@example.invalid",
    "-c",
    "commit.gpgsign=false",
    "commit",
    "-qm",
    "tamper",
  );
  assert.throws(
    () => verifyTree(action, git("rev-parse", "HEAD"), "v0.1.0", sourceCommit),
    /checksum/,
  );
});
test("no moving tags, placeholder SHAs, wrong architecture, or tracked dist in release candidates", (t) => {
  for (const tag of [
    "v1",
    "latest",
    "v1.2",
    "v01.2.3",
    "v1.2.3-beta",
    "v1.2.3\n",
  ])
    assert.throws(() => exactTag(tag));
  const { binary, output } = fixture(t);
  writeFileSync(binary, "#!/bin/sh\n");
  assert.throws(() => linuxBinary(binary), /ELF/);
  mkdirSync(join(output, "action/dist"));
  assert.throws(() => inventory(join(output, "action")), /dist/);
  const tracked = execFileSync(
    "git",
    ["ls-files", "actions/perfine", "packages/perfine"],
    { cwd: root, encoding: "utf8" },
  );
  assert.ok(!tracked.split("\n").some((p) => p.split("/").includes("dist")));
});
test("release pipeline is manual, opt-in, protected and separates build from publication", () => {
  const text = readFileSync(
    join(root, ".github/workflows/release-perfine.yml"),
    "utf8",
  );
  assert.match(text, /workflow_dispatch:/);
  assert.doesNotMatch(text, /^  (push|pull_request):/m);
  assert.match(text, /default: false/);
  assert.match(text, /environment: perfine-production/);
  assert.match(text, /if: inputs.publish/);
  assert.doesNotMatch(text, /--force|--clobber/);
  for (const path of [".github/workflows/release-perfine.yml"]) {
    const workflow = readFileSync(join(root, path), "utf8");
    assert.match(workflow, /node "packages\/perfine\/scripts\/smoke.mjs"/);
    assert.match(
      workflow,
      /git diff --exit-code -- "actions\/perfine\/ACTION_NOTICES"/,
    );
  }
});

test("Ubuntu distribution jobs provision protoc and well-known includes before Cargo", () => {
  const yaml = require("js-yaml");
  for (const [path, job] of [
    [".github/workflows/release-perfine.yml", "build"],
  ]) {
    const steps = yaml.load(readFileSync(join(root, path), "utf8")).jobs[job]
      .steps;
    const build = steps.findIndex((step) => step.run?.includes("cargo build"));
    const setup = steps.findIndex((step) =>
      step.run?.includes(
        "apt-get install -y protobuf-compiler libprotobuf-dev",
      ),
    );
    assert.ok(setup >= 0 && setup < build);
    assert.match(steps[setup].run, /protoc --version/);
    for (const file of ["descriptor", "empty", "timestamp"])
      assert.ok(
        steps[setup].run.includes(
          `test -r "/usr/include/google/protobuf/${file}.proto"`,
        ),
      );
  }
});

// Synthetic headers establish packaging validation only, never native execution.
test("all platform archives use matching metadata and identical npm entry bytes", (t) => {
  let entry;
  for (const target of Object.keys(targets)) {
    const { output, binary } = fixture(t, target);
    const info = targetInfo(target);
    const bytes = readFileSync(join(output, "perfine-0.1.0.tgz"));
    if (entry) assert.deepEqual(bytes, entry);
    else entry = bytes;
    const manifest = JSON.parse(
      readFileSync(join(output, "platform/package.json")),
    );
    assert.equal(manifest.name, info.packageName);
    assert.deepEqual(manifest.os, [info.os]);
    assert.deepEqual(manifest.cpu, [info.cpu]);
    assert.equal(readdirSync(output).includes("action"), info.os === "linux");
    for (const wrong of Object.keys(targets).filter((key) => key !== target))
      assert.throws(() => nativeBinary(binary, wrong), /executable/);
    const entryRoot = join(output, "npm");
    const { verifiedBinary } = require(join(entryRoot, "bin/perfine.cjs"));
    assert.throws(
      () => verifiedBinary(entryRoot, info.os, info.cpu),
      /Reinstall perfine/,
    );
    const installed = join(entryRoot, "node_modules", info.packageName);
    cpSync(join(output, "platform"), installed, { recursive: true });
    assert.equal(
      verifiedBinary(entryRoot, info.os, info.cpu),
      join(installed, "runtime", info.executable),
    );
    for (const [field, value] of [
      ["version", "0.2.0"],
      ["name", "other"],
      ["os", ["other"]],
      ["cpu", ["other"]],
    ]) {
      writeFileSync(
        join(installed, "package.json"),
        JSON.stringify({ ...manifest, [field]: value }),
      );
      assert.throws(
        () => verifiedBinary(entryRoot, info.os, info.cpu),
        /mismatch/,
      );
    }
    writeFileSync(join(installed, "package.json"), JSON.stringify(manifest));
    const provenanceFile = join(installed, "provenance.json");
    const provenance = JSON.parse(readFileSync(provenanceFile));
    writeFileSync(
      provenanceFile,
      JSON.stringify({ ...provenance, target: "wrong" }),
    );
    assert.throws(
      () => verifiedBinary(entryRoot, info.os, info.cpu),
      /provenance mismatch/,
    );
    writeFileSync(provenanceFile, JSON.stringify(provenance));
    writeFileSync(join(installed, "runtime/SHA256SUMS"), "corrupt");
    assert.throws(
      () => verifiedBinary(entryRoot, info.os, info.cpu),
      /checksum mismatch/,
    );
    assert.throws(
      () => verifiedBinary(entryRoot, "linux", "arm64"),
      /Unsupported/,
    );
  }
});
