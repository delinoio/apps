// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { Api } from "./dev.mjs";
import { main } from "./dev-db-reset.mjs";

const serverRoot = fileURLToPath(new URL("../servers/", import.meta.url));
function config() {
  return {
    services: Object.fromEntries(
      Object.values(Api).map((api, index) => [
        `${api}-postgres`,
        {
          ports: [
            {
              target: 5432,
              published: String(15432 + index),
              host_ip: "127.0.0.1",
              protocol: "tcp",
            },
          ],
          environment: {
            POSTGRES_USER: api,
            POSTGRES_PASSWORD: "fixture",
            POSTGRES_DB: api,
          },
        },
      ]),
    ),
  };
}

test("reset migrates every fresh database using Compose URLs and service-local configuration", async () => {
  const calls = [];
  const env = {
    CARGO_TARGET_DIR: "fixture-target",
    DATABASE_URL: "postgres://remote.invalid/db",
    COMPOSE_PROJECT_NAME: "fixture-project",
    DELCORE_POSTGRES_PORT: "15432",
    IDENTITY_KEY: "fixture-identity",
  };
  assert.deepEqual(
    await main({
      readConfig: config,
      env,
      run: async (command, args, options) => {
        calls.push({ command, args, options });
        return { code: 0, signal: null };
      },
    }),
    { code: 0, signal: null },
  );
  assert.equal(calls.length, 3 + Object.values(Api).length);
  const target = resolve(serverRoot, "fixture-target");
  assert.equal(calls[0].command, "cargo");
  assert.deepEqual(calls[0].args, [
    "build",
    "--locked",
    "--workspace",
    "--bins",
    "--target-dir",
    target,
  ]);
  assert.equal(calls[0].options.cwd, serverRoot.replace(/\/$/u, ""));
  assert.deepEqual(
    calls.slice(1, 3).map(({ command, args }) => [command, ...args]),
    [
      ["docker", "compose", "down", "--volumes"],
      ["docker", "compose", "up", "-d", "--wait"],
    ],
  );
  for (const [index, api] of Object.values(Api).entries()) {
    const call = calls[index + 3];
    assert.equal(
      call.command,
      resolve(
        target,
        "debug",
        `${api}-api${process.platform === "win32" ? ".exe" : ""}`,
      ),
    );
    assert.deepEqual(call.args, ["migrate", "--dev-env"]);
    assert.equal(call.options.cwd, resolve(serverRoot, `${api}-api`));
    assert.equal(
      call.options.env.DATABASE_URL,
      `postgres://${api}:fixture@127.0.0.1:${15432 + index}/${api}?sslmode=disable`,
    );
    assert.equal(call.options.env.IDENTITY_KEY, env.IDENTITY_KEY);
  }
  assert.ok(
    calls.every(
      ({ options }) =>
        options.env.COMPOSE_PROJECT_NAME === env.COMPOSE_PROJECT_NAME &&
        options.env.DELCORE_POSTGRES_PORT === "15432",
    ),
  );
  assert.equal(env.DATABASE_URL, "postgres://remote.invalid/db");
});

for (
  let failedStep = 0;
  failedStep < 3 + Object.values(Api).length;
  failedStep++
) {
  for (const outcome of [
    { code: 7, signal: null },
    { code: null, signal: "SIGINT" },
  ]) {
    test(`reset stops at step ${failedStep} on ${outcome.signal || "failure"}`, async () => {
      let count = 0;
      const result = await main({
        readConfig: config,
        run: async () =>
          count++ === failedStep ? outcome : { code: 0, signal: null },
      });
      assert.deepEqual(result, outcome);
      assert.equal(count, failedStep + 1);
    });
  }
}

test("reset rejects nonlocal Compose configuration before build or deletion", async () => {
  const invalid = config();
  invalid.services["delcore-postgres"].ports[0].host_ip = "0.0.0.0";
  await assert.rejects(
    main({
      readConfig: () => invalid,
      run: () => assert.fail("must not execute commands"),
    }),
    /Invalid local Compose/,
  );
});
