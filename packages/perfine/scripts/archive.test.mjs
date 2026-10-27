// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import test from "node:test";
import { gzipSync, gunzipSync } from "node:zlib";
import { normalizeLauncherArchive } from "./archive.mjs";

function fixture(mode, name = "package/bin/perfine.cjs") {
  const tar = Buffer.alloc(2048);
  tar.write(name, 0);
  tar.write(`${mode.toString(8).padStart(7, "0")}\0`, 100);
  tar.write("0000000\0", 108);
  tar.write("0000000\0", 116);
  tar.write("00000000003\0", 124);
  tar.write("00000000000\0", 136);
  tar[156] = 48;
  tar.write("ustar\0", 257);
  tar.write("00", 263);
  tar.fill(32, 148, 156);
  const checksum = tar.subarray(0, 512).reduce((sum, byte) => sum + byte, 0);
  tar.write(`${checksum.toString(8).padStart(6, "0")}\0 `, 148);
  tar.write("bin", 512);
  return gzipSync(tar);
}

test("Windows and POSIX launcher permissions produce byte-identical npm entry archives", () => {
  const windows = fixture(0o644);
  const posix = fixture(0o755);
  windows[9] = 11;
  posix[9] = 3;
  assert.notDeepEqual(windows, posix);
  const normalized = normalizeLauncherArchive(windows);
  assert.deepEqual(normalized, normalizeLauncherArchive(posix));
  assert.deepEqual(normalized, normalizeLauncherArchive(normalized));
  const tar = gunzipSync(normalized);
  assert.equal(tar.subarray(100, 108).toString(), "0000755\0");
  assert.equal(tar.subarray(512, 515).toString(), "bin");
  const expected = Number.parseInt(tar.subarray(148, 154).toString(), 8);
  tar.fill(32, 148, 156);
  assert.equal(
    tar.subarray(0, 512).reduce((sum, byte) => sum + byte, 0),
    expected,
  );
});

test("archive normalization rejects a missing launcher and truncated tar entries", () => {
  assert.throws(
    () => normalizeLauncherArchive(fixture(0o644, "package/other")),
    /exactly one/,
  );
  const tar = gunzipSync(fixture(0o644));
  tar.write("77777777777\0", 124);
  assert.throws(() => normalizeLauncherArchive(gzipSync(tar)), /Truncated/);
});
