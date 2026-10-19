// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import {
  mkdtempSync,
  mkdirSync,
  writeFileSync,
  readFileSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import yaml from "js-yaml";
import {
  configure,
  metadata,
  stage,
  platforms,
  version,
  validateSource,
} from "./fixkey.mjs";
const revision = "a".repeat(40),
  key = "fixture-public-key";
function fixture(t) {
  const dir = mkdtempSync(path.join(tmpdir(), "fixkey-release-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}
function bundles(t) {
  const dir = fixture(t),
    out = path.join(dir, "assets");
  for (const [platform, spec] of Object.entries(platforms)) {
    const bundle = path.join(dir, platform);
    mkdirSync(bundle);
    for (const suffix of new Set([spec.package, spec.bundle]))
      writeFileSync(
        path.join(bundle, `FixKey_0.0.1${suffix}`),
        `fixture-${platform}-${suffix}`,
      );
    writeFileSync(
      path.join(bundle, `FixKey_0.0.1${spec.bundle}.sig`),
      Buffer.from(
        "untrusted comment: test fixture, not a signed payload\nfixture\ntrusted comment: timestamp:0\tfile:fixture\tversion:0.0.1\nfixture",
      ).toString("base64"),
    );
    stage(bundle, out, platform, "0.0.1", revision, key);
  }
  return out;
}
test("all platforms require updater signatures without a Windows code-signing identity", (t) => {
  const dir = fixture(t),
    file = path.join(dir, "config.json");
  for (const platform of Object.keys(platforms)) {
    configure(file, platform, key);
    const config = JSON.parse(readFileSync(file, "utf8"));
    assert.deepEqual(config.plugins.updater, {
      pubkey: key,
      requireSignedVersion: true,
      allowDowngrades: false,
    });
    assert.equal(config.bundle, undefined);
    assert.throws(() => configure(file, platform, ""), /updater-key-missing/u);
  }
  const config = JSON.parse(
    readFileSync(
      new URL("../../apps/fixkey/src-tauri/tauri.conf.json", import.meta.url),
      "utf8",
    ),
  );
  assert.equal(config.plugins.updater.requireSignedVersion, true);
  assert.equal(config.plugins.updater.allowDowngrades, false);
});
test("metadata binds every platform to the exact source, key and payload hashes", (t) => {
  const out = bundles(t);
  const result = metadata(out, "0.0.1", revision, key);
  assert.deepEqual(Object.keys(result.platforms).sort(), [
    "darwin-aarch64",
    "linux-x86_64",
    "windows-x86_64",
  ]);
  assert.match(
    result.platforms["darwin-aarch64"].url,
    /^https:\/\/github\.com\/delinoio\/apps\/releases\/download\/fixkey@v0\.0\.1\/FixKey-macos-arm64.app.tar.gz$/u,
  );
  assert.match(
    readFileSync(path.join(out, "SHA256SUMS"), "utf8"),
    /fixkey-latest.json/u,
  );
  assert.throws(
    () => metadata(out, "0.0.1", "b".repeat(40), key),
    /identity-mismatch/u,
  );
  assert.throws(
    () => metadata(out, "0.0.1", revision, "wrong-key"),
    /identity-mismatch/u,
  );
  writeFileSync(path.join(out, platforms["windows-x64"].payload), "tampered");
  assert.throws(
    () => metadata(out, "0.0.1", revision, key),
    /checksum-mismatch/u,
  );
});
test("missing platforms, signatures and ambiguous bundles fail closed", (t) => {
  const dir = fixture(t);
  assert.throws(() => metadata(dir, "0.0.1", revision, key));
  writeFileSync(path.join(dir, "a.AppImage"), "fixture");
  assert.throws(
    () =>
      stage(
        dir,
        path.join(dir, "out"),
        "linux-x11-x64",
        "0.0.1",
        revision,
        key,
      ),
    /signed-payload-missing/u,
  );
  writeFileSync(path.join(dir, "b.AppImage"), "fixture");
  assert.throws(
    () =>
      stage(
        dir,
        path.join(dir, "out"),
        "linux-x11-x64",
        "0.0.1",
        revision,
        key,
      ),
    /ambiguous/u,
  );
  for (const invalid of ["1.0.0-beta.1", "01.0.0", "1.0.0;touch injected"])
    assert.throws(() => version(invalid));
});
test("source versions stay synchronized", () => {
  const root = path.resolve(import.meta.dirname, "../..");
  const selected = JSON.parse(
    readFileSync(path.join(root, "apps/fixkey/package.json"), "utf8"),
  ).version;
  validateSource(root, selected);
});
test("tagged releases validate main ancestry before native acceptance, signing and publication", () => {
  const workflow = yaml.load(
    readFileSync(
      new URL("../../.github/workflows/release-fixkey.yml", import.meta.url),
      "utf8",
    ),
  );
  assert.deepEqual(workflow.on.push.tags, ["fixkey@v*"]);
  assert.equal(workflow.on.workflow_dispatch.inputs.tag.required, true);
  const source = workflow.jobs.source;
  assert.match(source.if, /github.event_name == 'push'/u);
  assert.match(source.if, /refs\/heads\/main/u);
  assert.equal(source.environment, undefined);
  assert.equal(source["runs-on"], "ubuntu-latest");
  assert.equal(source.steps[0].with.ref, "main");
  assert.equal(source.steps[0].with["fetch-depth"], 0);
  assert.equal(source.steps.at(-1).env.RELEASE_PROJECT, "fixkey");
  assert.equal(
    source.steps.at(-1).env.RELEASE_TAG,
    "${{ github.event_name == 'push' && github.ref_name || inputs.tag }}",
  );
  assert.match(source.steps.at(-1).run, /validate-source/u);
  assert.equal(workflow.jobs.prepare.needs, "source");
  assert.equal(
    workflow.jobs.prepare.steps[0].with.ref,
    "${{ needs.source.outputs.revision }}",
  );
  assert.equal(
    workflow.jobs.prepare.outputs.revision,
    "${{ needs.source.outputs.revision }}",
  );
  for (const name of ["build", "publish"]) {
    const job = workflow.jobs[name];
    assert.equal(
      job.steps[0].with.ref,
      "${{ needs.prepare.outputs.revision }}",
    );
    assert.equal(
      job.env.RELEASE_REVISION,
      "${{ needs.prepare.outputs.revision }}",
    );
    assert.ok(job.needs.includes("prepare"));
    assert.doesNotMatch(
      job.steps.map((step) => step.run ?? "").join("\n"),
      /\$GITHUB_SHA/u,
    );
  }
  const approval = workflow.jobs.prepare.steps.at(-1);
  assert.equal(
    approval.env.RELEASE_REVISION,
    "${{ needs.source.outputs.revision }}",
  );
  assert.match(
    approval.run,
    /test "\$FIXKEY_NATIVE_ACCEPTANCE_REVISION" = "\$RELEASE_REVISION"/u,
  );
  assert.match(
    workflow.jobs.publish.steps.at(-1).run,
    /tag="fixkey@v\$RELEASE_VERSION"/u,
  );
  assert.equal(workflow.jobs.build.environment, "release-signing");
  assert.equal(workflow.jobs.publish.environment, "release-publish");
  const buildEnv = workflow.jobs.build.steps.find(
    (step) => step.name === "Build installers and signed updater payloads",
  ).env;
  for (const name of [
    "APPLE_CERTIFICATE",
    "APPLE_CERTIFICATE_PASSWORD",
    "APPLE_SIGNING_IDENTITY",
    "APPLE_ID",
    "APPLE_PASSWORD",
    "APPLE_TEAM_ID",
  ]) {
    assert.ok(buildEnv[name].includes(`secrets.${name}`));
  }
  assert.doesNotMatch(
    JSON.stringify(workflow),
    /FIXKEY_APPLE_|fixkey-signing|fixkey-publish/u,
  );
  assert.equal(workflow.jobs.publish.permissions.contents, "write");
  assert.match(
    workflow.jobs.prepare.steps.map((step) => step.run ?? "").join("\n"),
    /FIXKEY_NATIVE_ACCEPTANCE_REVISION/u,
  );
  const steps = workflow.jobs.build.steps;
  assert.ok(
    steps.some(
      (step) => step.name === "Verify macOS signature and notarization receipt",
    ),
  );
  assert.ok(
    steps.some(
      (step) =>
        step.name ===
        "Verify every updater signature with the embedded release key",
    ),
  );
  assert.doesNotMatch(
    JSON.stringify(workflow),
    /FIXKEY_WINDOWS_|Get-AuthenticodeSignature|Import-PfxCertificate/u,
  );
  assert.match(
    JSON.stringify(workflow.jobs.publish),
    /Windows executables are not Authenticode signed/u,
  );
  assert.equal(
    workflow.jobs.build.strategy.matrix.include.find(
      (row) => row.platform === "macos-arm64",
    ).os,
    "macos-14",
  );
});
