// SPDX-License-Identifier: Apache-2.0
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { isAbsolute, join, resolve } from "node:path";
import { parseEnv } from "node:util";
import { fileURLToPath } from "node:url";
import { Api, databaseHandoff } from "./dev.mjs";
import { exitLikeChild, spawnDevServer } from "./spawn-dev-server.mjs";

export function prepareApiEnvironment(
  api,
  source = process.env,
  cwd = process.cwd(),
) {
  if (!Object.values(Api).includes(api))
    throw new Error("Unknown development API");
  const env = { ...source };
  delete env[databaseHandoff];
  if (!source[databaseHandoff]) return { env, cleanup() {} };
  const url = JSON.parse(source[databaseHandoff])[api];
  if (typeof url !== "string" || new URL(url).hostname !== "127.0.0.1")
    throw new Error("Missing local database handoff");
  if (api === Api.GitTracker) {
    env.DATABASE_URL = url;
    return { env, cleanup() {} };
  }
  let configPath = source.CONFIG;
  if (!configPath) {
    try {
      configPath = parseEnv(readFileSync(join(cwd, ".env"), "utf8")).CONFIG;
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
  }
  if (!configPath || !isAbsolute(configPath))
    throw new Error("An absolute source CONFIG is required");
  const config = JSON.parse(readFileSync(configPath, "utf8"));
  if (!config || typeof config !== "object" || Array.isArray(config))
    throw new Error("Invalid source CONFIG");
  const directory = mkdtempSync(join(tmpdir(), "delcore-dev-"));
  const cleanup = () => rmSync(directory, { recursive: true, force: true });
  try {
    env.CONFIG = join(directory, "config.json");
    writeFileSync(
      env.CONFIG,
      JSON.stringify({ ...config, database_url: url }),
      { mode: 0o600 },
    );
    return { env, cleanup };
  } catch (error) {
    cleanup();
    throw error;
  }
}

export async function main(api = process.argv[2]) {
  const prepared = prepareApiEnvironment(api);
  try {
    console.log(
      JSON.stringify({
        event: "dev_api_start",
        service: api,
        compose: Boolean(process.env[databaseHandoff]),
      }),
    );
    return await spawnDevServer(
      "go",
      ["run", "github.com/air-verse/air@v1.67.4", "-c", ".air.toml"],
      {
        stdio: "inherit",
        env: prepared.env,
      },
      { terminateProcessTree: true },
    );
  } finally {
    prepared.cleanup();
  }
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
        event: "dev_api_start_failed",
        message:
          "Could not prepare API development. Check the source CONFIG, Compose handoff and Go installation.",
      }),
    );
    process.exitCode = 1;
  }
}
