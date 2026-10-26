// SPDX-License-Identifier: Apache-2.0
import { createHash } from "node:crypto";
import { spawn, execFile } from "node:child_process";
import { promisify } from "node:util";
import { createWriteStream, constants } from "node:fs";
import {
  access,
  lstat,
  mkdtemp,
  readFile,
  rm,
  writeFile,
} from "node:fs/promises";
import { dirname, join } from "node:path";
import { Readable, Transform } from "node:stream";
import { pipeline } from "node:stream/promises";
import {
  assetURL,
  archiveName,
  newerVersion,
  releasePrefix,
  repository,
  targetName,
  updatePublicKey,
  verifyManifest,
  versionParts,
} from "./update-contract.mjs";

const execute = promisify(execFile);
const hours = 6 * 60 * 60 * 1000;
const shellQuote = (value) => `'${String(value).replaceAll("'", "'\\''")}'`;
const psQuote = (value) => `'${String(value).replaceAll("'", "''")}'`;

// Lynxtron has no autoUpdater API. Keep the replacement helper outside the
// running package, so Windows can release every runtime DLL before the swap.
// Remove this adapter when Lynxtron supplies a supported native updater.
export function replacementScript({
  platform,
  pid,
  installed,
  candidate,
  stage,
}) {
  const backup = join(stage, "previous");
  if (!Number.isSafeInteger(pid) || pid <= 0)
    throw new Error("Invalid update parent PID.");
  if (platform === "darwin")
    return `#!/bin/sh
set -eu
attempt=0
while kill -0 ${pid} 2>/dev/null; do
  attempt=$((attempt + 1))
  if [ "$attempt" -ge 120 ]; then
    printf '%s\\n' '{"event":"lane_update_install_timeout","platform":"darwin"}' > ${shellQuote(join(stage, "result.json"))}
    exit 1
  fi
  sleep 1
done
rollback() {
  printf '%s\\n' '{"event":"lane_update_install_failed","platform":"darwin"}' > ${shellQuote(join(stage, "result.json"))}
  if [ ! -e ${shellQuote(installed)} ] && [ -d ${shellQuote(backup)} ]; then
    mv ${shellQuote(backup)} ${shellQuote(installed)}
  fi
}
trap rollback EXIT
trap 'exit 1' HUP INT TERM
mv ${shellQuote(installed)} ${shellQuote(backup)}
[ ! -e ${shellQuote(installed)} ]
mv ${shellQuote(candidate)} ${shellQuote(installed)}
trap - EXIT HUP INT TERM
rm -rf ${shellQuote(stage)}
`;
  if (platform !== "win32") throw new Error("Unsupported updater platform.");
  return `$ErrorActionPreference = 'Stop'
$parentProcess = Get-Process -Id ${pid} -ErrorAction SilentlyContinue
if ($parentProcess) {
  if (-not $parentProcess.WaitForExit(120000)) {
    '{"event":"lane_update_install_timeout","platform":"win32"}' | Set-Content -LiteralPath ${psQuote(join(stage, "result.json"))}
    exit 1
  }
}
try {
  [System.IO.Directory]::Move(${psQuote(installed)}, ${psQuote(backup)})
  [System.IO.Directory]::Move(${psQuote(candidate)}, ${psQuote(installed)})
} catch {
  '{"event":"lane_update_install_failed","platform":"win32"}' | Set-Content -LiteralPath ${psQuote(join(stage, "result.json"))}
  if ((-not (Test-Path -LiteralPath ${psQuote(installed)})) -and (Test-Path -LiteralPath ${psQuote(backup)})) {
    [System.IO.Directory]::Move(${psQuote(backup)}, ${psQuote(installed)})
  }
  exit 1
}
Remove-Item -LiteralPath ${psQuote(stage)} -Recurse -Force
`;
}

async function response(url, fetcher) {
  const result = await fetcher(url, {
    headers: {
      Accept:
        new URL(url).hostname === "api.github.com"
          ? "application/vnd.github+json"
          : "application/octet-stream",
      "User-Agent": "Lane-Desktop-Updater",
    },
    signal: AbortSignal.timeout(120000),
  });
  if (!result.ok) throw new Error("Desktop update request failed.");
  return result;
}
async function json(url, fetcher) {
  const result = await response(url, fetcher);
  // Bound metadata while reading, including responses without Content-Length.
  const chunks = [];
  let size = 0;
  for await (const chunk of Readable.fromWeb(result.body)) {
    size += chunk.length;
    if (size > 1024 * 1024) throw new Error("Update metadata is too large.");
    chunks.push(chunk);
  }
  return JSON.parse(Buffer.concat(chunks).toString("utf8"));
}

export async function downloadArchive(manifest, destination, fetcher = fetch) {
  const result = await response(manifest.url, fetcher);
  let size = 0;
  const hash = createHash("sha256");
  const checker = new Transform({
    transform(chunk, _encoding, callback) {
      size += chunk.length;
      if (size > manifest.size)
        return callback(new Error("Update archive exceeds signed size."));
      hash.update(chunk);
      callback(null, chunk);
    },
  });
  await pipeline(
    Readable.fromWeb(result.body),
    checker,
    createWriteStream(destination, { flags: "wx", mode: 0o600 }),
  );
  if (size !== manifest.size || hash.digest("hex") !== manifest.sha256)
    throw new Error("Desktop update archive checksum mismatch.");
}

export class DesktopUpdater {
  constructor({
    config,
    installed,
    platform = process.platform,
    arch = process.arch,
    fetcher = fetch,
    executeCommand = execute,
    launch = spawn,
    log = (event) => console.info(JSON.stringify({ event, platform })),
  }) {
    Object.assign(this, {
      config,
      installed,
      platform,
      arch,
      fetcher,
      executeCommand,
      launch,
      log,
    });
    this.stopped = false;
  }
  async start() {
    if (
      !this.config.updates ||
      this.config.scheme !== "lane" ||
      this.config.testing
    )
      return;
    try {
      versionParts(this.config.version);
      if (this.config.updates.target !== targetName(this.platform, this.arch))
        throw new Error("Update target mismatch.");
      updatePublicKey(this.config.updates.publicKey);
      // Never follow a symlink installation or request privilege elevation.
      const installed = await lstat(this.installed);
      if (!installed.isDirectory() || installed.isSymbolicLink())
        throw new Error("Invalid installation directory.");
      await access(dirname(this.installed), constants.W_OK);
    } catch {
      this.log("lane_update_disabled");
      return;
    }
    this.log("lane_update_started");
    void this.check();
    this.timer = setInterval(() => void this.check(), hours);
    this.timer.unref?.();
  }
  async check() {
    if (this.stopped || this.busy || this.ready) return;
    this.busy = true;
    let stage;
    try {
      this.log("lane_update_checking");
      const releases = await json(
        `https://api.github.com/repos/${repository}/releases?per_page=100`,
        this.fetcher,
      );
      if (!Array.isArray(releases)) throw new Error("Invalid release list.");
      const versions = releases
        .filter(
          (release) =>
            !release.draft &&
            !release.prerelease &&
            typeof release.tag_name === "string" &&
            release.tag_name.startsWith(releasePrefix),
        )
        .map((release) => release.tag_name.slice(releasePrefix.length))
        .filter((version) => {
          try {
            return newerVersion(version, this.config.version);
          } catch {
            return false;
          }
        })
        .sort((left, right) =>
          newerVersion(left, right) ? -1 : newerVersion(right, left) ? 1 : 0,
        );
      if (!versions.length || this.stopped) return;
      const version = versions[0],
        target = targetName(this.platform, this.arch);
      const envelope = await json(
        assetURL(version, `update-${target}.json`),
        this.fetcher,
      );
      const manifest = verifyManifest(envelope, {
        publicKey: this.config.updates.publicKey,
        version,
        platform: this.platform,
        arch: this.arch,
      });
      stage = await mkdtemp(join(dirname(this.installed), ".lane-update-"));
      const archive = join(stage, archiveName(version, target));
      await downloadArchive(manifest, archive, this.fetcher);
      const unpack = join(stage, "unpack");
      if (this.platform === "darwin") {
        await this.executeCommand("/usr/bin/ditto", [
          "-x",
          "-k",
          archive,
          unpack,
        ]);
      } else {
        await this.executeCommand("powershell.exe", [
          "-NoProfile",
          "-NonInteractive",
          "-Command",
          `Expand-Archive -LiteralPath ${psQuote(archive)} -DestinationPath ${psQuote(unpack)}`,
        ]);
      }
      const candidate = join(
        unpack,
        this.platform === "darwin" ? "Lane.app" : "application",
      );
      const resources =
        this.platform === "darwin" ? "Contents/Resources/app" : "resources/app";
      const packaged = JSON.parse(
        await readFile(join(candidate, resources, "package.json"), "utf8"),
      );
      if (
        packaged.name !== "lane" ||
        packaged.version !== version ||
        packaged.scheme !== "lane" ||
        packaged.testing ||
        packaged.updates?.publicKey !== this.config.updates.publicKey ||
        packaged.updates?.target !== target
      )
        throw new Error("Update package identity mismatch.");
      await access(
        join(
          candidate,
          this.platform === "darwin"
            ? "Contents/MacOS/lynxtron"
            : "lynxtron.exe",
        ),
      );
      if (this.platform === "darwin")
        await this.executeCommand("/usr/bin/codesign", [
          "--verify",
          "--deep",
          "--strict",
          candidate,
        ]);
      const script = join(
        stage,
        this.platform === "darwin" ? "install.sh" : "install.ps1",
      );
      await writeFile(
        script,
        replacementScript({
          platform: this.platform,
          pid: process.pid,
          installed: this.installed,
          candidate,
          stage,
        }),
        { mode: 0o700 },
      );
      if (this.stopped) return;
      this.ready = { stage, script };
      stage = undefined;
      this.log("lane_update_ready");
    } catch {
      // Do not log server bodies, filesystem paths or raw OS diagnostics.
      this.log("lane_update_failed");
    } finally {
      if (stage)
        await rm(stage, { recursive: true, force: true }).catch(() => {});
      this.busy = false;
    }
  }
  stop() {
    this.stopped = true;
    clearInterval(this.timer);
  }
  installOnExit() {
    this.stop();
    if (!this.ready) return;
    try {
      const { script, stage } = this.ready;
      const child =
        this.platform === "darwin"
          ? this.launch("/bin/sh", [script], {
              detached: true,
              stdio: "ignore",
              cwd: dirname(this.installed),
            })
          : this.launch(
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
                detached: true,
                stdio: "ignore",
                windowsHide: true,
                cwd: dirname(this.installed),
              },
            );
      child.on("error", () => {
        this.log("lane_update_install_failed");
        void rm(stage, { recursive: true, force: true }).catch(() => {});
      });
      child.unref();
      this.ready = undefined;
      this.log("lane_update_install_scheduled");
    } catch {
      this.log("lane_update_install_failed");
    }
  }
}
