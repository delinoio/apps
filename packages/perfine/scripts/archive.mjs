// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { gzipSync, gunzipSync } from "node:zlib";

// npm pack records the launcher as 0644 on Windows despite chmod(0755).
// Normalize the generated entry archive before checksums and native smoke tests;
// remove this adaptation when npm preserves the same bin mode on every host.
export function normalizeLauncherArchive(archive) {
  const tar = gunzipSync(archive);
  let found = 0;
  for (let offset = 0; offset + 512 <= tar.length;) {
    const header = tar.subarray(offset, offset + 512);
    if (header.every((byte) => byte === 0)) break;
    const name = header.subarray(0, 100).toString().split("\0")[0];
    const size = Number.parseInt(
      header.subarray(124, 136).toString().replace(/\0/g, "").trim(),
      8,
    );
    assert.ok(
      Number.isSafeInteger(size) && size >= 0,
      "Invalid npm tar entry size",
    );
    assert.ok(offset + 512 + size <= tar.length, "Truncated npm tar entry");
    if (name === "package/bin/perfine.cjs") {
      assert.ok(
        header[156] === 0 || header[156] === 48,
        "Launcher tar entry must be a regular file",
      );
      found++;
      header.write("0000755\0", 100, 8, "ascii");
      header.fill(32, 148, 156);
      const checksum = header.reduce((sum, byte) => sum + byte, 0);
      header.write(
        `${checksum.toString(8).padStart(6, "0")}\0 `,
        148,
        8,
        "ascii",
      );
    }
    offset += 512 + Math.ceil(size / 512) * 512;
  }
  assert.equal(found, 1, "Expected exactly one npm launcher entry");
  const normalized = gzipSync(tar, { level: 9 });
  // The gzip OS marker is metadata, not an executable platform constraint.
  normalized[9] = 255;
  return normalized;
}
