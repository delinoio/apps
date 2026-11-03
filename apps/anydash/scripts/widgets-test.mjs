// SPDX-License-Identifier: Apache-2.0
// Runs the native widget decoder tests: the C# Widgets Board tests on any host
// with a .NET 8 SDK, and the Swift decoder and macOS collector client tests on
// macOS. Android decoder tests run with Gradle (testDebugUnitTest).
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const dotnet = process.env.DOTNET || "dotnet";
const probe = spawnSync(dotnet, ["--version"], { stdio: "ignore" });
if (probe.status === 0) {
  const result = spawnSync(dotnet, ["test", "native/widgets/windows/Tests"], {
    cwd: root,
    stdio: "inherit",
    env: { ...process.env, DOTNET_CLI_TELEMETRY_OPTOUT: "1" },
  });
  rmSync(join(root, "native/widgets/windows/Tests/bin"), {
    recursive: true,
    force: true,
  });
  rmSync(join(root, "native/widgets/windows/Tests/obj"), {
    recursive: true,
    force: true,
  });
  if (result.status !== 0) throw new Error("dotnet test failed");
} else
  console.info(
    JSON.stringify({
      event: "anydash_dotnet_widget_tests_skipped",
      reason: "requires a .NET 8 SDK",
    }),
  );
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
  // The shared decoder, then the macOS host's collector client.
  const suites = [
    ["decoder-tests", ["native/widgets/shared/Tests/main.swift"]],
    [
      "collector-tests",
      [
        "native/widgets/macos/Widgets/CollectorClient.swift",
        "native/widgets/macos/Tests/main.swift",
      ],
    ],
  ];
  for (const [name, sources] of suites) {
    const binary = join(out, name);
    for (const [command, args] of [
      [
        "swiftc",
        [
          "-O",
          "-o",
          binary,
          "native/widgets/shared/AnyDashSnapshot.swift",
          ...sources,
        ],
      ],
      [binary, [join(root, "native/widgets/fixtures")]],
    ]) {
      const result = spawnSync(command, args, { cwd: root, stdio: "inherit" });
      if (result.status !== 0) throw new Error(`${command} failed`);
    }
  }
} finally {
  rmSync(out, { recursive: true, force: true });
}
