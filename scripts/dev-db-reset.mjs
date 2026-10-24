// SPDX-License-Identifier: Apache-2.0
import { execFileSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { Api, composeDatabaseUrls } from "./dev.mjs";
import { exitLikeChild, spawnDevServer } from "./spawn-dev-server.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const serverRoot = resolve(root, "servers");

function readComposeConfig() {
  return JSON.parse(
    execFileSync("docker", ["compose", "config", "--format", "json"], {
      cwd: root,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "inherit"],
    }),
  );
}

export async function main({
  run = spawnDevServer,
  readConfig = readComposeConfig,
  env = process.env,
} = {}) {
  const urls = composeDatabaseUrls(readConfig());
  const target = resolve(serverRoot, env.CARGO_TARGET_DIR || "target");
  const steps = [
    {
      event: "dev_db_build",
      command: "cargo",
      args: [
        "build",
        "--locked",
        "--workspace",
        "--bins",
        "--target-dir",
        target,
      ],
      cwd: serverRoot,
    },
    {
      event: "dev_db_remove",
      command: "docker",
      args: ["compose", "down", "--volumes"],
      cwd: root,
    },
    {
      event: "dev_db_start",
      command: "docker",
      args: ["compose", "up", "-d", "--wait"],
      cwd: root,
    },
    ...Object.values(Api).map((api) => ({
      event: "dev_db_migrate",
      service: api,
      command: resolve(
        target,
        "debug",
        `${api}-api${process.platform === "win32" ? ".exe" : ""}`,
      ),
      args: ["migrate", "--dev-env"],
      cwd: resolve(serverRoot, `${api}-api`),
      env: { ...env, DATABASE_URL: urls[api] },
    })),
  ];
  for (const step of steps) {
    console.log(JSON.stringify({ event: step.event, service: step.service }));
    const result = await run(
      step.command,
      step.args,
      { cwd: step.cwd, env: step.env || env, stdio: "inherit" },
      { terminateProcessTree: true },
    );
    if (result.code !== 0 || result.signal) return result;
  }
  console.log(JSON.stringify({ event: "dev_db_reset_complete" }));
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
        event: "dev_db_reset_failed",
        message:
          "Could not reset development databases. Check Docker, Cargo and service-local migration configuration.",
      }),
    );
    process.exitCode = 1;
  }
}
