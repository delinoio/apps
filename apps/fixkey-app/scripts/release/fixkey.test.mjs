// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  mkdtempSync,
  mkdirSync,
  writeFileSync,
  readFileSync,
  rmSync,
  symlinkSync,
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
test("pinned standard licenses retain their checksums in Windows checkouts", (t) => {
  const dir = fixture(t);
  const sources = JSON.parse(
    readFileSync(
      new URL(
        "../../../../apps/fixkey-app/src-tauri/licenses/sources.json",
        import.meta.url,
      ),
      "utf8",
    ),
  );
  const git = (args) =>
    execFileSync("git", args, { cwd: dir, encoding: "utf8" });
  git(["init", "--quiet"]);
  git(["config", "core.autocrlf", "true"]);
  writeFileSync(
    path.join(dir, ".gitattributes"),
    readFileSync(new URL("../../../../.gitattributes", import.meta.url)),
  );
  const relativeDirectory = "apps/fixkey-app/src-tauri/licenses";
  mkdirSync(path.join(dir, relativeDirectory), { recursive: true });
  const files = Object.entries(sources).filter(
    ([name]) => name !== "schemaVersion",
  );
  for (const [name] of files) {
    writeFileSync(
      path.join(dir, relativeDirectory, `${name}.txt`),
      readFileSync(
        new URL(
          `../../../../${relativeDirectory}/${name}.txt`,
          import.meta.url,
        ),
      ),
    );
  }
  git(["add", ".gitattributes", relativeDirectory]);
  for (const [name] of files)
    rmSync(path.join(dir, relativeDirectory, `${name}.txt`));
  git(["checkout-index", "--all", "--force"]);
  for (const [name, record] of files) {
    const bytes = readFileSync(
      path.join(dir, relativeDirectory, `${name}.txt`),
    );
    assert.equal(
      createHash("sha256").update(bytes).digest("hex"),
      record.sha256,
      name,
    );
  }
});
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
      new URL(
        "../../../../apps/fixkey-app/src-tauri/tauri.conf.json",
        import.meta.url,
      ),
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
    "linux-aarch64",
    "linux-x86_64",
    "windows-aarch64",
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
test("ARM64 updater payloads remain separate from x64 payloads", (t) => {
  const out = bundles(t);
  const result = metadata(out, "0.0.1", revision, key);
  for (const [target, payload] of [
    ["windows-aarch64", "FixKey-windows-arm64.exe"],
    ["linux-aarch64", "FixKey-linux-x11-arm64.AppImage"],
  ]) {
    assert.equal(
      result.platforms[target].url,
      `https://github.com/delinoio/apps/releases/download/fixkey@v0.0.1/${payload}`,
    );
    assert.ok(
      readFileSync(path.join(out, "SHA256SUMS"), "utf8").includes(
        `  ${payload}\n`,
      ),
    );
  }
  for (const platform of ["windows-arm64", "linux-x11-arm64"]) {
    const record = path.join(out, `${platform}.json`);
    const bytes = readFileSync(record);
    rmSync(record);
    assert.throws(() => metadata(out, "0.0.1", revision, key), /ENOENT/u);
    writeFileSync(record, bytes);
  }
});
test("ARM64 release jobs use native hosts and the pinned CLI", () => {
  const workflow = yaml.load(
    readFileSync(
      new URL(
        "../../../../.github/workflows/release-fixkey.yml",
        import.meta.url,
      ),
      "utf8",
    ),
  );
  const lock = JSON.parse(
    readFileSync(
      new URL("../../../../prebuilt-dependencies.lock.json", import.meta.url),
      "utf8",
    ),
  );
  const rows = workflow.jobs.build.strategy.matrix.include;
  assert.deepEqual(
    rows.map((row) => row.platform).sort(),
    Object.keys(platforms).sort(),
  );
  for (const [platform, os, target, bundles] of [
    ["windows-arm64", "windows-11-arm", "aarch64-pc-windows-msvc", "nsis"],
    [
      "linux-x11-arm64",
      "ubuntu-22.04-arm",
      "aarch64-unknown-linux-gnu",
      "appimage",
    ],
  ]) {
    assert.deepEqual(
      rows.find((row) => row.platform === platform),
      { platform, os, target, bundles },
    );
    assert.equal(lock.dependencies["tauri-cli"].assets[target].arch, "arm64");
  }
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
test("staging ignores unpacked application trees but rejects artifact symlinks", (t) => {
  const assets = bundles(t);
  for (const [platform, tree, folder] of [
    ["macos-arm64", "FixKey.app", "macos"],
    ["linux-x11-x64", "fixkey.AppDir", "appimage"],
  ]) {
    const bundle = path.join(path.dirname(assets), platform);
    const unpacked = path.join(bundle, folder, tree);
    mkdirSync(unpacked, { recursive: true });
    symlinkSync(
      "missing-internal-target",
      path.join(unpacked, "internal-link"),
    );
    // A payload-shaped file inside an unpacked app must not cause ambiguity.
    writeFileSync(
      path.join(unpacked, `nested${platforms[platform].bundle}`),
      "internal",
    );
    stage(bundle, assets, platform, "0.0.1", revision, key);
    const link = path.join(bundle, `linked${platforms[platform].bundle}`);
    symlinkSync("missing-payload", link);
    assert.throws(
      () => stage(bundle, assets, platform, "0.0.1", revision, key),
      /artifact-symlink/u,
    );
    rmSync(link);
  }
});
test("source versions stay synchronized", () => {
  const root = path.resolve(import.meta.dirname, "../../../..");
  const selected = JSON.parse(
    readFileSync(path.join(root, "apps/fixkey-app/package.json"), "utf8"),
  ).version;
  validateSource(root, selected);
});
test("source validation accepts Windows checkout line endings and rejects version drift", (t) => {
  const dir = fixture(t);
  const native = path.join(dir, "apps/fixkey-app/src-tauri");
  mkdirSync(native, { recursive: true });
  writeFileSync(
    path.join(dir, "apps/fixkey-app/package.json"),
    '{\r\n  "version": "0.1.4"\r\n}\r\n',
  );
  writeFileSync(
    path.join(native, "tauri.conf.json"),
    '{\r\n  "version": "0.1.4"\r\n}\r\n',
  );
  writeFileSync(
    path.join(native, "Cargo.toml"),
    '[package]\r\nname = "fixkey"\r\nversion = "0.1.4"\r\n',
  );
  const lock = path.join(dir, "Cargo.lock");
  writeFileSync(
    lock,
    '[[package]]\r\nname = "fixkey"\r\nversion = "0.1.4"\r\n',
  );
  validateSource(dir, "0.1.4");
  writeFileSync(
    lock,
    '[[package]]\r\nname = "fixkey"\r\nversion = "0.1.3"\r\n',
  );
  assert.throws(
    () => validateSource(dir, "0.1.4"),
    /fixkey-lock-version-mismatch/u,
  );
});
test("tagged releases validate main ancestry before signing and publication", () => {
  const workflow = yaml.load(
    readFileSync(
      new URL(
        "../../../../.github/workflows/release-fixkey.yml",
        import.meta.url,
      ),
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
  const preparation = workflow.jobs.prepare.steps.at(-1);
  assert.match(preparation.run, /test -n "\$FIXKEY_UPDATER_PUBLIC_KEY"/u);
  assert.match(preparation.run, /fixkey\.mjs source/u);
  assert.match(
    workflow.jobs.publish.steps.at(-1).run,
    /tag="fixkey@v\$RELEASE_VERSION"/u,
  );
  assert.equal(workflow.jobs.build.environment, "release-signing");
  assert.equal(
    workflow.jobs.build.strategy.matrix.include.find(
      (entry) => entry.platform === "macos-arm64",
    ).bundles,
    "app,dmg",
  );
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
  assert.doesNotMatch(
    JSON.stringify(workflow),
    /FIXKEY_NATIVE_ACCEPTANCE_REVISION|Native acceptance was reviewed/u,
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
