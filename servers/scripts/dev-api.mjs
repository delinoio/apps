// SPDX-License-Identifier: FSL-1.1-ALv2
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { Api, databaseHandoff } from "../../scripts/dev.mjs";
import {
  exitLikeChild,
  spawnDevServer,
} from "../../scripts/spawn-dev-server.mjs";

export function prepareApiEnvironment(api, source = process.env) {
  if (!Object.values(Api).includes(api))
    throw new Error("Unknown development API");
  const env = {
    ...source,
    // Development listeners are fixed per service, including when a shared
    // parent shell or Infisical value supplies LISTEN.
    LISTEN: {
      [Api.Delcore]: "127.0.0.1:8080",
      [Api.Delitools]: "127.0.0.1:46322",
      [Api.DevBase]: "127.0.0.1:46317",
      [Api.GitTracker]: "127.0.0.1:46321",
    }[api],
  };
  delete env[databaseHandoff];
  if (source[databaseHandoff]) {
    const url = JSON.parse(source[databaseHandoff])[api];
    if (typeof url !== "string" || new URL(url).hostname !== "127.0.0.1")
      throw new Error("Missing local database handoff");
    env.DATABASE_URL = url;
  }
  return { env };
}

export function apiCommand(api, source = process.env) {
  const serverRoot = fileURLToPath(new URL("../", import.meta.url));
  const target = resolve(serverRoot, source.CARGO_TARGET_DIR || "target");
  const binary = {
    [Api.Delcore]: "delcore-api",
    [Api.Delitools]: "delitools-api",
    [Api.DevBase]: "devbase-api",
    [Api.GitTracker]: "git-tracker-api",
  }[api];
  if (!binary) throw new Error("Unknown development API");
  return [
    resolve(
      target,
      "debug",
      `${binary}${process.platform === "win32" ? ".exe" : ""}`,
    ),
    ["serve", "--dev-env"],
  ];
}

export async function main(api = process.argv[2]) {
  const prepared = prepareApiEnvironment(api);
  console.log(
    JSON.stringify({
      event: "dev_api_start",
      service: api,
      compose: Boolean(process.env[databaseHandoff]),
    }),
  );
  const [command, args] = apiCommand(api);
  return await spawnDevServer(
    command,
    args,
    {
      stdio: "inherit",
      env: prepared.env,
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
        event: "dev_api_start_failed",
        message:
          "Could not prepare API development. Check the service .env, Compose handoff and compiled server binaries.",
      }),
    );
    process.exitCode = 1;
  }
}
