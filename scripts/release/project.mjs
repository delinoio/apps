// SPDX-License-Identifier: Apache-2.0
export const Bump = Object.freeze({
  Patch: "patch",
  Minor: "minor",
  Major: "major",
});
const stable = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/u;
function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}
export function bumpVersion(version, bump) {
  requireValue(
    stable.test(version),
    "Expected a stable MAJOR.MINOR.PATCH version",
  );
  requireValue(Object.values(Bump).includes(bump), "Unknown version bump");
  const parts = version.split(".").map(BigInt);
  const index = { major: 0, minor: 1, patch: 2 }[bump];
  parts[index] += 1n;
  for (let i = index + 1; i < 3; i++) parts[i] = 0n;
  requireValue(
    parts.every((part) => part <= 2147483647n),
    "Version component overflow",
  );
  return parts.join(".");
}
