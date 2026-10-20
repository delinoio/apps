// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import {
  existsSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { Api, composeDatabaseUrls, databaseHandoff } from "./dev.mjs";
import { prepareApiEnvironment } from "./dev-api.mjs";

const urls = {
  [Api.Delcore]:
    "postgres://delcore:fixture@127.0.0.1:15432/delcore?sslmode=disable",
  [Api.GitTracker]:
    "postgres://git_tracker:fixture@127.0.0.1:15433/git_tracker?sslmode=disable",
};
const handoff = { [databaseHandoff]: JSON.stringify(urls) };
const fixture = (t) => {
  const directory = mkdtempSync(join(tmpdir(), "compose-dev-test-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  return directory;
};

test("Compose host ports and credentials determine URLs without duplicate defaults", () => {
  const config = { services: {} };
  for (const api of Object.values(Api)) {
    config.services[`${api}-postgres`] = {
      ports: [
        {
          target: 5432,
          published: "15432",
          host_ip: "127.0.0.1",
          protocol: "tcp",
        },
      ],
      environment: {
        POSTGRES_USER: "user@host",
        POSTGRES_PASSWORD: "p@ss:/?#",
        POSTGRES_DB: "db/name",
      },
    };
  }
  for (const value of Object.values(composeDatabaseUrls(config))) {
    const url = new URL(value);
    assert.equal(url.port, "15432");
    assert.equal(decodeURIComponent(url.username), "user@host");
    assert.equal(decodeURIComponent(url.password), "p@ss:/?#");
    assert.equal(decodeURIComponent(url.pathname), "/db/name");
  }
  config.services["delcore-postgres"].ports[0].host_ip = "0.0.0.0";
  assert.throws(() => composeDatabaseUrls(config), /Invalid local Compose/);
});

test("Git Tracker overrides injected DATABASE_URL while preserving provider settings", () => {
  const prepared = prepareApiEnvironment(Api.GitTracker, {
    ...handoff,
    DATABASE_URL: "postgres://remote/shared",
    CLIENT_SECRET: "fixture-provider",
  });
  assert.equal(prepared.env.DATABASE_URL, urls[Api.GitTracker]);
  assert.equal(prepared.env.CLIENT_SECRET, "fixture-provider");
  assert.equal(prepared.env[databaseHandoff], undefined);
});

test("Delcore preserves its source and all non-database settings in a private temporary copy", (t) => {
  const directory = fixture(t);
  const path = join(directory, "operator.json");
  const original = JSON.stringify(
    {
      database_url: "postgres://remote/shared",
      issuer: "https://fixture.invalid",
      identity_key: "fixture-key",
    },
    null,
    2,
  );
  writeFileSync(path, original);
  const prepared = prepareApiEnvironment(Api.Delcore, {
    ...handoff,
    CONFIG: path,
  });
  t.after(prepared.cleanup);
  const config = JSON.parse(readFileSync(prepared.env.CONFIG));
  assert.equal(config.database_url, urls[Api.Delcore]);
  assert.equal(config.identity_key, "fixture-key");
  assert.equal(config.issuer, "https://fixture.invalid");
  assert.equal(readFileSync(path, "utf8"), original);
  assert.notEqual(prepared.env.CONFIG, path);
  if (process.platform !== "win32") {
    assert.equal(statSync(prepared.env.CONFIG).mode & 0o777, 0o600);
    assert.equal(statSync(dirname(prepared.env.CONFIG)).mode & 0o777, 0o700);
  }
  prepared.cleanup();
  assert.equal(existsSync(prepared.env.CONFIG), false);
});

test("Delcore supports a service-local CONFIG fallback; injected CONFIG takes priority", (t) => {
  const directory = fixture(t);
  const path = join(directory, "local.json");
  writeFileSync(
    path,
    JSON.stringify({ database_url: "postgres://remote/shared" }),
  );
  writeFileSync(join(directory, ".env"), `CONFIG="${path}"\n`);
  const prepared = prepareApiEnvironment(Api.Delcore, handoff, directory);
  prepared.cleanup();
  assert.throws(
    () =>
      prepareApiEnvironment(
        Api.Delcore,
        { ...handoff, CONFIG: join(directory, "missing.json") },
        directory,
      ),
    /ENOENT/,
  );
});

test("direct API development preserves existing environment without a Compose handoff", () => {
  for (const api of Object.values(Api)) {
    const source = {
      CONFIG: "/operator/config.json",
      DATABASE_URL: "postgres://remote/shared",
    };
    assert.deepEqual(prepareApiEnvironment(api, source).env, source);
  }
  assert.throws(
    () => prepareApiEnvironment(Api.GitTracker, { [databaseHandoff]: "{}" }),
    /Missing local/,
  );
});

test(
  "root development stops before Turbo when Compose startup fails",
  { skip: process.platform === "win32" },
  (t) => {
    const directory = fixture(t);
    const config = { services: {} };
    for (const api of Object.values(Api)) {
      config.services[`${api}-postgres`] = {
        ports: [
          {
            target: 5432,
            published: "17432",
            host_ip: "127.0.0.1",
            protocol: "tcp",
          },
        ],
        environment: {
          POSTGRES_USER: "fixture",
          POSTGRES_PASSWORD: "fixture-password",
          POSTGRES_DB: "fixture",
        },
      };
    }
    writeFileSync(
      join(directory, "docker"),
      `#!${process.execPath}\nif (process.argv.includes('config')) process.stdout.write(${JSON.stringify(JSON.stringify(config))}); else process.exit(42);\n`,
      { mode: 0o755 },
    );
    const result = spawnSync(
      process.execPath,
      [fileURLToPath(new URL("./dev.mjs", import.meta.url)), "--dry=json"],
      {
        env: {
          ...process.env,
          PATH: `${directory}${delimiter}${process.env.PATH}`,
        },
        encoding: "utf8",
      },
    );
    assert.equal(result.status, 42);
    assert.equal(result.stdout.includes('"tasks"'), false);
    assert.equal(result.stdout.includes("fixture-password"), false);
  },
);

test(
  "API launcher removes the private config after Air failure or handled termination",
  { skip: process.platform === "win32", timeout: 15000 },
  async (t) => {
    const directory = fixture(t);
    const path = join(directory, "source.json");
    const output = join(directory, "config-path");
    writeFileSync(
      path,
      JSON.stringify({ database_url: "postgres://remote/shared" }),
    );
    const go = join(directory, "go");
    writeFileSync(
      go,
      `#!${process.execPath}\nimport('node:fs').then(({writeFileSync}) => { writeFileSync(process.env.FIXTURE_OUTPUT, process.env.CONFIG); if (process.env.FIXTURE_WAIT) { process.on('SIGTERM', () => process.exit(0)); setInterval(() => {}, 100); } else process.exit(9); });\n`,
      { mode: 0o755 },
    );
    for (const wait of [false, true]) {
      rmSync(output, { force: true });
      const child = spawn(
        process.execPath,
        [fileURLToPath(new URL("./dev-api.mjs", import.meta.url)), Api.Delcore],
        {
          env: {
            ...process.env,
            ...handoff,
            CONFIG: path,
            PATH: `${directory}${delimiter}${process.env.PATH}`,
            FIXTURE_OUTPUT: output,
            FIXTURE_WAIT: wait ? "1" : "",
          },
          stdio: "ignore",
        },
      );
      t.after(() => {
        if (child.exitCode === null && child.signalCode === null)
          child.kill("SIGTERM");
      });
      const completion = once(child, "exit");
      if (wait) {
        while (!existsSync(output))
          await new Promise((done) => setTimeout(done, 20));
        child.kill("SIGTERM");
      }
      const [code, signal] = await completion;
      if (wait) assert.equal(signal, "SIGTERM");
      else assert.equal(code, 9);
      const temporary = readFileSync(output, "utf8");
      assert.equal(existsSync(temporary), false);
      assert.equal(existsSync(path), true);
    }
  },
);
