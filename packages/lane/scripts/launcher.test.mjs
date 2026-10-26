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
  readFileSync,
  readdirSync,
  realpathSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import {
  platformKey,
  verifiedExecutable,
  platforms,
  platformPackage,
} from "../bin/verify.mjs";
import { installCandidate } from "./packed-install.mjs";
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
      files: ["bin", "manifest.json"],
      optionalDependencies: Object.fromEntries(
        platforms.map((key) => [platformPackage(key).name, "0.1.0"]),
      ),
    }),
  );
  const key = platformKey();
  const path = key.startsWith("windows") ? "lane.exe" : "lane";
  const platformRoot = resolve(root, "node_modules", platformPackage(key).name);
  const bytes = Buffer.from(
    '#!/usr/bin/env node\nprocess.stdout.write(JSON.stringify(process.argv.slice(2))+"\\n");process.stderr.write("fixture stderr\\n");if(process.argv.includes("wait")){process.on("SIGTERM",()=>{process.stderr.write("terminated\\n");process.exit(23)});setInterval(()=>{},1000)}else{process.stdin.pipe(process.stdout);process.stdin.on("end",()=>{process.exitCode=7})}\n',
  );
  mkdirSync(platformRoot, { recursive: true });
  const info = platformPackage(key);
  writeFileSync(
    resolve(platformRoot, "package.json"),
    JSON.stringify({
      name: info.name,
      version: "0.1.0",
      os: [info.os],
      cpu: [info.cpu],
    }),
  );
  writeFileSync(resolve(platformRoot, path), bytes, { mode: 0o755 });
  const manifest = {
    schemaVersion: 1,
    sourceRevision: "a".repeat(40),
    launcherRevision: "b".repeat(40),
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
  return {
    root,
    key,
    manifest,
    save,
    file: resolve(platformRoot, path),
    platformRoot,
  };
}
test("manifest rejects unsupported, absent, wrong version/platform, traversal and checksum corruption", (t) => {
  const f = fixture(t);
  assert.equal(verifiedExecutable(f.root), realpathSync(f.file));
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
  "packed candidates install only the host binary and fail clearly when optional dependencies are omitted",
  { skip: process.platform === "win32" },
  async (t) => {
    const f = fixture(t);
    const pack = (cwd) => {
      const result = spawnSync(
        "npm",
        ["pack", "--ignore-scripts", "--pack-destination", f.root, "--json"],
        { cwd, encoding: "utf8" },
      );
      assert.equal(result.status, 0, result.stderr);
      return JSON.parse(result.stdout)[0];
    };
    const entry = pack(f.root);
    assert.ok(
      entry.files.every(
        ({ path }) =>
          path === "package.json" ||
          path === "manifest.json" ||
          path.startsWith("bin/"),
      ),
    );
    for (const key of platforms) {
      const info = platformPackage(key);
      const dir = resolve(f.root, "candidates", key);
      mkdirSync(dir, { recursive: true });
      writeFileSync(
        resolve(dir, "package.json"),
        JSON.stringify({
          name: info.name,
          version: "0.1.0",
          os: [info.os],
          cpu: [info.cpu],
        }),
      );
      cpSync(
        f.file,
        resolve(dir, key.startsWith("windows") ? "lane.exe" : "lane"),
      );
      pack(dir);
    }
    const install = resolve(f.root, "install");
    await installCandidate({
      dist: f.root,
      version: "0.1.0",
      keys: platforms,
      prefix: install,
    });
    assert.deepEqual(
      readdirSync(resolve(install, "node_modules/@delinoio")).sort(),
      ["lane", `lane-${f.key}`].sort(),
    );
    const run = spawnSync(
      process.execPath,
      [
        resolve(install, "node_modules/@delinoio/lane/bin/lane.mjs"),
        "--version",
      ],
      { input: "", encoding: "utf8" },
    );
    assert.equal(run.status, 7, run.stderr);
    assert.match(run.stdout, /--version/);
    for (const key of platforms.filter((key) => key !== f.key)) {
      const prefix = resolve(f.root, `install-${key}`);
      await installCandidate({
        dist: f.root,
        version: "0.1.0",
        keys: platforms,
        prefix,
        targetKey: key,
      });
      assert.deepEqual(
        readdirSync(resolve(prefix, "node_modules/@delinoio")).sort(),
        ["lane", `lane-${key}`].sort(),
      );
      assert.deepEqual(
        readFileSync(
          verifiedExecutable(
            resolve(prefix, "node_modules/@delinoio/lane"),
            key,
          ),
        ),
        readFileSync(f.file),
      );
    }
    const globalPrefix = resolve(f.root, "global");
    await installCandidate({
      dist: f.root,
      version: "0.1.0",
      keys: platforms,
      prefix: globalPrefix,
      global: true,
    });
    const globalRun = spawnSync(
      resolve(globalPrefix, "bin/lane"),
      ["--version"],
      { input: "", encoding: "utf8" },
    );
    assert.equal(globalRun.status, 7, globalRun.stderr);
    assert.match(globalRun.stdout, /--version/);
    const omitted = mkdtempSync(resolve(tmpdir(), "lane-omitted-"));
    t.after(() => rmSync(omitted, { recursive: true, force: true }));
    await installCandidate({
      dist: f.root,
      version: "0.1.0",
      keys: platforms,
      prefix: omitted,
      omitOptional: true,
    });
    const failure = spawnSync(
      process.execPath,
      [resolve(omitted, "node_modules/@delinoio/lane/bin/lane.mjs")],
      { encoding: "utf8" },
    );
    assert.equal(failure.status, 1);
    assert.match(failure.stderr, /optional dependencies enabled/);
  },
);

test(
  "preserves signal termination when the child does not handle SIGINT or SIGHUP",
  { skip: process.platform === "win32", timeout: 10000 },
  async (t) => {
    const f = fixture(t);
    for (const signal of ["SIGINT", "SIGHUP"]) {
      const child = spawn(
        process.execPath,
        [resolve(f.root, "bin/lane.mjs"), "wait"],
        { stdio: ["ignore", "pipe", "pipe"] },
      );
      t.after(() => {
        if (child.exitCode === null && child.signalCode === null)
          child.kill("SIGKILL");
      });
      const exit = new Promise((done, reject) => {
        child.once("error", reject);
        child.once("exit", (code, received) =>
          done({ code, signal: received }),
        );
      });
      await new Promise((done) => child.stderr.once("data", done));
      child.kill(signal);
      assert.deepEqual(await exit, { code: null, signal });
    }
  },
);

test("rejects mismatched platform metadata and dependency versions", (t) => {
  const f = fixture(t);
  const path = resolve(f.platformRoot, "package.json");
  const metadata = JSON.parse(readFileSync(path));
  for (const [field, value] of [
    ["name", "wrong"],
    ["version", "0.2.0"],
    ["os", ["freebsd"]],
    ["cpu", ["riscv64"]],
  ]) {
    writeFileSync(path, JSON.stringify({ ...metadata, [field]: value }));
    assert.throws(
      () => verifiedExecutable(f.root),
      /platform package mismatch/,
    );
  }
  writeFileSync(path, JSON.stringify(metadata));
  const pkgPath = resolve(f.root, "package.json");
  const pkg = JSON.parse(readFileSync(pkgPath));
  pkg.optionalDependencies[platformPackage(f.key).name] = "^0.1.0";
  writeFileSync(pkgPath, JSON.stringify(pkg));
  assert.throws(
    () => verifiedExecutable(f.root),
    /invalid optional dependency/,
  );
});
