// SPDX-License-Identifier: Apache-2.0
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  cpSync,
  mkdtempSync,
  mkdirSync,
  writeFileSync,
  rmSync,
  symlinkSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { platformKey, verifiedExecutable } from "../bin/verify.mjs";
function fixture(t) {
  const root = mkdtempSync(resolve(tmpdir(), "lane-launcher-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  cpSync(new URL("../bin", import.meta.url), resolve(root, "bin"), {
    recursive: true,
  });
  writeFileSync(
    resolve(root, "package.json"),
    JSON.stringify({
      name: "@delinoio/lane",
      version: "0.1.0",
      type: "module",
      bin: { lane: "bin/lane.mjs" },
    }),
  );
  const key = platformKey();
  const path = `artifacts/lane-${key}/${key.startsWith("windows") ? "lane.exe" : "lane"}`;
  const bytes = Buffer.from(
    '#!/usr/bin/env node\nprocess.stdout.write(JSON.stringify(process.argv.slice(2))+"\\n");process.stderr.write("fixture stderr\\n");if(process.argv.includes("wait")){process.on("SIGTERM",()=>{process.stderr.write("terminated\\n");process.exit(23)});setInterval(()=>{},1000)}else{process.stdin.pipe(process.stdout);process.stdin.on("end",()=>{process.exitCode=7})}\n',
  );
  mkdirSync(resolve(root, "artifacts", `lane-${key}`), { recursive: true });
  writeFileSync(resolve(root, path), bytes, { mode: 0o755 });
  const manifest = {
    schemaVersion: 1,
    version: "0.1.0",
    artifacts: {
      [key]: {
        platform: key,
        version: "0.1.0",
        path,
        size: bytes.length,
        sha256: createHash("sha256").update(bytes).digest("hex"),
      },
    },
  };
  const save = () =>
    writeFileSync(resolve(root, "manifest.json"), JSON.stringify(manifest));
  save();
  return { root, key, manifest, save, file: resolve(root, path) };
}
test("manifest rejects unsupported, absent, wrong version/platform, traversal and checksum corruption", (t) => {
  const f = fixture(t);
  assert.equal(verifiedExecutable(f.root), f.file);
  assert.throws(() => platformKey("freebsd", "x64"), /unsupported/);
  assert.throws(
    () => verifiedExecutable(f.root, "linux-riscv64"),
    /unsupported/,
  );
  const entry = f.manifest.artifacts[f.key];
  for (const [field, value, pattern] of [
    ["path", "../../outside", /invalid/],
    ["platform", "other", /invalid/],
    ["version", "9.0.0", /invalid/],
    ["sha256", "0".repeat(64), /checksum/],
    ["size", 1, /checksum/],
  ]) {
    const prior = entry[field];
    entry[field] = value;
    f.save();
    assert.throws(() => verifiedExecutable(f.root), pattern);
    entry[field] = prior;
  }
  f.manifest.version = "0.2.0";
  f.save();
  assert.throws(() => verifiedExecutable(f.root), /version/);
  f.manifest.version = "0.1.0";
  delete f.manifest.artifacts[f.key];
  f.save();
  assert.throws(() => verifiedExecutable(f.root), /missing/);
  f.manifest.artifacts[f.key] = entry;
  f.save();
  rmSync(f.file);
  assert.throws(() => verifiedExecutable(f.root), /ENOENT/);
  symlinkSync(process.execPath, f.file);
  assert.throws(() => verifiedExecutable(f.root), /regular/);
});
test(
  "synthetic executable preserves argv, stdin, stdout, stderr and exit status",
  { skip: process.platform === "win32" },
  (t) => {
    const f = fixture(t);
    const result = spawnSync(
      process.execPath,
      [resolve(f.root, "bin/lane.mjs"), "space value", "--json"],
      { input: "input\n", encoding: "utf8" },
    );
    assert.equal(result.status, 7);
    assert.equal(result.stdout, '["space value","--json"]\ninput\n');
    assert.equal(result.stderr, "fixture stderr\n");
  },
);
test(
  "forwards SIGTERM to synthetic executable",
  { skip: process.platform === "win32", timeout: 10000 },
  async (t) => {
    const f = fixture(t);
    const child = spawn(
      process.execPath,
      [resolve(f.root, "bin/lane.mjs"), "wait"],
      { stdio: ["ignore", "pipe", "pipe"] },
    );
    t.after(() => {
      if (child.exitCode === null) child.kill("SIGKILL");
    });
    let output = "";
    const result = new Promise((done, reject) => {
      child.on("error", reject);
      child.on("exit", (code, signal) => done({ code, signal }));
    });
    await new Promise((done) =>
      child.stderr.once("data", (data) => {
        output += data;
        done();
      }),
    );
    child.stderr.on("data", (data) => {
      output += data;
    });
    // The fixture installs handlers before its event loop can receive a signal.
    child.kill("SIGTERM");
    assert.deepEqual(await result, { code: 23, signal: null });
    assert.match(output, /terminated/);
  },
);
test(
  "packed fixture installs offline and launches from a disposable directory",
  { skip: process.platform === "win32" },
  (t) => {
    const f = fixture(t);
    const packed = spawnSync("npm", ["pack", "--ignore-scripts", "--json"], {
      cwd: f.root,
      encoding: "utf8",
    });
    assert.equal(packed.status, 0, packed.stderr);
    const install = resolve(f.root, "install");
    mkdirSync(install);
    const filename = JSON.parse(packed.stdout)[0].filename;
    const result = spawnSync(
      "npm",
      [
        "install",
        "--offline",
        "--ignore-scripts",
        "--no-audit",
        "--no-fund",
        "--prefix",
        install,
        resolve(f.root, filename),
      ],
      { encoding: "utf8" },
    );
    assert.equal(result.status, 0, result.stderr);
    const run = spawnSync(
      process.execPath,
      [
        resolve(install, "node_modules/@delinoio/lane/bin/lane.mjs"),
        "--version",
      ],
      { input: "", encoding: "utf8" },
    );
    assert.equal(run.status, 7);
    assert.match(run.stdout, /--version/);
  },
);
