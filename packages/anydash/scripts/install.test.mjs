// SPDX-License-Identifier: Apache-2.0
// Runs install.sh and install.ps1 against local release directories served on
// loopback. Platform detection is driven by fake uname, sysctl and ldd commands.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { executableName, platforms } from "../bin/verify.mjs";
import { writeArchive } from "./archive.mjs";
import { archiveName, sha256File, writeChecksums } from "./package.mjs";
import { release, serveReleases } from "./release-fixture.mjs";

const scripts = fileURLToPath(new URL("../install/", import.meta.url));
const sh = resolve(scripts, "install.sh");
const ps1 = resolve(scripts, "install.ps1");
const pwsh = spawnSync(
  "pwsh",
  ["-NoProfile", "-Command", "$PSVersionTable.PSVersion.Major"],
  {
    encoding: "utf8",
  },
);
const hasPwsh = !pwsh.error && pwsh.status === 0;

/** Writes a release directory whose archives contain a runnable fake CLI. */
function releaseDirectory(root, version, keys = platforms) {
  const directory = resolve(root, `anydash@v${version}`);
  mkdirSync(directory, { recursive: true });
  const files = [];
  for (const key of keys) {
    const stage = resolve(root, "stage", version, key);
    mkdirSync(stage, { recursive: true });
    const name = executableName(key);
    writeFileSync(
      resolve(stage, name),
      `#!/bin/sh\necho "anydash ${version} ${key} $*"\n`,
    );
    chmodSync(resolve(stage, name), 0o755);
    writeFileSync(
      resolve(stage, "SHA256SUMS"),
      `${sha256File(resolve(stage, name))}  ${name}\n`,
    );
    writeFileSync(resolve(stage, "LICENSE"), "license\n");
    const archive = archiveName(key, version);
    writeArchive(stage, `anydash-${key}`, resolve(directory, archive), [name]);
    files.push(archive);
  }
  writeChecksums(directory, files);
  return directory;
}

function fakeCommands(
  root,
  { os = "Linux", arch = "x86_64", translated, musl },
) {
  const bin = resolve(root, "fakebin");
  rmSync(bin, { recursive: true, force: true });
  mkdirSync(bin);
  const write = (name, body) => {
    writeFileSync(resolve(bin, name), `#!/bin/sh\n${body}\n`);
    chmodSync(resolve(bin, name), 0o755);
  };
  write(
    "uname",
    `case "$1" in -s) echo ${os} ;; -m) echo ${arch} ;; *) echo ${os} ;; esac`,
  );
  write("sysctl", translated ? "echo 1" : "exit 1");
  write(
    "ldd",
    musl
      ? "echo 'musl libc (x86_64)' >&2; exit 1"
      : "echo 'ldd (GNU libc) 2.36'",
  );
  return bin;
}

function run(command, args, env) {
  return new Promise((done, reject) => {
    const child = spawn(command, args, {
      env,
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

async function fixture(t, versions = ["1.2.0", "1.10.0"]) {
  const root = mkdtempSync(resolve(tmpdir(), "anydash-install-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const directories = Object.fromEntries(
    versions.map((version) => [
      `anydash@v${version}`,
      releaseDirectory(root, version),
    ]),
  );
  const server = await serveReleases({
    releases: [
      release("lane@v9.0.0"),
      ...versions.map((version) => release(`anydash@v${version}`)),
    ],
    directories,
  });
  t.after(server.close);
  return { root, directories, server };
}

// Linux distributions often run install.sh with dash; macOS sh is bash.
const shells = [
  "sh",
  ...(spawnSync("dash", ["-c", "true"]).status === 0 ? ["dash"] : []),
];
for (const interpreter of shells) {
  const shell = (f, platform, args = [], extra = {}) =>
    run(interpreter, [sh, ...args], {
      PATH: `${fakeCommands(f.root, platform)}:${process.env.PATH}`,
      HOME: resolve(f.root, "home"),
      ...f.server.env,
      ...extra,
    });

  test(
    `${interpreter}: install.sh selects the newest AnyDash release and installs a verified executable`,
    { skip: process.platform === "win32" },
    async (t) => {
      const f = await fixture(t);
      const result = await shell(f, { os: "Linux", arch: "x86_64" });
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.stderr, /installed AnyDash 1\.10\.0/);
      const installed = resolve(f.root, "home/.local/bin/anydash");
      const run = spawnSync(installed, ["--version"], { encoding: "utf8" });
      assert.equal(run.stdout.trim(), "anydash 1.10.0 linux-x64 --version");
      assert.ok(
        f.server.requests.includes(
          "/download/anydash@v1.10.0/anydash-linux-x64-1.10.0.tar.gz",
        ),
      );
      assert.deepEqual(readdirSync(resolve(f.root, "home/.local/bin")), [
        "anydash",
      ]);
    },
  );

  test(
    `${interpreter}: install.sh accepts an explicit version and install directory`,
    { skip: process.platform === "win32" },
    async (t) => {
      const f = await fixture(t);
      const target = resolve(f.root, "custom bin");
      const result = await shell(f, { os: "Darwin", arch: "arm64" }, [
        "--version",
        "1.2.0",
        "--install-dir",
        target,
      ]);
      assert.equal(result.status, 0, result.stderr);
      assert.match(
        spawnSync(resolve(target, "anydash"), [], { encoding: "utf8" }).stdout,
        /anydash 1\.2\.0 darwin-arm64/,
      );
      assert.ok(!f.server.requests.includes("/releases"));
      const env = await shell(f, { os: "Darwin", arch: "arm64" }, [], {
        ANYDASH_VERSION: "v1.2.0",
        ANYDASH_INSTALL_DIR: resolve(f.root, "env bin"),
      });
      assert.equal(env.status, 0, env.stderr);
      assert.ok(existsSync(resolve(f.root, "env bin/anydash")));
    },
  );

  test(
    `${interpreter}: install.sh maps OS and CPU names to the support matrix`,
    { skip: process.platform === "win32" },
    async (t) => {
      const f = await fixture(t);
      for (const [platform, key] of [
        [{ os: "Linux", arch: "x86_64" }, "linux-x64"],
        [{ os: "Linux", arch: "amd64" }, "linux-x64"],
        [{ os: "Linux", arch: "aarch64" }, "linux-arm64"],
        [{ os: "Darwin", arch: "x86_64" }, "darwin-x64"],
        [{ os: "Darwin", arch: "arm64" }, "darwin-arm64"],
        [{ os: "Darwin", arch: "x86_64", translated: true }, "darwin-arm64"],
      ]) {
        const directory = resolve(f.root, `home-${key}`);
        const result = await shell(f, platform, ["--install-dir", directory]);
        assert.equal(result.status, 0, result.stderr);
        assert.match(
          spawnSync(resolve(directory, "anydash"), [], { encoding: "utf8" })
            .stdout,
          new RegExp(` ${key} `),
        );
      }
      for (const [platform, message] of [
        [{ os: "FreeBSD", arch: "amd64" }, /unsupported platform FreeBSD/],
        [
          { os: "Linux", arch: "riscv64" },
          /unsupported platform linux\/riscv64/,
        ],
        [{ os: "Linux", arch: "x86_64", musl: true }, /musl libc/],
      ]) {
        const before = f.server.requests.length;
        const result = await shell(f, platform);
        assert.equal(result.status, 1);
        assert.match(result.stderr, message);
        assert.match(result.stderr, /nothing was installed/);
        assert.equal(f.server.requests.length, before, "no download starts");
      }
      assert.ok(!existsSync(resolve(f.root, "home/.local/bin/anydash")));
    },
  );

  test(
    `${interpreter}: install.sh rejects checksum mismatches and keeps the existing executable`,
    { skip: process.platform === "win32" },
    async (t) => {
      const f = await fixture(t, ["2.0.0"]);
      const target = resolve(f.root, "bin");
      mkdirSync(target);
      writeFileSync(resolve(target, "anydash"), "previous\n");
      const sums = resolve(f.directories["anydash@v2.0.0"], "SHA256SUMS");
      const original = readFileSync(sums, "utf8");
      writeFileSync(
        sums,
        original.replace(
          /^[a-f0-9]{64}(  anydash-linux-x64-)/m,
          `${"0".repeat(64)}$1`,
        ),
      );
      const mismatch = await shell(f, { os: "Linux", arch: "x86_64" }, [
        "--install-dir",
        target,
      ]);
      assert.equal(mismatch.status, 1);
      assert.match(
        mismatch.stderr,
        /checksum mismatch for anydash-linux-x64-2\.0\.0\.tar\.gz/,
      );
      assert.equal(
        readFileSync(resolve(target, "anydash"), "utf8"),
        "previous\n",
      );
      assert.deepEqual(readdirSync(target), ["anydash"]);

      writeFileSync(sums, original.replace(/^.*anydash-linux-x64-.*\n/m, ""));
      const missing = await shell(f, { os: "Linux", arch: "x86_64" }, [
        "--install-dir",
        target,
      ]);
      assert.equal(missing.status, 1);
      assert.match(
        missing.stderr,
        /does not list anydash-linux-x64-2\.0\.0\.tar\.gz/,
      );

      // A consistent outer checksum cannot hide a modified executable.
      const stage = resolve(f.root, "stage/2.0.0/linux-x64");
      writeFileSync(resolve(stage, "anydash"), "#!/bin/sh\necho tampered\n");
      writeArchive(
        stage,
        "anydash-linux-x64",
        resolve(
          f.directories["anydash@v2.0.0"],
          "anydash-linux-x64-2.0.0.tar.gz",
        ),
        ["anydash"],
      );
      writeChecksums(
        f.directories["anydash@v2.0.0"],
        platforms.map((key) => archiveName(key, "2.0.0")),
      );
      const inner = await shell(f, { os: "Linux", arch: "x86_64" }, [
        "--install-dir",
        target,
      ]);
      assert.equal(inner.status, 1);
      assert.match(
        inner.stderr,
        /checksum mismatch for the extracted executable/,
      );
      assert.equal(
        readFileSync(resolve(target, "anydash"), "utf8"),
        "previous\n",
      );
    },
  );

  test(
    `${interpreter}: install.sh rejects invalid versions, missing releases and unknown options`,
    { skip: process.platform === "win32" },
    async (t) => {
      const f = await fixture(t, ["1.0.0"]);
      for (const [args, message] of [
        [["--version", "1.0"], /invalid version 1\.0/],
        [
          ["--version", "3.0.0"],
          /cannot download SHA256SUMS for AnyDash 3\.0\.0/,
        ],
        [["--bogus"], /unknown option --bogus/],
      ]) {
        const result = await shell(f, { os: "Linux", arch: "x86_64" }, args);
        assert.equal(result.status, 1);
        assert.match(result.stderr, message);
      }
      const empty = await serveReleases({
        releases: [release("lane@v1.0.0")],
        directories: {},
      });
      t.after(empty.close);
      const none = await shell(
        { ...f, server: empty },
        { os: "Linux", arch: "x86_64" },
      );
      assert.equal(none.status, 1);
      assert.match(none.stderr, /no published AnyDash release/);
    },
  );
}

const powershell = (f, script) =>
  run(
    "pwsh",
    [
      "-NoProfile",
      "-NonInteractive",
      "-Command",
      `$ErrorActionPreference = 'Stop'; $AnyDashInstallLibrary = $true; . '${ps1}'; ${script}`,
    ],
    { ...process.env, ...f.server.env },
  );

test(
  "install.ps1 maps architectures and installs a verified executable",
  { skip: !hasPwsh && "pwsh is not installed" },
  async (t) => {
    const f = await fixture(t);
    const mapping = await powershell(
      f,
      "Get-AnyDashTarget -Architecture X64; Get-AnyDashTarget -Architecture Arm64; try { Get-AnyDashTarget -Architecture X86 } catch { Write-Output $_.Exception.Message }",
    );
    assert.equal(mapping.status, 0, mapping.stderr);
    assert.deepEqual(mapping.stdout.trim().split(/\r?\n/), [
      "windows-x64",
      "windows-arm64",
      "unsupported platform windows/X86; AnyDash supports x64 and arm64",
    ]);
    const target = resolve(f.root, "Programs AnyDash");
    const install = await powershell(
      f,
      `Install-AnyDash -Target windows-arm64 -InstallDir '${target}' -ReleasesUrl $env:ANYDASH_RELEASES_URL -DownloadUrl $env:ANYDASH_DOWNLOAD_URL -UpdatePath $false`,
    );
    assert.equal(install.status, 0, install.stderr);
    assert.match(install.stdout, /installed AnyDash 1\.10\.0/);
    assert.match(
      readFileSync(resolve(target, "anydash.exe"), "utf8"),
      /anydash 1\.10\.0 windows-arm64/,
    );
    assert.deepEqual(readdirSync(target), ["anydash.exe"]);
    const direct = await run(
      "pwsh",
      ["-NoProfile", "-NonInteractive", "-File", ps1],
      { ...process.env, ...f.server.env, OS: "" },
    );
    assert.notEqual(direct.status, 0);
    assert.match(
      direct.stdout + direct.stderr,
      /use https:\/\/anydash\.delino\.io\/install\.sh/,
    );
  },
);

test(
  "install.ps1 rejects checksum mismatches and installs nothing",
  { skip: !hasPwsh && "pwsh is not installed" },
  async (t) => {
    const f = await fixture(t, ["2.0.0"]);
    const sums = resolve(f.directories["anydash@v2.0.0"], "SHA256SUMS");
    writeFileSync(
      sums,
      readFileSync(sums, "utf8").replace(
        /^[a-f0-9]{64}(  anydash-windows-x64-)/m,
        `${"f".repeat(64)}$1`,
      ),
    );
    const target = resolve(f.root, "bin");
    const result = await powershell(
      f,
      `try { Install-AnyDash -Target windows-x64 -Version 2.0.0 -InstallDir '${target}' -ReleasesUrl $env:ANYDASH_RELEASES_URL -DownloadUrl $env:ANYDASH_DOWNLOAD_URL -UpdatePath $false } catch { Write-Output $_.Exception.Message }`,
    );
    assert.equal(result.status, 0, result.stderr);
    assert.match(
      result.stdout,
      /checksum mismatch for anydash-windows-x64-2\.0\.0\.tar\.gz/,
    );
    assert.ok(!existsSync(resolve(target, "anydash.exe")));
  },
);
