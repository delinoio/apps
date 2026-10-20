// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import yaml from "js-yaml";

const script = fileURLToPath(new URL("./fixkey-build.sh", import.meta.url));
const busy =
  "failed to bundle project: error running bundle_dmg.sh: `failed to run bundle_dmg.sh: hdiutil: create failed - Resource busy`";

function run(t, outcomes, platform = "macOS") {
  const directory = mkdtempSync(path.join(tmpdir(), "fixkey-build-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const bin = path.join(directory, "bin");
  mkdirSync(bin);
  writeFileSync(
    path.join(bin, "sleep"),
    '#!/bin/sh\nprintf "%s\\n" "$1" >> delays\n',
    { mode: 0o755 },
  );
  const command = path.join(directory, "fake build.mjs");
  writeFileSync(
    command,
    `
import { appendFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
const attempts = existsSync("attempts") ? readFileSync("attempts", "utf8").trim().split("\\n").length : 0;
appendFileSync("attempts", JSON.stringify(process.argv.slice(2)) + "\\n");
const bundle = "target/aarch64-apple-darwin/release/bundle/macos";
if (attempts > 0 && existsSync(bundle + "/rw.123.FixKey_0.1.6_aarch64.dmg")) throw new Error("partial-image-retained");
mkdirSync(bundle, { recursive: true });
writeFileSync(bundle + "/rw.123.FixKey_0.1.6_aarch64.dmg", "partial");
writeFileSync(bundle + "/FixKey_0.1.6_aarch64.dmg", "installer");
const outcome = ${JSON.stringify(outcomes)}[attempts];
console.error(outcome.message);
process.exit(outcome.code);
`,
  );
  const args = ["--config", "config with spaces.json", "literal;$(no-command)"];
  const result = spawnSync(
    "bash",
    [script, process.execPath, command, ...args],
    {
      cwd: directory,
      env: {
        ...process.env,
        RUNNER_OS: platform,
        PATH: `${bin}${path.delimiter}${process.env.PATH}`,
      },
      encoding: "utf8",
    },
  );
  assert.ifError(result.error);
  const attempts = readFileSync(path.join(directory, "attempts"), "utf8")
    .trim()
    .split("\n")
    .map(JSON.parse);
  for (const actual of attempts) assert.deepEqual(actual, args);
  const delays = existsSync(path.join(directory, "delays"))
    ? readFileSync(path.join(directory, "delays"), "utf8").trim().split("\n")
    : [];
  assert.deepEqual(delays, Array(attempts.length - 1).fill("10"));
  assert.equal(
    readFileSync(
      path.join(
        directory,
        "target/aarch64-apple-darwin/release/bundle/macos/FixKey_0.1.6_aarch64.dmg",
      ),
      "utf8",
    ),
    "installer",
  );
  return { ...result, attempts: attempts.length };
}

test("package build succeeds without retry", (t) => {
  const result = run(t, [{ code: 0, message: "built" }]);
  assert.equal(result.status, 0);
  assert.equal(result.attempts, 1);
});

test("macOS DMG creation recovers on the third attempt and removes partial images", (t) => {
  const result = run(t, [
    { code: 7, message: busy },
    { code: 8, message: busy },
    { code: 0, message: "built" },
  ]);
  assert.equal(result.status, 0);
  assert.equal(result.attempts, 3);
  assert.match(result.stdout, /"event":"dmg-create-retry"/u);
});

test("persistent DMG creation failure returns the final build exit code", (t) => {
  const result = run(t, [
    { code: 7, message: busy },
    { code: 8, message: busy },
    { code: 9, message: busy },
  ]);
  assert.equal(result.status, 9);
  assert.equal(result.attempts, 3);
});

test("unrelated failures stop immediately, including after a retry", (t) => {
  const failure = { code: 23, message: "notarization rejected" };
  const immediate = run(t, [failure]);
  assert.equal(immediate.status, 23);
  assert.equal(immediate.attempts, 1);
  const afterRetry = run(t, [{ code: 7, message: busy }, failure]);
  assert.equal(afterRetry.status, 23);
  assert.equal(afterRetry.attempts, 2);
});

test("Windows and Linux failures are never retried", (t) => {
  for (const platform of ["Windows", "Linux"]) {
    const result = run(t, [{ code: 17, message: busy }], platform);
    assert.equal(result.status, 17);
    assert.equal(result.attempts, 1);
  }
});

test("release workflow routes the complete signed build through the retry wrapper", () => {
  const workflow = yaml.load(
    readFileSync(
      new URL(
        "../../../../.github/workflows/release-fixkey.yml",
        import.meta.url,
      ),
      "utf8",
    ),
  );
  const step = workflow.jobs.build.steps.find(
    (entry) => entry.name === "Build installers and signed updater payloads",
  );
  assert.match(
    step.run,
    /bash apps\/fixkey-app\/scripts\/release\/fixkey-build\.sh pnpm --filter fixkey-desktop exec tauri build --features desktop --target/u,
  );
  assert.match(step.run, /--bundles '\$\{\{ matrix.bundles \}\}' --config/u);
  assert.equal(
    step.env.TAURI_SIGNING_PRIVATE_KEY,
    "${{ secrets.FIXKEY_UPDATER_PRIVATE_KEY }}",
  );
});
