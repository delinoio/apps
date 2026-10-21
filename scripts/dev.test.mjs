// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import {
  existsSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { Api, composeDatabaseUrls, databaseHandoff } from "./dev.mjs";
import { prepareApiEnvironment } from "../servers/scripts/dev-api.mjs";

const urls = {
  [Api.Delcore]:
    "postgres://delcore:fixture@127.0.0.1:15432/delcore?sslmode=disable",
  [Api.DevBase]:
    "postgres://devbase:fixture@127.0.0.1:15434/devbase?sslmode=disable",
  [Api.GitTracker]:
    "postgres://git_tracker:fixture@127.0.0.1:15433/git_tracker?sslmode=disable",
};
const listeners = {
  [Api.Delcore]: "127.0.0.1:8080",
  [Api.DevBase]: "127.0.0.1:46317",
  [Api.GitTracker]: "127.0.0.1:46321",
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

test("DevBase overrides injected DATABASE_URL while preserving its provider settings", () => {
  const prepared = prepareApiEnvironment(Api.DevBase, {
    ...handoff,
    DATABASE_URL: "postgres://remote/shared",
    CLIENT_SECRET: "fixture-provider",
    DELCORE_URL: "https://delcore.example.test",
  });
  assert.equal(prepared.env.DATABASE_URL, urls[Api.DevBase]);
  assert.equal(prepared.env.CLIENT_SECRET, "fixture-provider");
  assert.equal(prepared.env.DELCORE_URL, "https://delcore.example.test");
  assert.equal(prepared.env.LISTEN, "127.0.0.1:46317");
  assert.equal(prepared.env[databaseHandoff], undefined);
});

test("Delcore leaves dotenv parsing to Go while preserving injected and Compose values", (t) => {
  const directory = fixture(t);
  const path = join(directory, ".env");
  const original =
    "SUFFIX=fixture-local\nIDENTITY_KEY=prefix$SUFFIX\nISSUER=https://fixture.invalid\n";
  writeFileSync(path, original);
  const prepared = prepareApiEnvironment(Api.Delcore, {
    ...handoff,
    DATABASE_URL: "postgres://remote/shared",
    IDENTITY_KEY: "fixture-injected",
  });
  assert.equal(prepared.env.DATABASE_URL, urls[Api.Delcore]);
  assert.equal(prepared.env.IDENTITY_KEY, "fixture-injected");
  assert.equal(prepared.env.ISSUER, undefined);
  assert.equal(prepared.env[databaseHandoff], undefined);
  assert.equal(readFileSync(path, "utf8"), original);
  assert.equal(
    prepareApiEnvironment(Api.Delcore, {}).env.IDENTITY_KEY,
    undefined,
  );
  assert.equal(
    prepareApiEnvironment(Api.Delcore, { IDENTITY_KEY: "" }).env.IDENTITY_KEY,
    "",
  );
});

test("API development listeners stay distinct despite shared and local LISTEN values", (t) => {
  const directory = fixture(t);
  writeFileSync(join(directory, ".env"), "LISTEN=0.0.0.0:9999\n");
  for (const compose of [false, true]) {
    for (const injected of ["127.0.0.1:8080", "127.0.0.1:46321", ""]) {
      const source = { ...(compose ? handoff : {}), LISTEN: injected };
      assert.equal(
        prepareApiEnvironment(Api.Delcore, source).env.LISTEN,
        "127.0.0.1:8080",
      );
      assert.equal(
        prepareApiEnvironment(Api.GitTracker, source).env.LISTEN,
        "127.0.0.1:46321",
      );
      assert.equal(
        prepareApiEnvironment(Api.DevBase, source).env.LISTEN,
        "127.0.0.1:46317",
      );
    }
  }
});

test("direct API development preserves existing environment without a Compose handoff", () => {
  for (const api of Object.values(Api)) {
    const source = {
      ISSUER: "https://fixture.invalid",
      DATABASE_URL: "postgres://remote/shared",
    };
    assert.deepEqual(prepareApiEnvironment(api, source).env, {
      ...source,
      LISTEN: listeners[api],
    });
  }
  assert.throws(
    () => prepareApiEnvironment(Api.GitTracker, { [databaseHandoff]: "{}" }),
    /Missing local/,
  );
});

test(
  "root development stops before process-compose when Compose startup fails",
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
      [fileURLToPath(new URL("./dev.mjs", import.meta.url)), "--tui=false"],
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
  "API launcher passes Compose environment and forwards Air failure or termination",
  { skip: process.platform === "win32", timeout: 15000 },
  async (t) => {
    const directory = fixture(t);
    const path = join(directory, ".env");
    const output = join(directory, "database-url");
    writeFileSync(path, "DATABASE_URL=postgres://local/shared\n");
    const go = join(directory, "go");
    writeFileSync(
      go,
      `#!${process.execPath}\nimport('node:fs').then(({writeFileSync}) => { writeFileSync(process.env.FIXTURE_OUTPUT, process.env.DATABASE_URL); if (process.env.FIXTURE_WAIT) { process.on('SIGTERM', () => process.exit(0)); setInterval(() => {}, 100); } else process.exit(9); });\n`,
      { mode: 0o755 },
    );
    for (const wait of [false, true]) {
      rmSync(output, { force: true });
      const child = spawn(
        process.execPath,
        [
          fileURLToPath(
            new URL("../servers/scripts/dev-api.mjs", import.meta.url),
          ),
          Api.Delcore,
        ],
        {
          cwd: directory,
          env: {
            ...process.env,
            ...handoff,
            DATABASE_URL: "postgres://remote/shared",
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
      assert.equal(readFileSync(output, "utf8"), urls[Api.Delcore]);
      assert.equal(existsSync(path), true);
    }
  },
);

test(
  "root development hands databases and arguments to process-compose and returns its failure",
  { skip: process.platform === "win32" },
  (t) => {
    const directory = fixture(t);
    const output = join(directory, "process-compose.json");
    const config = {
      services: Object.fromEntries(
        Object.values(Api).map((api) => [
          `${api}-postgres`,
          {
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
          },
        ]),
      ),
    };
    writeFileSync(
      join(directory, "docker"),
      `#!${process.execPath}\nif (process.argv.includes('config')) process.stdout.write(${JSON.stringify(JSON.stringify(config))});\n`,
      { mode: 0o755 },
    );
    writeFileSync(
      join(directory, "process-compose"),
      `#!${process.execPath}\nconst fs = require('node:fs'); fs.writeFileSync(process.env.FIXTURE_OUTPUT, JSON.stringify({args: process.argv.slice(2), cwd: process.cwd(), urls: JSON.parse(process.env.DEV_COMPOSE_DATABASE_URLS)})); process.exit(19);\n`,
      { mode: 0o755 },
    );
    const result = spawnSync(
      process.execPath,
      [
        fileURLToPath(new URL("./dev.mjs", import.meta.url)),
        "--tui=false",
        "delcore-api",
      ],
      {
        env: {
          ...process.env,
          PATH: `${directory}${delimiter}${process.env.PATH}`,
          FIXTURE_OUTPUT: output,
        },
        encoding: "utf8",
      },
    );
    assert.equal(result.status, 19);
    const captured = JSON.parse(readFileSync(output, "utf8"));
    assert.deepEqual(captured.args, [
      "--config",
      "process-compose.yaml",
      "--no-server",
      "--disable-dotenv",
      "up",
      "--tui=false",
      "delcore-api",
    ]);
    assert.equal(
      captured.cwd,
      fileURLToPath(new URL("../", import.meta.url)).replace(/\/$/u, ""),
    );
    assert.deepEqual(captured.urls, composeDatabaseUrls(config));
    assert.equal(result.stdout.includes("fixture-password"), false);
  },
);

test(
  "server builder skips empty workspaces and propagates the single workspace build result",
  { skip: process.platform === "win32" },
  (t) => {
    const directory = fixture(t);
    const output = join(directory, "cargo-calls.jsonl");
    writeFileSync(
      join(directory, "cargo"),
      `#!${process.execPath}
const fs = require('node:fs');
fs.appendFileSync(process.env.FIXTURE_OUTPUT, JSON.stringify({args: process.argv.slice(2), cwd: process.cwd()}) + '\\n');
if (process.argv[2] === 'metadata') {
  process.stdout.write(JSON.stringify({workspace_members: process.env.FIXTURE_EMPTY ? [] : ['fixture']}));
} else {
  process.exit(Number(process.env.FIXTURE_BUILD_EXIT));
}
`,
      { mode: 0o755 },
    );
    for (const [empty, buildExit] of [
      [true, 0],
      [false, 0],
      [false, 17],
    ]) {
      rmSync(output, { force: true });
      const result = spawnSync(
        process.execPath,
        [
          fileURLToPath(
            new URL("../servers/scripts/build-rust.mjs", import.meta.url),
          ),
        ],
        {
          env: {
            ...process.env,
            PATH: `${directory}${delimiter}${process.env.PATH}`,
            FIXTURE_OUTPUT: output,
            FIXTURE_EMPTY: empty ? "1" : "",
            FIXTURE_BUILD_EXIT: String(buildExit),
          },
          encoding: "utf8",
        },
      );
      assert.equal(result.status, buildExit);
      const calls = readFileSync(output, "utf8")
        .trim()
        .split("\n")
        .map((line) => JSON.parse(line));
      assert.deepEqual(
        calls.map((call) => call.args),
        [
          ["metadata", "--no-deps", "--format-version", "1"],
          ...(empty ? [] : [["build", "--workspace", "--bins"]]),
        ],
      );
      for (const call of calls) {
        assert.equal(
          call.cwd,
          fileURLToPath(new URL("../servers/", import.meta.url)).replace(
            /\/$/u,
            "",
          ),
        );
      }
    }
  },
);
