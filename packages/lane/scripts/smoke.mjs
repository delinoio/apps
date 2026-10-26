// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { execFileSync, spawnSync } from "node:child_process";
import {
  mkdtempSync,
  readFileSync,
  rmSync,
  mkdirSync,
  lstatSync,
  readdirSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  verifiedExecutable,
  platformKey,
  platforms,
  platformPackage,
} from "../bin/verify.mjs";
import { installCandidate } from "./packed-install.mjs";
const source = fileURLToPath(new URL("../", import.meta.url));
const { version } = JSON.parse(readFileSync(resolve(source, "package.json")));
const temporary = mkdtempSync(resolve(tmpdir(), "lane-packed-"));
try {
  const manifest = JSON.parse(
    readFileSync(resolve(source, "dist/package/manifest.json")),
  );
  const selectedPlatforms = Object.keys(manifest.artifacts);
  assert.ok(
    selectedPlatforms.length &&
      selectedPlatforms.every((key) => platforms.includes(key)),
  );
  const expectedArchives = new Set(
    selectedPlatforms
      .flatMap((key) => [
        `lane-${key}-${version}.tar.gz`,
        `delinoio-lane-${key}-${version}.tgz`,
      ])
      .concat(`delinoio-lane-${version}.tgz`),
  );
  const sums = readFileSync(resolve(source, "dist/SHA256SUMS"), "utf8")
    .trim()
    .split("\n");
  for (const line of sums) {
    const match = /^([a-f0-9]{64})  ([a-z0-9.@-]+)$/.exec(line);
    assert.ok(match, "invalid archive checksum entry");
    const [, checksum, name] = match;
    assert.ok(expectedArchives.delete(name), "unexpected or duplicate archive");
    assert.equal(
      createHash("sha256")
        .update(readFileSync(resolve(source, "dist", name)))
        .digest("hex"),
      checksum,
    );
  }
  assert.equal(expectedArchives.size, 0, "missing archive checksum");
  await installCandidate({
    dist: resolve(source, "dist"),
    version,
    keys: selectedPlatforms,
    prefix: temporary,
  });
  assert.deepEqual(
    readdirSync(resolve(temporary, "node_modules/@delinoio")).sort(),
    ["lane", `lane-${platformKey()}`].sort(),
  );
  const launcherEntries = execFileSync(
    "tar",
    ["-tzf", resolve(source, "dist", `delinoio-lane-${version}.tgz`)],
    { encoding: "utf8" },
  );
  assert.doesNotMatch(
    launcherEntries,
    /artifacts\/|package\/lane(?:\.exe)?\n|THIRD_PARTY_NOTICES/,
  );
  const installed = resolve(temporary, "node_modules/@delinoio/lane");
  for (const key of selectedPlatforms) {
    const executable = resolve(
      source,
      "dist/platforms",
      `lane-${key}`,
      key.startsWith("windows") ? "lane.exe" : "lane",
    );
    const info = platformPackage(key);
    const metadata = JSON.parse(
      readFileSync(
        resolve(source, "dist/platforms", `lane-${key}`, "package.json"),
      ),
    );
    assert.equal(metadata.name, info.name);
    assert.equal(metadata.version, version);
    assert.deepEqual(metadata.os, [info.os]);
    assert.deepEqual(metadata.cpu, [info.cpu]);
    const entry = manifest.artifacts[key];
    const bytes = readFileSync(executable);
    assert.equal(entry.size, bytes.length);
    assert.equal(
      entry.sha256,
      createHash("sha256").update(bytes).digest("hex"),
    );
    const packedPlatform = resolve(temporary, `packed-${key}`);
    mkdirSync(packedPlatform);
    execFileSync("tar", [
      "-xzf",
      resolve(source, "dist", `delinoio-lane-${key}-${version}.tgz`),
      "-C",
      packedPlatform,
    ]);
    assert.deepEqual(
      readFileSync(resolve(packedPlatform, "package", entry.path)),
      bytes,
    );
    for (const notice of [
      "LICENSE",
      "NOTICE",
      "THIRD_PARTY_NOTICES/modules.json",
    ])
      assert.ok(lstatSync(resolve(packedPlatform, "package", notice)).isFile());
    const archive = resolve(source, "dist", `lane-${key}-${version}.tar.gz`);
    const entries = execFileSync("tar", ["-tzf", archive], { encoding: "utf8" })
      .trim()
      .split("\n");
    assert.ok(
      entries.every(
        (name) =>
          name.startsWith(`lane-${key}/`) &&
          !name.split("/").includes("..") &&
          !name.startsWith("/"),
      ),
    );
    for (const required of [
      "LICENSE",
      "NOTICE",
      "manifest.json",
      "SHA256SUMS",
      "THIRD_PARTY_NOTICES/modules.json",
    ])
      assert.ok(entries.includes(`lane-${key}/${required}`), required);
    const extracted = resolve(temporary, key);
    mkdirSync(extracted);
    execFileSync("tar", ["-xzf", archive, "-C", extracted]);
    const binary = resolve(
      extracted,
      `lane-${key}`,
      key.startsWith("windows") ? "lane.exe" : "lane",
    );
    assert.ok(lstatSync(binary).isFile());
    assert.deepEqual(readFileSync(binary), readFileSync(executable));
    const manifest = JSON.parse(
      readFileSync(resolve(extracted, `lane-${key}`, "manifest.json")),
    );
    assert.equal(manifest.version, version);
    assert.equal(manifest.platform, key);
    assert.equal(
      manifest.path,
      key.startsWith("windows") ? "lane.exe" : "lane",
    );
  }
  for (const [command, prefix] of [
    [process.execPath, [resolve(installed, "bin/lane.mjs")]],
    [verifiedExecutable(installed), []],
  ]) {
    const run = spawnSync(command, [...prefix, "--version"], {
      encoding: "utf8",
    });
    assert.equal(run.status, 0, run.stderr);
    assert.equal(run.stdout.trim(), `lane ${version}`);
    const help = spawnSync(command, [...prefix, "--help"], {
      encoding: "utf8",
    });
    assert.equal(help.status, 0, help.stderr);
    // Initialization and discovery do not open or mutate the user's local store.
    const requests = [
      {
        jsonrpc: "2.0",
        id: 1,
        method: "initialize",
        params: {
          protocolVersion: "2025-06-18",
          capabilities: {},
          clientInfo: { name: "lane-package-smoke", version },
        },
      },
      { jsonrpc: "2.0", method: "notifications/initialized" },
      { jsonrpc: "2.0", id: 2, method: "tools/list" },
    ];
    const mcp = spawnSync(command, [...prefix, "mcp", "serve"], {
      input: requests.map((r) => JSON.stringify(r)).join("\n") + "\n",
      encoding: "utf8",
      timeout: 10000,
    });
    assert.equal(mcp.status, 0, mcp.stderr);
    const messages = mcp.stdout
      .trim()
      .split("\n")
      .map((line) => JSON.parse(line));
    assert.equal(messages[0].result.protocolVersion, "2025-06-18");
    assert.equal(messages[0].result.serverInfo.version, version);
    assert.ok(
      messages
        .find((message) => message.id === 2)
        .result.tools.some((tool) => tool.name === "lane_create_task"),
    );
  }
  console.log(
    `Real packed Lane ${version} version/help/MCP initialize+discovery verified on ${platformKey()}; other platforms require native execution.`,
  );
} finally {
  rmSync(temporary, { recursive: true, force: true });
}
