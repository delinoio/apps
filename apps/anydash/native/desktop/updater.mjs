// SPDX-License-Identifier: Apache-2.0
// Signature-verified desktop updates from GitHub Releases. The feed is the
// repository's stable `anydash@vMAJOR.MINOR.PATCH` releases. Each release
// carries `anydash-desktop-<platform>-<version>.tar.gz`, `SHA256SUMS` and
// `SHA256SUMS.sig` (Ed25519 over SHA256SUMS, base64). An update installs only
// after the signature, the checksum and the OS code signature verify and the
// user confirms in Settings.
import { spawn } from "node:child_process";
import { createHash, createPublicKey, verify } from "node:crypto";
import { mkdir, mkdtemp, readdir, rm, writeFile } from "node:fs/promises";
import { basename, dirname, join } from "node:path";

const tagPattern = /^anydash@v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
const api = "https://api.github.com/repos/delinoio/apps/releases?per_page=100";
const MAX_ARCHIVE = 600 * 1024 * 1024;

export function compareVersions(a, b) {
  const parse = (value) =>
    String(value)
      .replace(/^anydash@v/, "")
      .split(".")
      .map((part) => BigInt(part));
  const [left, right] = [parse(a), parse(b)];
  for (let index = 0; index < 3; index++) {
    if (left[index] !== right[index])
      return left[index] > right[index] ? 1 : -1;
  }
  return 0;
}

export const platformKey = (
  platform = process.platform,
  arch = process.arch,
) =>
  platform === "darwin" && ["arm64", "x64"].includes(arch)
    ? `darwin-${arch}`
    : platform === "win32" && arch === "x64"
      ? "windows-x64"
      : undefined;

/** Newest stable release above the installed version, with its assets. */
export function selectRelease(releases, installed, key) {
  if (!Array.isArray(releases))
    throw new Error("Could not read the release list.");
  const latest = releases
    .filter(
      (release) =>
        !release.draft &&
        !release.prerelease &&
        tagPattern.test(release.tag_name),
    )
    .sort((a, b) => compareVersions(b.tag_name, a.tag_name))[0];
  if (!latest || compareVersions(latest.tag_name, installed) <= 0)
    return undefined;
  const version = latest.tag_name.replace("anydash@v", "");
  const asset = (name) => {
    const matches = (latest.assets ?? []).filter(
      (entry) => entry.name === name,
    );
    if (
      matches.length !== 1 ||
      !Number.isSafeInteger(matches[0].size) ||
      matches[0].size <= 0
    )
      throw new Error(
        "The latest release is missing a complete AnyDash update.",
      );
    const url = new URL(matches[0].browser_download_url);
    if (
      url.origin !== "https://github.com" ||
      url.search ||
      url.hash ||
      decodeURIComponent(url.pathname) !==
        `/delinoio/apps/releases/download/${latest.tag_name}/${name}`
    )
      throw new Error("The release download address is invalid.");
    return matches[0];
  };
  return {
    version,
    archive: asset(`anydash-desktop-${key}-${version}.tar.gz`),
    sums: asset("SHA256SUMS"),
    signature: asset("SHA256SUMS.sig"),
  };
}

/** Verifies the Ed25519 signature over SHA256SUMS with the embedded key. */
export function verifySums(sums, signature, publicKey) {
  const key = createPublicKey({
    key: Buffer.from(publicKey, "base64"),
    format: "der",
    type: "spki",
  });
  return verify(
    null,
    sums,
    key,
    Buffer.from(signature.toString("utf8").trim(), "base64"),
  );
}

export function expectedDigest(sums, name) {
  for (const line of sums.toString("utf8").split("\n")) {
    const match = /^([a-f0-9]{64}) [ *](.+)$/.exec(line.trim());
    if (match && match[2] === name) return match[1];
  }
  throw new Error("The checksum list does not cover this update.");
}

function run(command, args, options = {}) {
  return new Promise((resolve) => {
    const child = spawn(command, args, {
      stdio: ["ignore", "pipe", "pipe"],
      windowsHide: true,
      ...options,
    });
    const chunks = [];
    // codesign prints signing details on stderr; collect both streams.
    child.stdout?.on("data", (chunk) => chunks.push(chunk));
    child.stderr?.on("data", (chunk) => chunks.push(chunk));
    child.on("error", () => resolve({ code: -1, output: "" }));
    child.on("exit", (code) =>
      resolve({ code, output: Buffer.concat(chunks).toString("utf8") }),
    );
  });
}

/** OS code-signature check of the extracted application. */
export async function verifyCodeSignature(
  app,
  { platform = process.platform, teamId, publisher, exec = run },
) {
  if (platform === "darwin") {
    const verified = await exec("/usr/bin/codesign", [
      "--verify",
      "--deep",
      "--strict",
      app,
    ]);
    if (verified.code !== 0) return false;
    if (!teamId) return false;
    const details = await exec("/usr/bin/codesign", [
      "-dv",
      "--verbose=2",
      app,
    ]);
    return details.output.includes(`TeamIdentifier=${teamId}`);
  }
  if (platform === "win32") {
    const exe = join(app, "AnyDash.exe");
    const script = `$s = Get-AuthenticodeSignature -LiteralPath '${exe.replace(/'/g, "''")}'; if ($s.Status -ne 'Valid') { exit 1 }; [Console]::Out.Write($s.SignerCertificate.Subject)`;
    const result = await exec("powershell.exe", [
      "-NoProfile",
      "-NonInteractive",
      "-Command",
      script,
    ]);
    return (
      result.code === 0 && !!publisher && result.output.includes(publisher)
    );
  }
  return false;
}

const messages = {
  idle: "Check for a newer version of AnyDash.",
  checking: "Checking for updates…",
  current: "AnyDash is up to date.",
  downloading: "Downloading update…",
  verifying: "Verifying the update signature…",
  ready: "An update is ready. Install it when you are ready; AnyDash restarts.",
  installing: "Installing update. AnyDash restarts…",
};

export class DesktopUpdater {
  constructor({
    config,
    install,
    stateDirectory,
    fetcher = fetch,
    platform = process.platform,
    arch = process.arch,
    beforeInstall = async () => {},
    relaunch = () => {},
    log = () => {},
    exec = run,
  }) {
    Object.assign(this, {
      config,
      install,
      stateDirectory,
      fetcher,
      platform,
      arch,
      beforeInstall,
      relaunch,
      log,
      exec,
    });
    this.key = platformKey(platform, arch);
    this.state = {
      phase: "idle",
      installedVersion: config.version,
      message: messages.idle,
    };
    if (!config.updatePublicKey || !config.releaseBuild || !this.key)
      this.state = {
        ...this.state,
        phase: "unavailable",
        message:
          "In-app updates require an official signed AnyDash release. Development builds cannot update.",
      };
  }

  set(phase, extra = {}) {
    this.state = {
      ...this.state,
      phase,
      message: messages[phase] ?? this.state.message,
      ...extra,
    };
    this.log({ event: "anydash_update_state", phase });
  }

  async #download(asset, limit) {
    const response = await this.fetcher(asset.browser_download_url, {
      redirect: "follow",
    });
    if (!response.ok) throw new Error("The update download failed.");
    const body = Buffer.from(await response.arrayBuffer());
    if (body.length !== asset.size || body.length > limit)
      throw new Error("The update download is incomplete.");
    return body;
  }

  async check() {
    if (this.state.phase === "unavailable") return this.state;
    try {
      this.set("checking");
      const response = await this.fetcher(api, {
        headers: { accept: "application/vnd.github+json" },
      });
      if (!response.ok) throw new Error("Could not reach GitHub Releases.");
      const release = selectRelease(
        await response.json(),
        this.config.version,
        this.key,
      );
      if (!release) {
        this.set("current");
        return this.state;
      }
      this.set("downloading", { availableVersion: release.version });
      const [sums, signature, archive] = [
        await this.#download(release.sums, 1024 * 1024),
        await this.#download(release.signature, 4096),
        await this.#download(release.archive, MAX_ARCHIVE),
      ];
      this.set("verifying");
      if (!verifySums(sums, signature, this.config.updatePublicKey))
        throw new Error(
          "The update signature is invalid. Nothing was installed.",
        );
      const digest = createHash("sha256").update(archive).digest("hex");
      if (digest !== expectedDigest(sums, release.archive.name))
        throw new Error(
          "The update checksum does not match. Nothing was installed.",
        );
      const staging = await mkdtemp(
        join(dirname(this.install), ".anydash-update-"),
      );
      const file = join(staging, release.archive.name);
      await writeFile(file, archive);
      const extracted = join(staging, "app");
      await mkdir(extracted);
      // System tar (bsdtar on macOS and Windows) extracts without following links.
      if ((await this.exec("tar", ["-xzf", file, "-C", extracted])).code !== 0)
        throw new Error("The update archive is unreadable.");
      const [entry] = await readdir(extracted);
      const app = join(extracted, entry ?? "");
      if (
        !entry ||
        !(await verifyCodeSignature(app, {
          platform: this.platform,
          teamId: this.config.teamId,
          publisher: this.config.publisher,
          exec: this.exec,
        }))
      )
        throw new Error(
          "The update’s code signature is invalid. Nothing was installed.",
        );
      this.staged = { staging, app };
      this.set("ready");
    } catch (error) {
      this.set("error", {
        message: error instanceof Error ? error.message : "The update failed.",
      });
    }
    return this.state;
  }

  /** Runs after the user confirms. The swap happens once this process exits. */
  async installConfirmed() {
    if (this.state.phase !== "ready" || !this.staged) return this.state;
    this.set("installing");
    await this.beforeInstall();
    const { app, staging } = this.staged;
    const backup = `${this.install}.previous`;
    // The swap keeps the previous app beside the new one until the next update.
    const ps = (value) => `'${value.replace(/'/g, "''")}'`;
    const sh = (value) => `'${value.replace(/'/g, "'\\''")}'`;
    const script =
      this.platform === "win32"
        ? `$p=${process.pid}; while (Get-Process -Id $p -ErrorAction SilentlyContinue) { Start-Sleep -Milliseconds 200 }; Remove-Item -LiteralPath ${ps(backup)} -Recurse -Force -ErrorAction SilentlyContinue; Move-Item -LiteralPath ${ps(this.install)} -Destination ${ps(backup)}; Move-Item -LiteralPath ${ps(app)} -Destination ${ps(this.install)}; Start-Process -FilePath ${ps(join(this.install, "AnyDash.exe"))}; Remove-Item -LiteralPath ${ps(staging)} -Recurse -Force`
        : `while kill -0 ${process.pid} 2>/dev/null; do sleep 0.2; done; rm -rf ${sh(backup)}; mv ${sh(this.install)} ${sh(backup)} && mv ${sh(app)} ${sh(this.install)} && open ${sh(this.install)}; rm -rf ${sh(staging)}`;
    const child =
      this.platform === "win32"
        ? spawn(
            "powershell.exe",
            ["-NoProfile", "-NonInteractive", "-Command", script],
            { detached: true, stdio: "ignore", windowsHide: true },
          )
        : spawn("/bin/sh", ["-c", script], { detached: true, stdio: "ignore" });
    child.unref();
    this.log({
      event: "anydash_update_install_started",
      target: basename(this.install),
    });
    this.relaunch();
    return this.state;
  }

  async act(action) {
    if (action === "check") void this.check();
    else if (action === "install") return this.installConfirmed();
    return this.state;
  }

  async cleanup() {
    for (const entry of await readdir(dirname(this.install)).catch(() => []))
      if (entry.startsWith(".anydash-update-"))
        await rm(join(dirname(this.install), entry), {
          recursive: true,
          force: true,
        });
  }
}
