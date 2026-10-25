// SPDX-License-Identifier: Apache-2.0
// Exercise real release archives in a disposable native installation. This checks
// executable/launcher compatibility, not profiler, account, or registry acceptance.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import {
  mkdtempSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import {
  root,
  sha256,
  archiveNames,
  hostTarget,
  nativeBinary,
  npm,
} from "./build.mjs";

const info = hostTarget();
const release = resolve(
  process.argv[2] ?? join(root, "packages/perfine/dist/release"),
);
// A failed retry must not leave a previous successful smoke record behind.
rmSync(join(release, "smoke-result.json"), { force: true });
const provenance = JSON.parse(
  readFileSync(join(release, "standalone/provenance.json")),
);
assert.equal(
  provenance.target,
  info.target,
  "Smoke must run on the packaged native host",
);
const version = provenance.version;
const names = archiveNames(version, info.target);
const archives = {};
for (const line of readFileSync(join(release, "SHA256SUMS"), "utf8")
  .trim()
  .split("\n")) {
  const [checksum, name, extra] = line.split("  ");
  assert.equal(extra, undefined);
  assert.ok(Object.values(names).includes(name), "Unexpected release archive");
  assert.ok(!Object.hasOwn(archives, name), "Duplicate release checksum");
  assert.equal(sha256(readFileSync(join(release, name))), checksum);
  archives[name] = checksum;
}
assert.deepEqual(Object.keys(archives).sort(), Object.values(names).sort());
const directory = mkdtempSync(join(tmpdir(), "perfine install 한글-"));
try {
  const standalone = join(directory, "standalone");
  mkdirSync(standalone);
  execFileSync("tar", [
    "-xzf",
    join(release, names.standalone),
    "-C",
    standalone,
  ]);
  npm(
    [
      "install",
      "--prefix",
      directory,
      "--offline",
      "--ignore-scripts",
      "--cache",
      join(directory, "empty-cache"),
      "--no-audit",
      "--no-fund",
      join(release, names.entry),
      join(release, names.platform),
    ],
    { stdio: "inherit" },
  );
  for (const installed of [
    standalone,
    join(directory, "node_modules", info.packageName),
  ]) {
    const installedProvenance = JSON.parse(
      readFileSync(join(installed, "provenance.json")),
    );
    assert.deepEqual(installedProvenance, provenance);
    const hash = sha256(
      nativeBinary(join(installed, "runtime", info.executable), info.target),
    );
    assert.equal(installedProvenance.binarySha256, hash);
    assert.equal(
      readFileSync(join(installed, "runtime/SHA256SUMS"), "utf8"),
      `${hash}  ${info.executable}\n`,
    );
    for (const name of ["LICENSE", "NOTICE", "THIRD_PARTY_NOTICES"])
      assert.ok(readFileSync(join(installed, name)).length);
  }
  const entryRoot = join(directory, "node_modules/perfine");
  const entryProvenance = JSON.parse(
    readFileSync(join(entryRoot, "provenance.json")),
  );
  assert.equal(entryProvenance.sourceCommit, provenance.sourceCommit);
  assert.equal(entryProvenance.version, version);
  for (const name of ["LICENSE", "NOTICE", "THIRD_PARTY_NOTICES"])
    assert.ok(readFileSync(join(entryRoot, name)).length);
  if (process.platform === "win32") {
    // Exercise npm's generated Windows shim as well as the archived launcher.
    // Pass its path as data; do not interpolate user-directory bytes into code.
    const output = execFileSync(
      "powershell.exe",
      [
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "& $env:PERFINE_SMOKE_SHIM --version; exit $LASTEXITCODE",
      ],
      {
        encoding: "utf8",
        env: {
          ...process.env,
          PERFINE_SMOKE_SHIM: join(directory, "node_modules/.bin/perfine.cmd"),
        },
      },
    ).trim();
    assert.equal(output, `perfine ${version}`);
  }
  for (const [kind, binary, prefix] of [
    ["standalone", join(standalone, "runtime", info.executable), []],
    process.platform === "win32"
      ? ["npm", process.execPath, [join(entryRoot, "bin/perfine.cjs")]]
      : ["npm", join(directory, "node_modules/.bin/perfine"), []],
  ]) {
    const output = execFileSync(binary, [...prefix, "--version"], {
      encoding: "utf8",
    }).trim();
    assert.equal(output, `perfine ${version}`);
    const help = execFileSync(binary, [...prefix, "measure", "--help"], {
      encoding: "utf8",
    });
    assert.match(help, /--experiment/);
    assert.match(help, /--repository/);
    assert.match(help, /--config/);
    // An empty installation has no workload configuration. Require the real
    // structured doctor diagnostic; do not claim successful profiler discovery.
    const diagnostic = spawnSync(
      binary,
      [
        ...prefix,
        "--repository",
        directory,
        "--config",
        "missing.yaml",
        "--json",
        "doctor",
      ],
      { encoding: "utf8" },
    );
    assert.equal(diagnostic.status, 1, diagnostic.stderr);
    const response = JSON.parse(diagnostic.stdout);
    assert.equal(response.schema_version, 1);
    assert.ok(response.error.message.length);
    console.log(
      JSON.stringify({
        event: "perfine_install_smoke",
        kind,
        version: output,
        doctor: "expected_missing_configuration",
        sourceCommit: JSON.parse(
          readFileSync(join(standalone, "provenance.json")),
        ).sourceCommit,
      }),
    );
  }
  writeFileSync(
    join(release, "smoke-result.json"),
    `${JSON.stringify({ schemaVersion: 1, sourceCommit: provenance.sourceCommit, version, target: info.target, platform: process.platform, arch: process.arch, status: "passed", archives }, null, 2)}\n`,
  );
} finally {
  rmSync(directory, { recursive: true, force: true });
}
