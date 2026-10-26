// SPDX-License-Identifier: Apache-2.0
import { createPublicKey, verify } from "node:crypto";

export const releasePrefix = "lane-desktop@v";
export const repository = "delinoio/apps";
export const maximumArchiveSize = 2 * 1024 ** 3;

export function versionParts(version) {
  if (
    typeof version !== "string" ||
    !/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(version)
  )
    throw new Error("Desktop updates require a stable semantic version.");
  const parts = version.split(".").map(Number);
  if (!parts.every(Number.isSafeInteger)) throw new Error("Invalid version.");
  return parts;
}

export function newerVersion(candidate, current) {
  const left = versionParts(candidate),
    right = versionParts(current);
  for (let i = 0; i < left.length; i++) {
    if (left[i] !== right[i]) return left[i] > right[i];
  }
  return false;
}

export function updatePublicKey(value) {
  const key = createPublicKey({
    key: Buffer.from(value, "base64"),
    format: "der",
    type: "spki",
  });
  if (key.asymmetricKeyType !== "ed25519")
    throw new Error("An Ed25519 update key is required.");
  return key;
}

export function desktopUpdateConfig({
  publicKey,
  scheme,
  testing,
  version,
  platform,
  arch,
}) {
  versionParts(version);
  if (!publicKey) return undefined;
  if (scheme !== "lane" || testing)
    throw new Error(
      "Update-enabled builds require the lane scheme and production native code.",
    );
  updatePublicKey(publicKey);
  return { publicKey, target: targetName(platform, arch) };
}

export function targetName(platform, arch) {
  if (
    !["darwin", "win32"].includes(platform) ||
    !["arm64", "x64"].includes(arch)
  )
    throw new Error("Unsupported desktop update target.");
  return `${platform}-${arch}`;
}

export function archiveName(version, target) {
  versionParts(version);
  return `Lane-${version}-${target}.zip`;
}

export function assetURL(version, name) {
  versionParts(version);
  return `https://github.com/${repository}/releases/download/${releasePrefix}${version}/${name}`;
}

export function verifyManifest(
  envelope,
  { publicKey, version, platform, arch },
) {
  if (
    typeof envelope?.payload !== "string" ||
    typeof envelope?.signature !== "string"
  )
    throw new Error("Invalid update envelope.");
  const bytes = Buffer.from(envelope.payload, "base64");
  if (
    !verify(
      null,
      bytes,
      updatePublicKey(publicKey),
      Buffer.from(envelope.signature, "base64"),
    )
  )
    throw new Error("Invalid desktop update signature.");
  const manifest = JSON.parse(bytes.toString("utf8"));
  const target = targetName(platform, arch);
  if (
    manifest.schema !== 1 ||
    manifest.version !== version ||
    manifest.target !== target ||
    manifest.url !== assetURL(version, archiveName(version, target)) ||
    !/^[a-f0-9]{64}$/.test(manifest.sha256) ||
    !Number.isSafeInteger(manifest.size) ||
    manifest.size <= 0 ||
    manifest.size > maximumArchiveSize
  )
    throw new Error("Invalid desktop update manifest.");
  return manifest;
}
