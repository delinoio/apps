// SPDX-License-Identifier: FSL-1.1-ALv2
import { execFileSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  exitLikeChild,
  spawnDevServer,
} from "../../scripts/spawn-dev-server.mjs";

const serverRoot = fileURLToPath(new URL("../", import.meta.url));

export async function main() {
  const metadata = JSON.parse(
    execFileSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
      cwd: serverRoot,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "inherit"],
    }),
  );
  // The workspace is empty until the first port. Do not invent a placeholder
  // crate just to make cargo build succeed; remove this branch once populated.
  if (metadata.workspace_members.length === 0) {
    console.log(
      JSON.stringify({
        event: "server_build_skipped",
        reason: "empty_workspace",
      }),
    );
    return { code: 0, signal: null };
  }
  console.log(JSON.stringify({ event: "server_build_started" }));
  return spawnDevServer(
    "cargo",
    ["build", "--workspace", "--bins"],
    { cwd: serverRoot, stdio: "inherit" },
    { terminateProcessTree: true },
  );
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  try {
    exitLikeChild(await main());
  } catch {
    console.error(
      JSON.stringify({
        event: "server_build_failed",
        message:
          "Could not build the server Cargo workspace. Check the Cargo diagnostics.",
      }),
    );
    process.exitCode = 1;
  }
}
