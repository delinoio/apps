// SPDX-License-Identifier: Apache-2.0
import { execFileSync } from "node:child_process";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";
import { exitLikeChild, spawnDevServer } from "./spawn-dev-server.mjs";

export const Api = Object.freeze({
  Delcore: "delcore",
  DevBase: "devbase",
  GitTracker: "git-tracker",
});
export const databaseHandoff = "DEV_COMPOSE_DATABASE_URLS";
const root = fileURLToPath(new URL("../", import.meta.url));

export function composeDatabaseUrls(config) {
  return Object.fromEntries(
    Object.values(Api).map((api) => {
      const service = config.services?.[`${api}-postgres`];
      const port = service?.ports?.find(
        (item) => Number(item.target) === 5432 && item.protocol === "tcp",
      );
      const settings = service?.environment;
      if (
        port?.host_ip !== "127.0.0.1" ||
        !/^\d+$/u.test(String(port.published)) ||
        Number(port.published) < 1 ||
        Number(port.published) > 65535 ||
        ![
          settings?.POSTGRES_USER,
          settings?.POSTGRES_PASSWORD,
          settings?.POSTGRES_DB,
        ].every((value) => typeof value === "string" && value.length > 0)
      ) {
        throw new Error(
          `Invalid local Compose database configuration for ${api}`,
        );
      }
      const url = new URL("postgres://127.0.0.1");
      url.port = String(port.published);
      url.username = settings.POSTGRES_USER;
      url.password = settings.POSTGRES_PASSWORD;
      url.pathname = `/${encodeURIComponent(settings.POSTGRES_DB)}`;
      url.searchParams.set("sslmode", "disable");
      return [api, url.href];
    }),
  );
}

export async function main(args = process.argv.slice(2)) {
  const config = JSON.parse(
    execFileSync("docker", ["compose", "config", "--format", "json"], {
      cwd: root,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "inherit"],
    }),
  );
  const urls = composeDatabaseUrls(config);
  const result = await spawnDevServer(
    "docker",
    ["compose", "up", "-d", "--wait"],
    { cwd: root, stdio: "inherit" },
  );
  if (result.code !== 0 || result.signal) return result;
  console.log(
    JSON.stringify({
      event: "dev_compose_ready",
      services: Object.values(Api),
    }),
  );
  const require = createRequire(import.meta.url);
  return spawnDevServer(
    process.execPath,
    [require.resolve("turbo"), "run", "dev", ...args],
    {
      cwd: root,
      stdio: "inherit",
      env: { ...process.env, [databaseHandoff]: JSON.stringify(urls) },
    },
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
        event: "dev_start_failed",
        message:
          "Could not prepare Compose databases or launch workspace development. Check Docker and Compose configuration.",
      }),
    );
    process.exitCode = 1;
  }
}
