// SPDX-License-Identifier: Apache-2.0
// Compiles and runs the shared Swift widget decoder tests on macOS.
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
if (process.platform !== "darwin") {
  console.info(
    JSON.stringify({
      event: "anydash_swift_widget_tests_skipped",
      reason: "requires macOS",
    }),
  );
  process.exit(0);
}
const out = mkdtempSync(join(tmpdir(), "anydash-swift-"));
try {
  const binary = join(out, "decoder-tests");
  for (const [command, args] of [
    [
      "swiftc",
      [
        "-O",
        "-o",
        binary,
        "native/widgets/shared/AnyDashSnapshot.swift",
        "native/widgets/shared/Tests/main.swift",
      ],
    ],
    [binary, [join(root, "native/widgets/fixtures")]],
  ]) {
    const result = spawnSync(command, args, { cwd: root, stdio: "inherit" });
    if (result.status !== 0) throw new Error(`${command} failed`);
  }
} finally {
  rmSync(out, { recursive: true, force: true });
}
