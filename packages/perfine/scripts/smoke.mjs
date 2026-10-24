// SPDX-License-Identifier: Apache-2.0
// Exercise real release archives in a disposable Linux installation. This checks
// executable/launcher compatibility, not profiler, account, or registry acceptance.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { root, sha256 } from "./build.mjs";

assert.equal(process.platform, "linux");
assert.equal(process.arch, "x64");
const release = resolve(
  process.argv[2] ?? join(root, "packages/perfine/dist/release"),
);
const version = JSON.parse(
  readFileSync(join(release, "standalone/provenance.json")),
).version;
for (const line of readFileSync(join(release, "SHA256SUMS"), "utf8")
  .trim()
  .split("\n")) {
  const [checksum, name] = line.split("  ");
  assert.match(name, /^perfine-[\w.-]+\.(?:tgz|tar.gz)$/);
  assert.equal(sha256(readFileSync(join(release, name))), checksum);
}
const directory = mkdtempSync(join(tmpdir(), "perfine-install-"));
try {
  const standalone = join(directory, "standalone");
  mkdirSync(standalone);
  execFileSync("tar", [
    "-xzf",
    join(release, `perfine-v${version}-linux-x86_64.tar.gz`),
    "-C",
    standalone,
  ]);
  execFileSync(
    "npm",
    [
      "install",
      "--prefix",
      directory,
      "--offline",
      "--ignore-scripts",
      "--no-audit",
      "--no-fund",
      join(release, `perfine-${version}.tgz`),
    ],
    { stdio: "inherit" },
  );
  for (const installed of [
    standalone,
    join(directory, "node_modules/perfine"),
  ]) {
    const provenance = JSON.parse(
      readFileSync(join(installed, "provenance.json")),
    );
    const hash = sha256(readFileSync(join(installed, "runtime/perfine")));
    assert.equal(provenance.binarySha256, hash);
    assert.equal(
      readFileSync(join(installed, "runtime/SHA256SUMS"), "utf8"),
      `${hash}  perfine\n`,
    );
    for (const name of ["LICENSE", "NOTICE", "THIRD_PARTY_NOTICES"])
      assert.ok(readFileSync(join(installed, name)).length);
  }
  for (const [kind, binary] of [
    ["standalone", join(standalone, "runtime/perfine")],
    ["npm", join(directory, "node_modules/.bin/perfine")],
  ]) {
    const output = execFileSync(binary, ["--version"], {
      encoding: "utf8",
    }).trim();
    assert.equal(output, `perfine ${version}`);
    const help = execFileSync(binary, ["measure", "--help"], {
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
} finally {
  rmSync(directory, { recursive: true, force: true });
}
