// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  cpSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { test } from "node:test";
import { gunzipSync } from "node:zlib";
import {
  executableName,
  platformKey,
  platformPackage,
  platforms,
  verifiedExecutable,
} from "../bin/verify.mjs";
import { entries, tarGzip } from "./archive.mjs";
import { npmCommand } from "./npm.mjs";
import { installCandidate } from "./packed-install.mjs";

const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

function fixture(t) {
  const root = mkdtempSync(resolve(tmpdir(), "anydash-launcher-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  cpSync(new URL("../bin", import.meta.url), resolve(root, "bin"), {
    recursive: true,
  });
  writeFileSync(
    resolve(root, "package.json"),
    JSON.stringify({
      name: "@delinoio/anydash",
      version: "0.1.0",
      type: "module",
      bin: { anydash: "bin/anydash.mjs" },
      files: ["bin", "manifest.json"],
      optionalDependencies: Object.fromEntries(
        platforms.map((key) => [platformPackage(key).name, "0.1.0"]),
      ),
    }),
  );
  const key = platformKey();
  const path = executableName(key);
  const info = platformPackage(key);
  const platformRoot = resolve(root, "node_modules", info.name);
  const bytes = Buffer.from(
    '#!/usr/bin/env node\nprocess.stdout.write(JSON.stringify(process.argv.slice(2))+"\\n");process.stderr.write("fixture stderr\\n");if(process.argv.includes("wait")){process.on("SIGTERM",()=>{process.stderr.write("terminated\\n");process.exit(23)});setInterval(()=>{},1000)}else{process.stdin.pipe(process.stdout);process.stdin.on("end",()=>{process.exitCode=7})}\n',
  );
  mkdirSync(platformRoot, { recursive: true });
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
    version: "0.1.0",
    artifacts: {
      [key]: {
        platform: key,
        version: "0.1.0",
        path,
        size: bytes.length,
        sha256: sha256(bytes),
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

test("platform keys cover the CLI support matrix", () => {
  assert.deepEqual(
    [
      ["darwin", "arm64"],
      ["darwin", "x64"],
      ["linux", "arm64"],
      ["linux", "x64"],
      ["win32", "arm64"],
      ["win32", "x64"],
    ].map(([os, arch]) => platformKey(os, arch)),
    platforms,
  );
  assert.deepEqual(platformPackage("windows-arm64"), {
    name: "@delinoio/anydash-windows-arm64",
    os: "win32",
    cpu: "arm64",
  });
  assert.equal(executableName("windows-x64"), "anydash.exe");
  assert.equal(executableName("linux-arm64"), "anydash");
  for (const [os, arch] of [
    ["freebsd", "x64"],
    ["linux", "riscv64"],
    ["win32", "ia32"],
    ["linux", "amd64"],
  ])
    assert.throws(() => platformKey(os, arch), /unsupported/);
});

test("manifest rejects unsupported, absent, wrong version/platform, traversal and checksum corruption", (t) => {
  const f = fixture(t);
  assert.equal(verifiedExecutable(f.root), realpathSync(f.file));
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
  f.manifest.sourceRevision = "main";
  f.save();
  assert.throws(() => verifiedExecutable(f.root), /source revision/);
  f.manifest.sourceRevision = "a".repeat(40);
  delete f.manifest.artifacts[f.key];
  f.save();
  assert.throws(() => verifiedExecutable(f.root), /missing artifact/);
  f.manifest.artifacts[f.key] = entry;
  f.save();
  writeFileSync(f.file, "corrupted");
  assert.throws(() => verifiedExecutable(f.root), /checksum mismatch/);
  rmSync(f.file);
  assert.throws(() => verifiedExecutable(f.root), /ENOENT/);
  symlinkSync(process.execPath, f.file);
  assert.throws(() => verifiedExecutable(f.root), /regular/);
});

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
  rmSync(f.platformRoot, { recursive: true });
  pkg.optionalDependencies[platformPackage(f.key).name] = "0.1.0";
  writeFileSync(pkgPath, JSON.stringify(pkg));
  assert.throws(
    () => verifiedExecutable(f.root),
    /optional dependencies enabled/,
  );
});

test(
  "launcher preserves argv, stdin, stdout, stderr and exit status",
  { skip: process.platform === "win32" },
  (t) => {
    const f = fixture(t);
    const result = spawnSync(
      process.execPath,
      [resolve(f.root, "bin/anydash.mjs"), "space value", "--json"],
      { input: "input\n", encoding: "utf8" },
    );
    assert.equal(result.status, 7);
    assert.equal(result.stdout, '["space value","--json"]\ninput\n');
    assert.equal(result.stderr, "fixture stderr\n");
  },
);

test("launcher reports verification failures with a reinstall instruction", (t) => {
  const f = fixture(t);
  writeFileSync(f.file, "corrupted");
  const result = spawnSync(
    process.execPath,
    [resolve(f.root, "bin/anydash.mjs"), "status"],
    { encoding: "utf8" },
  );
  assert.equal(result.status, 1);
  assert.match(result.stderr, /checksum mismatch/);
  assert.match(
    result.stderr,
    /Reinstall the matching @delinoio\/anydash version/,
  );
  assert.equal(result.stdout, "");
});

test(
  "launcher forwards SIGTERM to the executable",
  { skip: process.platform === "win32", timeout: 10000 },
  async (t) => {
    const f = fixture(t);
    const child = spawn(
      process.execPath,
      [resolve(f.root, "bin/anydash.mjs"), "wait"],
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
    child.kill("SIGTERM");
    assert.deepEqual(await result, { code: 23, signal: null });
    assert.match(output, /terminated/);
  },
);

test(
  "launcher preserves signal termination when the executable does not handle it",
  { skip: process.platform === "win32", timeout: 10000 },
  async (t) => {
    const f = fixture(t);
    for (const signal of ["SIGINT", "SIGHUP"]) {
      const child = spawn(
        process.execPath,
        [resolve(f.root, "bin/anydash.mjs"), "wait"],
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

test(
  "packed candidates install only the host platform package and fail clearly without optional dependencies",
  { skip: process.platform === "win32" },
  async (t) => {
    const f = fixture(t);
    const pack = (cwd) => {
      const command = npmCommand([
        "pack",
        "--ignore-scripts",
        "--pack-destination",
        f.root,
        "--json",
      ]);
      const result = spawnSync(command.file, command.args, {
        cwd,
        encoding: "utf8",
      });
      assert.equal(result.status, 0, result.stderr);
      return JSON.parse(result.stdout)[0];
    };
    for (const key of platforms)
      f.manifest.artifacts[key] = {
        ...f.manifest.artifacts[f.key],
        platform: key,
        path: executableName(key),
      };
    f.save();
    const entry = pack(f.root);
    assert.ok(
      entry.files.every(
        ({ path }) =>
          ["package.json", "manifest.json"].includes(path) ||
          path.startsWith("bin/"),
      ),
      "the launcher package carries no executables",
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
      cpSync(f.file, resolve(dir, executableName(key)));
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
      ["anydash", `anydash-${f.key}`].sort(),
    );
    const run = spawnSync(
      process.execPath,
      [
        resolve(install, "node_modules/@delinoio/anydash/bin/anydash.mjs"),
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
        ["anydash", `anydash-${key}`].sort(),
      );
      assert.deepEqual(
        readFileSync(
          verifiedExecutable(
            resolve(prefix, "node_modules/@delinoio/anydash"),
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
      resolve(globalPrefix, "bin/anydash"),
      ["status"],
      {
        input: "",
        encoding: "utf8",
      },
    );
    assert.equal(globalRun.status, 7, globalRun.stderr);
    assert.match(globalRun.stdout, /status/);
    // Outside the fixture root, whose node_modules would satisfy resolution.
    const omitted = mkdtempSync(resolve(tmpdir(), "anydash-omitted-"));
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
      [resolve(omitted, "node_modules/@delinoio/anydash/bin/anydash.mjs")],
      { encoding: "utf8" },
    );
    assert.equal(failure.status, 1);
    assert.match(failure.stderr, /optional dependencies enabled/);
  },
);

test("CLI archives are deterministic ustar archives with executable modes", (t) => {
  const root = mkdtempSync(resolve(tmpdir(), "anydash-archive-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(resolve(root, "THIRD_PARTY_NOTICES", "a".repeat(90)), {
    recursive: true,
  });
  writeFileSync(resolve(root, "anydash"), "binary");
  writeFileSync(resolve(root, "LICENSE"), "license");
  writeFileSync(
    resolve(root, "THIRD_PARTY_NOTICES", "a".repeat(90), "LICENSE-APACHE"),
    "notice",
  );
  const marked = new Set(["anydash-linux-x64/anydash"]);
  const first = tarGzip(entries(root, "anydash-linux-x64", marked));
  const second = tarGzip(entries(root, "anydash-linux-x64", marked));
  assert.deepEqual(first, second);
  const tar = gunzipSync(first);
  assert.equal(tar.subarray(257, 262).toString(), "ustar");
  const extracted = resolve(root, "out");
  mkdirSync(extracted);
  writeFileSync(resolve(root, "archive.tar.gz"), first);
  const result = spawnSync(
    "tar",
    ["-xzf", resolve(root, "archive.tar.gz"), "-C", extracted],
    {
      encoding: "utf8",
    },
  );
  assert.equal(result.status, 0, result.stderr);
  const listing = spawnSync("tar", ["-tvzf", resolve(root, "archive.tar.gz")], {
    encoding: "utf8",
  }).stdout;
  assert.match(listing, /^-rwxr-xr-x .*anydash-linux-x64\/anydash$/m);
  assert.match(listing, /^-rw-r--r-- .*anydash-linux-x64\/LICENSE$/m);
  assert.equal(
    readFileSync(
      resolve(
        extracted,
        "anydash-linux-x64/THIRD_PARTY_NOTICES",
        "a".repeat(90),
        "LICENSE-APACHE",
      ),
      "utf8",
    ),
    "notice",
  );
});
