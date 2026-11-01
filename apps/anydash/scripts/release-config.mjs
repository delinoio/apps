// SPDX-License-Identifier: Apache-2.0
// Release identities for the signed native builds. The unified release passes
// the `anydash@vX.Y.Z` version and its GitHub Actions run number; credentials
// never pass through these values.
const semver = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;

/** Version and build number for store builds (build numbers 1..9999). */
export function releaseIdentity(env = process.env) {
  const version = env.ANYDASH_RELEASE_VERSION ?? "";
  const buildNumber = env.ANYDASH_BUILD_NUMBER ?? "";
  if (!semver.test(version))
    throw new Error("ANYDASH_RELEASE_VERSION must be MAJOR.MINOR.PATCH");
  if (!/^[1-9][0-9]*$/.test(buildNumber) || Number(buildNumber) > 9999)
    throw new Error("ANYDASH_BUILD_NUMBER must be an integer from 1 to 9999");
  return { version, buildNumber };
}

/** MSIX identity versions have four parts; the revision stays 0. */
export function appxVersion(version) {
  if (!semver.test(version ?? ""))
    throw new Error("ANYDASH_RELEASE_VERSION must be MAJOR.MINOR.PATCH");
  const parts = version.split(".").map(Number);
  if (parts.some((part) => part > 65535))
    throw new Error("MSIX version parts must be at most 65535");
  return `${version}.0`;
}

/** Rewrites the widget package identity for the signing certificate subject. */
export function releaseAppxManifest(manifest, { publisher, version }) {
  if (!/^CN=/.test(publisher ?? "") || /["<>&]/.test(publisher))
    throw new Error(
      "ANYDASH_WINDOWS_PUBLISHER must be the certificate subject",
    );
  const identity = /<Identity\b[^>]*\/>/;
  if (!identity.test(manifest)) throw new Error("Package identity missing");
  return manifest.replace(identity, (element) =>
    element
      .replace(/Publisher="[^"]*"/, `Publisher="${publisher}"`)
      .replace(/Version="[^"]*"/, `Version="${appxVersion(version)}"`),
  );
}

/** App Store Connect export options for the signed iOS archive. */
export function iosExportOptions(team) {
  if (!/^[A-Z0-9]{10}$/.test(team ?? ""))
    throw new Error("ANYDASH_APPLE_TEAM_ID must be a 10-character team ID");
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>method</key>
  <string>app-store-connect</string>
  <key>destination</key>
  <string>export</string>
  <key>signingStyle</key>
  <string>automatic</string>
  <key>teamID</key>
  <string>${team}</string>
  <key>uploadSymbols</key>
  <true/>
</dict>
</plist>
`;
}
