// SPDX-License-Identifier: Apache-2.0
// Publication consumes a complete, checked set; failed or partial releases need
// operator reconciliation and must never replace existing immutable artifacts.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import {
  cpSync,
  existsSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { release as osRelease } from "node:os";
import { fileURLToPath } from "node:url";
import { sha256, commit, version } from "./build.mjs";

export const releaseTargets = [
  {
    target: "x86_64-unknown-linux-gnu",
    platform: "linux",
    arch: "x64",
    suffix: "linux-x64",
    archive: "linux-x86_64",
  },
  {
    target: "x86_64-apple-darwin",
    platform: "darwin",
    arch: "x64",
    suffix: "darwin-x64",
    archive: "darwin-x86_64",
  },
  {
    target: "aarch64-apple-darwin",
    platform: "darwin",
    arch: "arm64",
    suffix: "darwin-arm64",
    archive: "darwin-arm64",
  },
  {
    target: "x86_64-pc-windows-msvc",
    platform: "win32",
    arch: "x64",
    suffix: "win32-x64",
    archive: "win32-x86_64",
  },
];
const json = (path) => JSON.parse(readFileSync(path, "utf8"));
const writeJson = (path, value) =>
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`);
export function expectedArchives(v, item) {
  version(v);
  return [
    `perfine-${v}.tgz`,
    `delinoio-perfine-${item.suffix}-${v}.tgz`,
    `perfine-v${v}-${item.archive}.tar.gz`,
  ];
}
export function checkedArchives(directory, names) {
  const entries = readFileSync(join(directory, "SHA256SUMS"), "utf8")
    .trim()
    .split("\n");
  const result = {};
  for (const line of entries) {
    const match = /^([a-f0-9]{64})  ([a-zA-Z0-9_.-]+)$/.exec(line);
    assert.ok(
      match && names.includes(match[2]),
      "Unexpected release checksum entry",
    );
    const [, hash, name] = match;
    assert.equal(result[name], undefined, "Duplicate release checksum entry");
    assert.equal(
      sha256(readFileSync(join(directory, name))),
      hash,
      `Archive checksum mismatch: ${name}`,
    );
    result[name] = hash;
  }
  assert.deepEqual(
    Object.keys(result).sort(),
    [...names].sort(),
    "Incomplete release archive set",
  );
  return result;
}
export function minimumEvidence({
  platform,
  arch,
  version: observed,
  windowsCaption = "",
  libc = "",
}) {
  let verified = false;
  let required;
  if (platform === "linux") {
    required = "Linux x86_64, glibc 2.39";
    verified = arch === "x64" && libc === "2.39";
  } else if (platform === "darwin") {
    required = "macOS 15";
    verified = ["x64", "arm64"].includes(arch) && /^15\./.test(observed);
  } else if (platform === "win32") {
    required = "Windows 11 25H2, build 26200 family";
    verified =
      arch === "x64" &&
      /Windows 11/.test(windowsCaption) &&
      /^10\.0\.26200(?:\.|$)/.test(observed);
  } else throw new Error("Unsupported verification host");
  return {
    status: verified ? "verified" : "unverified",
    required,
    observed,
    libc,
    windowsCaption,
    scope:
      "package installation and executable smoke only; current patch compliance is operator-owned",
  };
}
export function recordHost(directory) {
  const smoke = json(join(directory, "smoke-result.json"));
  const item = releaseTargets.find((item) => item.target === smoke.target);
  assert.ok(
    item && item.platform === process.platform && item.arch === process.arch,
    "Native smoke target mismatch",
  );
  assert.equal(smoke.status, "passed");
  const host = {
    platform: process.platform,
    arch: process.arch,
    version: osRelease(),
  };
  if (process.platform === "darwin")
    host.version = execFileSync("sw_vers", ["-productVersion"], {
      encoding: "utf8",
    }).trim();
  if (process.platform === "linux")
    host.libc =
      process.report.getReport().header.glibcVersionRuntime ?? "unknown";
  if (process.platform === "win32")
    host.windowsCaption = execFileSync(
      "powershell.exe",
      [
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "(Get-CimInstance Win32_OperatingSystem).Caption",
      ],
      { encoding: "utf8" },
    ).trim();
  const record = {
    schemaVersion: 1,
    sourceCommit: smoke.sourceCommit,
    version: smoke.version,
    target: smoke.target,
    host,
    minimumOs: minimumEvidence(host),
    dockerWorkflow: "unperformed",
    liveProvider: "unperformed",
  };
  writeJson(join(directory, "validation.json"), record);
  return record;
}
export function assemble(input, output, sourceCommit, releaseVersion) {
  commit(sourceCommit);
  version(releaseVersion);
  assert.ok(
    !existsSync(output),
    "Release set already exists; reconcile it before retrying",
  );
  const verified = releaseTargets.map((item) => {
    const directory = join(input, item.target);
    const hashes = checkedArchives(
      directory,
      expectedArchives(releaseVersion, item),
    );
    const smoke = json(join(directory, "smoke-result.json"));
    const validation = json(join(directory, "validation.json"));
    for (const record of [
      smoke,
      validation,
      json(join(directory, "standalone/provenance.json")),
    ]) {
      assert.equal(record.schemaVersion, 1);
      assert.equal(record.sourceCommit, sourceCommit);
      assert.equal(record.version, releaseVersion);
      assert.equal(record.target, item.target);
    }
    assert.equal(
      smoke.status,
      "passed",
      "Every target must pass executable smoke before publication",
    );
    assert.equal(smoke.platform, item.platform);
    assert.equal(smoke.arch, item.arch);
    assert.deepEqual(
      smoke.archives,
      hashes,
      "Archives changed after smoke verification",
    );
    assert.equal(validation.host.platform, item.platform);
    assert.equal(validation.host.arch, item.arch);
    assert.deepEqual(validation.minimumOs, minimumEvidence(validation.host));
    return { item, directory, hashes, validation };
  });
  const entry = `perfine-${releaseVersion}.tgz`;
  for (const record of verified)
    assert.equal(
      record.hashes[entry],
      verified[0].hashes[entry],
      "Entry npm archives must match on all targets",
    );
  mkdirSync(output, { recursive: true });
  const hashes = {};
  for (const { directory, hashes: archives } of verified) {
    for (const [name, hash] of Object.entries(archives)) {
      cpSync(join(directory, name), join(output, name));
      hashes[name] = hash;
    }
  }
  cpSync(join(verified[0].directory, "action"), join(output, "action"), {
    recursive: true,
  });
  writeFileSync(
    join(output, "SHA256SUMS"),
    Object.entries(hashes)
      .sort()
      .map(([name, hash]) => `${hash}  ${name}\n`)
      .join(""),
  );
  const record = {
    schemaVersion: 1,
    sourceCommit,
    version: releaseVersion,
    archives: hashes,
    npmPackages: [
      ...releaseTargets.map((item) => ({
        name: `@delinoio/perfine-${item.suffix}`,
        archive: `delinoio-perfine-${item.suffix}-${releaseVersion}.tgz`,
      })),
      { name: "perfine", archive: entry },
    ],
    validation: verified.map(({ validation }) => validation),
  };
  writeJson(join(output, "release-set.json"), record);
  return record;
}
export function verifySet(directory) {
  const record = json(join(directory, "release-set.json"));
  commit(record.sourceCommit);
  version(record.version);
  assert.equal(record.schemaVersion, 1);
  const names = [
    ...new Set(
      releaseTargets.flatMap((item) => expectedArchives(record.version, item)),
    ),
  ];
  assert.deepEqual(checkedArchives(directory, names), record.archives);
  assert.deepEqual(record.npmPackages, [
    ...releaseTargets.map((item) => ({
      name: `@delinoio/perfine-${item.suffix}`,
      archive: `delinoio-perfine-${item.suffix}-${record.version}.tgz`,
    })),
    { name: "perfine", archive: `perfine-${record.version}.tgz` },
  ]);
  assert.equal(record.validation.length, releaseTargets.length);
  for (const [i, item] of releaseTargets.entries()) {
    const evidence = record.validation[i];
    assert.equal(evidence.target, item.target);
    assert.equal(evidence.sourceCommit, record.sourceCommit);
    assert.equal(evidence.version, record.version);
    assert.deepEqual(evidence.minimumOs, minimumEvidence(evidence.host));
  }
  return record;
}
// Run only in the Linux publication job, where npm is a native executable.
const npm = (...args) =>
  execFileSync("npm", args, {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  }).trim();
export function assertAuthority(user, members, access, packages) {
  assert.ok(
    user && ["owner", "admin", "developer"].includes(members[user]),
    "Authenticated npm user must belong to @delinoio with publishing authority",
  );
  for (const name of packages)
    assert.equal(
      access[name],
      "read-write",
      `No npm publish authority for ${name}`,
    );
}
export function preflight(directory) {
  const record = verifySet(directory);
  const user = npm("whoami");
  const members = JSON.parse(npm("org", "ls", "delinoio", "--json"));
  const access = JSON.parse(npm("access", "list", "packages", user, "--json"));
  const existing = [];
  for (const { name } of record.npmPackages) {
    const query = spawnSync("npm", ["view", name, "versions", "--json"], {
      encoding: "utf8",
    });
    if (query.error) throw query.error;
    if (query.status === 0) {
      const versions = JSON.parse(query.stdout);
      existing.push(name);
      assert.ok(
        !(Array.isArray(versions) ? versions : [versions]).includes(
          record.version,
        ),
        `Immutable npm version already exists: ${name}@${record.version}; reconcile partial publication manually`,
      );
    } else {
      let error;
      try {
        error = JSON.parse(query.stdout).error;
      } catch {
        /* A non-JSON failure is not proof of availability. */
      }
      assert.equal(
        error?.code,
        "E404",
        `Cannot establish npm package availability for ${name}`,
      );
      assert.ok(
        name.startsWith("@delinoio/"),
        "The entry npm package must already be controlled by the publisher",
      );
    }
  }
  assertAuthority(user, members, access, existing);
  return {
    sourceCommit: record.sourceCommit,
    version: record.version,
    namespaceAuthority: "verified",
    packages: record.npmPackages.map(({ name }) => name),
  };
}
if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  const [command, directory, output] = process.argv.slice(2);
  if (command === "record-host")
    console.log(JSON.stringify(recordHost(directory)));
  else if (command === "assemble")
    console.log(
      JSON.stringify(
        assemble(
          directory,
          output,
          process.env.PERFINE_SOURCE_COMMIT,
          process.env.PERFINE_RELEASE_VERSION,
        ),
      ),
    );
  else if (command === "verify")
    console.log(JSON.stringify(verifySet(directory)));
  else if (command === "preflight")
    console.log(JSON.stringify(preflight(directory)));
  else if (command === "publish-npm") {
    assert.equal(
      process.env.PERFINE_APPROVE_RELEASE,
      "true",
      "Explicit approved release required",
    );
    preflight(directory);
    for (const { archive } of verifySet(directory).npmPackages) {
      execFileSync(
        "npm",
        [
          "publish",
          resolve(directory, archive),
          "--access",
          "public",
          "--provenance",
          "--ignore-scripts",
        ],
        { stdio: "inherit" },
      );
    }
  } else
    throw new Error(
      "Expected record-host, assemble, verify, preflight or publish-npm",
    );
}
