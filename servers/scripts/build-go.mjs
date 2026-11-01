// SPDX-License-Identifier: FSL-1.1-ALv2
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  exitLikeChild,
  spawnDevServer,
} from "../../scripts/spawn-dev-server.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const serverRoot = fileURLToPath(new URL("../", import.meta.url));

// Go servers are packages in the root module. They write development binaries
// beside the Cargo outputs so the shared API launcher executes both the same way.
export const GoServer = Object.freeze({
  Mcpa: "mcpa-api",
});

export function goBuildSteps(env = process.env) {
  const target = resolve(serverRoot, env.CARGO_TARGET_DIR || "target");
  return Object.values(GoServer).map((server) => ({
    server,
    command: "go",
    args: [
      "build",
      "-o",
      resolve(
        target,
        "debug",
        `${server}${process.platform === "win32" ? ".exe" : ""}`,
      ),
      `./servers/${server}`,
    ],
    cwd: root,
  }));
}

export async function main({ run = spawnDevServer, env = process.env } = {}) {
  for (const step of goBuildSteps(env)) {
    console.log(
      JSON.stringify({ event: "server_go_build_started", server: step.server }),
    );
    const result = await run(
      step.command,
      step.args,
      { cwd: step.cwd, env, stdio: "inherit" },
      { terminateProcessTree: true },
    );
    if (result.code !== 0 || result.signal) return result;
  }
  return { code: 0, signal: null };
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
        event: "server_go_build_failed",
        message:
          "Could not build the Go servers. Install the Go version selected by go.mod and check the compiler diagnostics.",
      }),
    );
    process.exitCode = 1;
  }
}
