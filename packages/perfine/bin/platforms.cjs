// SPDX-License-Identifier: Apache-2.0
const targets = Object.freeze({
  "x86_64-unknown-linux-gnu": {
    os: "linux",
    cpu: "x64",
    suffix: "linux-x86_64",
    executable: "perfine",
  },
  "x86_64-apple-darwin": {
    os: "darwin",
    cpu: "x64",
    suffix: "darwin-x86_64",
    executable: "perfine",
  },
  "aarch64-apple-darwin": {
    os: "darwin",
    cpu: "arm64",
    suffix: "darwin-arm64",
    executable: "perfine",
  },
  "x86_64-pc-windows-msvc": {
    os: "win32",
    cpu: "x64",
    suffix: "win32-x86_64",
    executable: "perfine.exe",
  },
});
function targetInfo(target) {
  if (!Object.hasOwn(targets, target))
    throw new Error(`Unsupported Perfine target: ${target}`);
  const info = targets[target];
  return {
    ...info,
    target,
    packageName: `@delinoio/perfine-${info.os}-${info.cpu}`,
  };
}
function hostTarget(os = process.platform, cpu = process.arch) {
  const target = Object.keys(targets).find(
    (key) => targets[key].os === os && targets[key].cpu === cpu,
  );
  if (!target) throw new Error(`Unsupported Perfine platform: ${os}/${cpu}`);
  return targetInfo(target);
}
function validateBinary(bytes, target) {
  const info = targetInfo(target);
  let valid = false;
  if (info.os === "linux") {
    valid =
      bytes.length >= 64 &&
      bytes.subarray(0, 4).toString("hex") === "7f454c46" &&
      bytes[4] === 2 &&
      bytes[5] === 1 &&
      bytes.readUInt16LE(18) === 62;
  } else if (info.os === "darwin") {
    valid =
      bytes.length >= 32 &&
      bytes.readUInt32LE(0) === 0xfeedfacf &&
      bytes.readUInt32LE(4) ===
        (info.cpu === "arm64" ? 0x0100000c : 0x01000007) &&
      bytes.readUInt32LE(12) === 2;
  } else if (bytes.length >= 64 && bytes.toString("ascii", 0, 2) === "MZ") {
    const offset = bytes.readUInt32LE(60);
    valid =
      offset >= 64 &&
      offset + 26 <= bytes.length &&
      bytes.readUInt32LE(offset) === 0x00004550 &&
      bytes.readUInt16LE(offset + 4) === 0x8664 &&
      bytes.readUInt16LE(offset + 24) === 0x20b &&
      (bytes.readUInt16LE(offset + 22) & 2) !== 0 &&
      (bytes.readUInt16LE(offset + 22) & 0x2000) === 0;
  }
  if (!valid)
    throw new Error(
      `Expected ${info.os === "linux" ? "ELF" : info.os === "darwin" ? "Mach-O" : "PE"} executable for ${target}`,
    );
  return bytes;
}
module.exports = { targets, targetInfo, hostTarget, validateBinary };
