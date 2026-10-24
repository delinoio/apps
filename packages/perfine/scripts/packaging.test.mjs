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
} from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { execFileSync } from "node:child_process";
import { createRequire } from "node:module";
import { stage, sha256, root, linuxBinary } from "./build.mjs";
import { exactTag, verifyTree, inventory } from "./release.mjs";
const require = createRequire(import.meta.url);
const sourceCommit = "a".repeat(40);
function fixture(t) {
  const temp = mkdtempSync(join(tmpdir(), "perfine-pack-test-"));
  t.after(() => rmSync(temp, { recursive: true, force: true }));
  const bytes = Buffer.alloc(128);
  bytes.write("\x7fELF");
  bytes[4] = 2;
  bytes[5] = 1;
  bytes.writeUInt16LE(62, 18);
  const binary = join(temp, "binary");
  writeFileSync(binary, bytes);
  const notices = join(temp, "notices");
  writeFileSync(notices, "Fixture only. No native or provider acceptance.\n");
  const output = join(temp, "release");
  stage({ binary, notices, output, sourceCommit, releaseVersion: "0.1.0" });
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
  assert.equal(verifiedBinary(pkg), join(pkg, "runtime/perfine"));
  writeFileSync(join(pkg, "runtime/perfine"), "tampered");
  assert.throws(() => verifiedBinary(pkg), /checksum/);
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
  assert.doesNotMatch(text, /macos|--force|--clobber/);
});
