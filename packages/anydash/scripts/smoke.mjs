// SPDX-License-Identifier: Apache-2.0
// Checks the real packaged CLI from `pnpm build` on this host: archive
// checksums and contents, an isolated npm installation of the packed launcher
// and platform packages, the direct archive and the install script, each
// running `--version` and `--help`. Nothing is published.
import assert from "node:assert/strict";
import { execFileSync, spawn, spawnSync } from "node:child_process";
import {
  cpSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import {
  executableName,
  platformKey,
  platformPackage,
  platforms,
  verifiedExecutable,
} from "../bin/verify.mjs";
import {
  archiveName,
  launcherArchiveName,
  packageRoot,
  platformArchiveName,
  sha256File,
  verifyChecksums,
} from "./package.mjs";
import { installCandidate } from "./packed-install.mjs";
import { release, serveReleases } from "./release-fixture.mjs";

const dist = resolve(packageRoot, "dist");
const build = JSON.parse(readFileSync(resolve(dist, "build.json"), "utf8"));
const { version } = build;
const host = platformKey();
const selected = Object.keys(build.artifacts);
assert.ok(selected.length && selected.every((key) => platforms.includes(key)));
assert.ok(
  selected.includes(host),
  `the build must include the host platform ${host}`,
);
const temporary = mkdtempSync(resolve(tmpdir(), "anydash-smoke-"));
const env = { ...process.env, ANYDASH_HOME: resolve(temporary, "home") };
const checks = [];

function runAsync(file, args, childEnv) {
  return new Promise((done, reject) => {
    const child = spawn(file, args, {
      env: childEnv,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (data) => (stdout += data));
    child.stderr.on("data", (data) => (stderr += data));
    child.once("error", reject);
    child.once("exit", (status) => done({ status, stdout, stderr }));
  });
}

function execute(file, prefix, argument, label) {
  const result = spawnSync(file, [...prefix, argument], {
    encoding: "utf8",
    env,
    input: "",
  });
  assert.equal(result.status, 0, `${label} ${argument}: ${result.stderr}`);
  checks.push({ command: `${label} ${argument}`, result: "passed" });
  return result.stdout;
}
function expectCli(file, label, prefix = []) {
  assert.equal(
    execute(file, prefix, "--version", label).trim(),
    `anydash ${version}`,
  );
  assert.match(execute(file, prefix, "--help", label), /^Usage: anydash/m);
}

try {
  const sums = verifyChecksums(dist);
  assert.deepEqual(
    [...sums.keys()].sort(),
    [
      ...selected.flatMap((key) => [
        archiveName(key, version),
        platformArchiveName(key, version),
      ]),
      launcherArchiveName(version),
    ].sort(),
    "SHA256SUMS must cover exactly the built archives",
  );
  const launcherEntries = execFileSync(
    "tar",
    ["-tzf", launcherArchiveName(version)],
    { cwd: dist, encoding: "utf8" },
  );
  assert.doesNotMatch(
    launcherEntries,
    /package\/anydash(?:\.exe)?\n|THIRD_PARTY_NOTICES/,
    "the launcher package carries no executables",
  );
  for (const key of selected) {
    const entry = build.artifacts[key];
    const extracted = resolve(temporary, `archive-${key}`);
    mkdirSync(extracted);
    execFileSync("tar", [
      "-xzf",
      resolve(dist, archiveName(key, version)),
      "-C",
      extracted,
    ]);
    const root = resolve(extracted, `anydash-${key}`);
    assert.deepEqual(readdirSync(extracted), [`anydash-${key}`]);
    assert.equal(sha256File(resolve(root, executableName(key))), entry.sha256);
    assert.equal(
      JSON.parse(readFileSync(resolve(root, "manifest.json"), "utf8"))
        .sourceRevision,
      build.sourceRevision,
    );
    for (const notice of [
      "LICENSE",
      "NOTICE",
      "SHA256SUMS",
      "THIRD_PARTY_NOTICES/modules.json",
    ])
      assert.ok(lstatSync(resolve(root, notice)).isFile(), `missing ${notice}`);
    const packed = resolve(temporary, `npm-${key}`);
    mkdirSync(packed);
    execFileSync("tar", [
      "-xzf",
      resolve(dist, platformArchiveName(key, version)),
      "-C",
      packed,
    ]);
    const metadata = JSON.parse(
      readFileSync(resolve(packed, "package/package.json"), "utf8"),
    );
    const info = platformPackage(key);
    assert.equal(metadata.name, info.name);
    assert.equal(metadata.version, version);
    assert.deepEqual(metadata.os, [info.os]);
    assert.deepEqual(metadata.cpu, [info.cpu]);
    assert.equal(
      sha256File(resolve(packed, "package", entry.path)),
      entry.sha256,
    );
    if (key === host)
      expectCli(resolve(root, executableName(key)), "direct archive");
  }

  const prefix = resolve(temporary, "npm");
  await installCandidate({ dist, version, keys: selected, prefix });
  assert.deepEqual(
    readdirSync(resolve(prefix, "node_modules/@delinoio")).sort(),
    ["anydash", `anydash-${host}`].sort(),
    "npm installs only the host platform package",
  );
  const launcher = resolve(prefix, "node_modules/@delinoio/anydash");
  verifiedExecutable(launcher);
  expectCli(process.execPath, "npm launcher", [
    resolve(launcher, "bin/anydash.mjs"),
  ]);

  // The install script runs unchanged against a local copy of the release.
  const releaseDirectory = resolve(temporary, `anydash@v${version}`);
  mkdirSync(releaseDirectory);
  for (const name of sums.keys())
    cpSync(resolve(dist, name), resolve(releaseDirectory, name));
  cpSync(resolve(dist, "SHA256SUMS"), resolve(releaseDirectory, "SHA256SUMS"));
  const server = await serveReleases({
    releases: [release(`anydash@v${version}`)],
    directories: { [`anydash@v${version}`]: releaseDirectory },
  });
  try {
    const target = resolve(temporary, "installed");
    const script = resolve(
      packageRoot,
      "install",
      host.startsWith("windows-") ? "install.ps1" : "install.sh",
    );
    // Run asynchronously: the release fixture serves from this event loop.
    const run = host.startsWith("windows-")
      ? await runAsync(
          "powershell.exe",
          [
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            script,
          ],
          {
            ...env,
            ...server.env,
            ANYDASH_INSTALL_DIR: target,
            ANYDASH_NO_MODIFY_PATH: "1",
          },
        )
      : await runAsync("sh", [script, "--install-dir", target], {
          ...env,
          ...server.env,
        });
    assert.equal(run.status, 0, `install script: ${run.stdout}${run.stderr}`);
    checks.push({ command: `install script ${host}`, result: "passed" });
    expectCli(resolve(target, executableName(host)), "installed executable");
  } finally {
    await server.close();
  }
  const record = {
    schemaVersion: 1,
    status: "passed",
    version,
    sourceRevision: build.sourceRevision,
    platform: host,
    host: { platform: process.platform, arch: process.arch },
    checks,
    archives: Object.fromEntries(sums),
  };
  writeFileSync(
    resolve(dist, `smoke-${host}.json`),
    `${JSON.stringify(record, null, 2)}\n`,
  );
  console.info(
    JSON.stringify({
      event: "anydash_smoke_passed",
      platform: host,
      checks: checks.length,
    }),
  );
} finally {
  rmSync(temporary, { recursive: true, force: true });
}
