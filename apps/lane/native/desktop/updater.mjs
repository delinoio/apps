// SPDX-License-Identifier: Apache-2.0
import { createHash } from "node:crypto";
import {
  access,
  copyFile,
  chmod,
  mkdir,
  mkdtemp,
  open,
  readFile,
  readdir,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import { spawn } from "node:child_process";
import { dirname, join } from "node:path";
import { extractUpdate } from "./update-archive.mjs";
import { executablePath, writeResult } from "./update-files.mjs";

const api = "https://api.github.com/repos/delinoio/apps";
const tagPattern = /^lane@v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
export function compareReleases(a, b) {
  const left = tagPattern.exec(a),
    right = tagPattern.exec(b);
  if (!left || !right) throw new Error("The installed release is invalid.");
  for (let i = 1; i <= 3; i++) {
    const diff = BigInt(left[i]) - BigInt(right[i]);
    if (diff) return diff > 0 ? 1 : -1;
  }
  return 0;
}
export function selectRelease(releases, installed, platform) {
  if (!Array.isArray(releases))
    throw new Error("Could not read the release list.");
  const latest = releases
    .filter((r) => !r.draft && !r.prerelease && tagPattern.test(r.tag_name))
    .sort((a, b) => compareReleases(b.tag_name, a.tag_name))[0];
  if (!latest) throw new Error("No Lane desktop release is available yet.");
  if (compareReleases(latest.tag_name, installed) <= 0) return undefined;
  const archive = `lane-desktop-${platform}-${latest.tag_name.replace("lane@", "")}.tar.gz`;
  const asset = (name) => {
    const matches = latest.assets?.filter((a) => a.name === name);
    if (
      matches?.length !== 1 ||
      !Number.isSafeInteger(matches[0].size) ||
      matches[0].size <= 0
    )
      throw new Error("The latest release is missing a complete Lane update.");
    const url = new URL(matches[0].browser_download_url);
    if (
      url.origin !== "https://github.com" ||
      url.search ||
      url.hash ||
      url.username ||
      url.password ||
      decodeURIComponent(url.pathname) !==
        `/delinoio/apps/releases/download/${latest.tag_name}/${name}`
    )
      throw new Error("The release download address is invalid.");
    return matches[0];
  };
  return {
    release: latest.tag_name,
    archive: asset(archive),
    checksums: asset("SHA256SUMS"),
  };
}
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const messages = {
  idle: "Update Lane to the latest version. The app will restart when the update is ready.",
  checking: "Checking for updates…",
  downloading: "Downloading update…",
  verifying: "Verifying update…",
  installing: "Installing update. Lane will restart…",
  current: "Lane is up to date.",
};
export class DesktopUpdater {
  constructor({
    resources,
    install,
    stateDirectory,
    config,
    platform = process.platform,
    arch = process.arch,
    fetcher = fetch,
    restart,
    canRestart = () => true,
    log = () => {},
  }) {
    Object.assign(this, {
      resources,
      install,
      stateDirectory,
      config,
      platform,
      arch,
      fetcher,
      restart,
      canRestart,
      log,
    });
    this.result = join(stateDirectory, "update-result.json");
    this.state = {
      phase: "idle",
      installedRelease: config.releaseTag || "Development build",
      message: messages.idle,
    };
    this.enabled =
      config.scheme === "lane" &&
      !config.testing &&
      tagPattern.test(config.releaseTag) &&
      /^[a-f0-9]{40}$/.test(config.sourceRevision || "");
    if (
      !this.enabled ||
      config.scheme !== "lane" ||
      config.testing ||
      !tagPattern.test(config.releaseTag) ||
      !(
        (platform === "darwin" && ["x64", "arm64"].includes(arch)) ||
        (platform === "win32" && arch === "x64")
      )
    ) {
      this.state = {
        ...this.state,
        phase: "unavailable",
        message:
          "In-app updates require an official Lane desktop release. Development and test builds cannot update.",
      };
    }
  }
  set(phase, message = messages[phase], progress) {
    this.state = { ...this.state, phase, message, progress };
    this.log({ event: "lane_update_state", phase });
  }
  async cleanup() {
    if (this.lastCleanup && Date.now() - this.lastCleanup < 10000) return;
    this.lastCleanup = Date.now();
    for (const entry of await readdir(dirname(this.install)).catch(() => [])) {
      if (!entry.startsWith(".lane-update-")) continue;
      const stage = join(dirname(this.install), entry);
      try {
        const job = JSON.parse(await readFile(join(stage, "job.json"), "utf8"));
        if (job.install !== this.install || job.stage !== stage) continue;
        const pid = Number(
          await readFile(join(stage, "installer-pid"), "utf8"),
        );
        if (!Number.isSafeInteger(pid) || pid <= 0) continue;
        try {
          process.kill(pid, 0);
          continue;
        } catch (error) {
          if (error.code !== "ESRCH") continue;
        }
        try {
          await access(join(stage, "previous"));
          continue;
        } catch {
          /* Never delete a retained rollback bundle. */
        }
        await rm(stage, { recursive: true, force: true });
      } catch {
        /* Ignore unrelated, active or incomplete staging directories. */
      }
    }
  }
  async status() {
    await this.cleanup();
    try {
      const result = JSON.parse(await readFile(this.result, "utf8"));
      if (this.enabled && result.phase === "error") {
        await rm(this.result);
        this.set("error", result.message);
      }
    } catch {
      /* A missing result means no previous installation failed. */
    }
    return this.state;
  }
  async healthy() {
    try {
      const result = JSON.parse(await readFile(this.result, "utf8"));
      if (
        result.phase === "launching" &&
        result.release === this.config.releaseTag
      )
        await writeResult(this.result, {
          phase: "healthy",
          release: result.release,
        });
    } catch {
      /* No update is awaiting a startup acknowledgement. */
    }
  }
  async response(url) {
    const response = await this.fetcher(url, {
      headers: {
        Accept: "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
      },
      signal: AbortSignal.any([
        AbortSignal.timeout(10 * 60 * 1000),
        ...(this.operation ? [this.operation.controller.signal] : []),
      ]),
    });
    if (!response.ok)
      throw new Error(
        response.status === 403 || response.status === 429
          ? "Update checks are temporarily limited. Try again later."
          : "Could not download the update. Check your connection and retry.",
      );
    return response;
  }
  async bytes(url, limit) {
    const response = await this.response(url);
    const chunks = [];
    let size = 0;
    for await (const chunk of response.body) {
      size += chunk.length;
      if (size > limit) throw new Error("The update response is too large.");
      chunks.push(chunk);
    }
    return Buffer.concat(chunks).toString("utf8");
  }
  cancel() {
    if (this.running && this.operation && this.state.phase !== "installing") {
      this.operation.cancelled = true;
      this.operation.controller.abort();
      if (this.stage && this.installerStarted)
        void writeFile(join(this.stage, "cancel"), "cancel").catch(() => {});
      this.set("idle");
    }
  }
  async start() {
    if (
      !this.enabled ||
      this.running ||
      !["idle", "current", "error"].includes(this.state.phase)
    )
      return this.state;
    if (!this.canRestart()) {
      this.set(
        "error",
        "Save or close the open RealQA capture before updating Lane.",
      );
      return this.state;
    }
    const operation = { cancelled: false, controller: new AbortController() };
    this.operation = operation;
    this.set("checking");
    // Return immediately; the main process owns the operation across Settings
    // lifetime. Leaving Settings cancels before installation to protect edits.
    this.running = true;
    this.pending = this.perform(operation)
      .catch(async (error) => {
        if (!operation.cancelled)
          this.set(
            "error",
            error.constructor === Error && !error.code
              ? error.message
              : "The update failed. Check your connection, free disk space and folder permissions, then retry.",
          );
        this.log({
          event: "lane_update_failed",
          code: error.code || "UPDATE_FAILED",
        });
        if (this.stage && this.installerStarted)
          await writeFile(join(this.stage, "cancel"), "cancel").catch(() => {});
        if (this.stage && !this.installerStarted)
          await rm(this.stage, { recursive: true, force: true }).catch(
            () => {},
          );
      })
      .finally(() => {
        this.running = false;
      });
    return this.state;
  }
  async perform(operation) {
    const active = () => {
      if (operation.cancelled) throw new Error("Update cancelled.");
    };
    const releases = JSON.parse(
      await this.bytes(`${api}/releases?per_page=100`, 4 * 1024 * 1024),
    );
    active();
    const target = selectRelease(
      releases,
      this.config.releaseTag,
      `${this.platform === "win32" ? "windows" : "darwin"}-${this.arch === "x64" ? "amd64" : this.arch}`,
    );
    if (!target) {
      this.set("current");
      return;
    }
    if (target.archive.size > 1024 ** 3)
      throw new Error("The update archive is too large.");
    await access(this.install);
    active();
    this.stage = await mkdtemp(join(dirname(this.install), ".lane-update-"));
    this.installerStarted = false;
    await mkdir(this.stateDirectory, { recursive: true });
    const archive = join(this.stage, "update.tar.gz");
    active();
    this.set("downloading", messages.downloading, 0);
    const response = await this.response(target.archive.browser_download_url);
    const file = await open(archive, "wx", 0o600);
    let received = 0;
    const hash = createHash("sha256");
    try {
      for await (const chunk of response.body) {
        active();
        received += chunk.length;
        if (received > target.archive.size)
          throw new Error("The update download has an invalid size.");
        hash.update(chunk);
        await file.writeFile(chunk);
        this.state.progress = Math.min(
          100,
          Math.floor((received * 100) / target.archive.size),
        );
      }
    } finally {
      await file.close();
    }
    if (received !== target.archive.size)
      throw new Error("The update download is incomplete.");
    active();
    this.set("verifying");
    const checksums = await this.bytes(
      target.checksums.browser_download_url,
      1024 * 1024,
    );
    const matches = checksums
      .split(/\r?\n/)
      .filter((line) => line.endsWith(`  ${target.archive.name}`));
    if (
      matches.length !== 1 ||
      !/^[a-f0-9]{64}  /.test(matches[0]) ||
      matches[0].slice(0, 64) !== hash.digest("hex")
    )
      throw new Error("The update checksum did not match. Retry the update.");
    const destination = join(this.stage, "extracted");
    await mkdir(destination);
    const candidate = await extractUpdate(
      archive,
      destination,
      this.platform === "darwin" ? "Lane.app" : "application",
    );
    const candidateResources = join(
      candidate,
      this.platform === "darwin" ? "Contents/Resources/app" : "resources/app",
    );
    const candidateConfig = JSON.parse(
      await readFile(join(candidateResources, "package.json"), "utf8"),
    );
    if (
      candidateConfig.name !== "lane" ||
      candidateConfig.scheme !== "lane" ||
      candidateConfig.testing ||
      candidateConfig.releaseTag !== target.release ||
      !/^[a-f0-9]{40}$/.test(candidateConfig.sourceRevision || "")
    )
      throw new Error(
        "The update package does not match the selected release.",
      );
    if (!(await stat(executablePath(candidate, this.platform))).isFile())
      throw new Error("The update executable is missing.");
    active();
    if (!this.canRestart())
      throw new Error(
        "Save or close the open RealQA capture, then retry the update.",
      );
    await this.prepareInstaller(candidate, target.release);
    active();
    if (!this.canRestart())
      throw new Error(
        "Save or close the open RealQA capture, then retry the update.",
      );
    this.set("installing");
    this.restart();
  }
  async prepareInstaller(candidate, release) {
    const runtime = join(
      this.stage,
      this.platform === "win32" ? "node.exe" : "node",
    );
    await copyFile(
      join(
        this.resources,
        "relay",
        this.platform === "win32" ? "node.exe" : "node-macos12",
      ),
      runtime,
    );
    await chmod(runtime, 0o700);
    await copyFile(
      join(
        this.resources,
        "relay",
        this.platform === "win32" ? "NODE-LICENSE" : "NODE-MACOS12-LICENSE",
      ),
      join(this.stage, "NODE-LICENSE"),
    );
    const script = join(this.stage, "update-install.mjs");
    await copyFile(join(this.resources, "update-install.mjs"), script);
    const job = {
      stage: this.stage,
      candidate,
      install: this.install,
      platform: this.platform,
      parentPid: process.pid,
      result: this.result,
      release,
    };
    const jobFile = join(this.stage, "job.json");
    await writeFile(jobFile, JSON.stringify(job), { mode: 0o600 });
    const child = spawn(runtime, [script, jobFile], {
      detached: true,
      windowsHide: true,
      stdio: "ignore",
      cwd: this.stage,
      env: { ...process.env, NODE_OPTIONS: "", NODE_PATH: "" },
    });
    await new Promise((resolve, reject) => {
      child.once("spawn", resolve);
      child.once("error", reject);
    });
    child.unref();
    this.installerStarted = true;
    for (let i = 0; i < 100; i++) {
      try {
        await access(join(this.stage, "ready"));
        return;
      } catch {
        await delay(100);
      }
    }
    throw new Error("The update installer could not start. Retry the update.");
  }
  async abortInstall() {
    if (this.stage && this.installerStarted)
      await writeFile(join(this.stage, "cancel"), "cancel").catch(() => {});
    this.set(
      "error",
      "Lane could not close for the update. Finish pending operations and retry.",
    );
  }
  async commitInstall() {
    if (this.state.phase === "installing")
      await writeFile(join(this.stage, "install"), "install", { mode: 0o600 });
  }
}
