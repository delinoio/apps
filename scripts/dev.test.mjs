// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { Api, composeDatabaseUrls, databaseHandoff } from "./dev.mjs";
import {
  apiCommand,
  prepareApiEnvironment,
} from "../servers/scripts/dev-api.mjs";

const urls = {
  [Api.Lane]: "postgres://lane:fixture@127.0.0.1:15437/lane?sslmode=disable",
  [Api.Perfine]:
    "postgres://perfine:fixture@127.0.0.1:15436/perfine?sslmode=disable",
  [Api.Delcore]:
    "postgres://delcore:fixture@127.0.0.1:15432/delcore?sslmode=disable",
  [Api.DeliTools]:
    "postgres://delitools:fixture@127.0.0.1:15435/delitools?sslmode=disable",
  [Api.DevBase]:
    "postgres://devbase:fixture@127.0.0.1:15434/devbase?sslmode=disable",
  [Api.GitTracker]:
    "postgres://git_tracker:fixture@127.0.0.1:15433/git_tracker?sslmode=disable",
};
const listeners = {
  [Api.Lane]: "127.0.0.1:46326",
  [Api.Perfine]: "127.0.0.1:46323",
  [Api.Delcore]: "127.0.0.1:8080",
  [Api.DeliTools]: "127.0.0.1:46322",
  [Api.DevBase]: "127.0.0.1:46317",
  [Api.GitTracker]: "127.0.0.1:46321",
};
const handoff = { [databaseHandoff]: JSON.stringify(urls) };
const fixture = (t) => {
  const directory = mkdtempSync(join(tmpdir(), "compose-dev-test-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  return directory;
};

for (const [signal, ignoreSignal] of [
  ["SIGINT", false],
  ["SIGTERM", false],
  ["SIGINT", true],
]) {
  test(
    `root supervisor cleans up descendants after ${signal}${ignoreSignal ? " even when ignored" : ""}`,
    { skip: process.platform === "win32", timeout: 20000 },
    async (t) => {
      const directory = mkdtempSync(join(tmpdir(), "compose-dev-test-"));
      const output = join(directory, "ready.json");
      const stopped = join(directory, "stopped");
      const leaf = join(directory, "leaf.mjs");
      writeFileSync(
        leaf,
        `import { createServer } from 'node:net';
import { writeFileSync } from 'node:fs';
const server = createServer();
process.on('${signal}', () => {
  ${ignoreSignal ? "" : `writeFileSync(${JSON.stringify(stopped)}, 'stopped'); server.close();`}
});
server.listen(0, '127.0.0.1', () => {
  writeFileSync(${JSON.stringify(output)}, JSON.stringify({pid: process.pid, parent: process.ppid, port: server.address().port}));
});
`,
      );
      // Model a command wrapper that exits before its server descendant.
      writeFileSync(
        join(directory, "pnpm"),
        `#!${process.execPath}
const { spawn } = require('node:child_process');
spawn(process.execPath, [${JSON.stringify(leaf)}], {stdio: 'inherit'});
process.on('${signal}', () => process.exit(0));
`,
        { mode: 0o755 },
      );
      const devUrl = new URL("./dev.mjs", import.meta.url).href;
      const spawnUrl = new URL("./spawn-dev-server.mjs", import.meta.url).href;
      const child = spawn(
        process.execPath,
        [
          "--input-type=module",
          "-e",
          `import { supervise } from ${JSON.stringify(devUrl)}; import { exitLikeChild } from ${JSON.stringify(spawnUrl)}; exitLikeChild(await supervise([]));`,
        ],
        {
          env: {
            ...process.env,
            PATH: `${directory}${delimiter}${process.env.PATH}`,
          },
          stdio: "ignore",
        },
      );
      t.after(() => {
        child.kill("SIGKILL");
        if (existsSync(output)) {
          const { pid, parent } = JSON.parse(readFileSync(output, "utf8"));
          for (const target of [pid, parent]) {
            try {
              process.kill(target, "SIGKILL");
            } catch (error) {
              if (error.code !== "ESRCH") throw error;
            }
          }
        }
        rmSync(directory, { recursive: true, force: true });
      });
      const deadline = Date.now() + 5000;
      while (!existsSync(output)) {
        assert.ok(Date.now() < deadline, "descendant did not start");
        await delay(25);
      }
      const { pid, port } = JSON.parse(readFileSync(output, "utf8"));
      const exited = once(child, "exit");
      child.kill(signal);
      assert.deepEqual(await exited, [null, signal]);
      if (process.platform === "linux") {
        // Container PID 1 may leave an exited descendant as a zombie.
        try {
          const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
          assert.match(stat.slice(stat.lastIndexOf(")") + 1), /^\s+[ZX]\s/u);
        } catch (error) {
          if (error.code !== "ENOENT" && error.code !== "ESRCH") throw error;
        }
      } else {
        assert.throws(() => process.kill(pid, 0), { code: "ESRCH" });
      }
      if (!ignoreSignal) assert.equal(readFileSync(stopped, "utf8"), "stopped");
      const { createServer } = await import("node:net");
      const server = createServer();
      server.listen(port, "127.0.0.1");
      await once(server, "listening");
      await new Promise((resolve) => server.close(resolve));
    },
  );
}

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

test("DeliTools overrides injected DATABASE_URL while preserving its provider and Gateway settings", () => {
  const prepared = prepareApiEnvironment(Api.DeliTools, {
    ...handoff,
    DATABASE_URL: "postgres://remote/shared",
    CLIENT_SECRET: "fixture-provider",
    AI_GATEWAY_API_KEY: "fixture-gateway",
    DELITOOLS_MODELS: "fixture-models.yaml",
  });
  assert.equal(prepared.env.DATABASE_URL, urls[Api.DeliTools]);
  assert.equal(prepared.env.CLIENT_SECRET, "fixture-provider");
  assert.equal(prepared.env.AI_GATEWAY_API_KEY, "fixture-gateway");
  assert.equal(prepared.env.DELITOOLS_MODELS, "fixture-models.yaml");
  assert.equal(prepared.env.LISTEN, "127.0.0.1:46322");
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

test("Delcore leaves dotenv parsing to Rust while preserving injected and Compose values", (t) => {
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
      assert.equal(
        prepareApiEnvironment(Api.DeliTools, source).env.LISTEN,
        "127.0.0.1:46322",
      );
    }
  }
});

test("direct API development preserves existing environment without a Compose handoff", () => {
  for (const api of Object.keys(listeners)) {
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
      [fileURLToPath(new URL("./dev.mjs", import.meta.url)), "--ui=stream"],
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
  "API launcher passes Compose environment and forwards binary failure or termination",
  { skip: process.platform === "win32", timeout: 15000 },
  async (t) => {
    const directory = fixture(t);
    const path = join(directory, ".env");
    const output = join(directory, "database-url");
    writeFileSync(path, "DATABASE_URL=postgres://local/shared\n");
    mkdirSync(join(directory, "debug"));
    const binary = join(directory, "debug", "delcore-api");
    writeFileSync(
      binary,
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
            CARGO_TARGET_DIR: directory,
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
  "root development hands databases and arguments directly to Turbo and returns its failure",
  { skip: process.platform === "win32" },
  (t) => {
    const directory = fixture(t);
    const output = join(directory, "turbo.json");
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
      join(directory, "pnpm"),
      `#!${process.execPath}\nconst fs = require('node:fs'); fs.writeFileSync(process.env.FIXTURE_OUTPUT, JSON.stringify({args: process.argv.slice(2), cwd: process.cwd(), urls: JSON.parse(process.env.DEV_COMPOSE_DATABASE_URLS)})); process.exit(19);\n`,
      { mode: 0o755 },
    );
    writeFileSync(
      join(directory, "portless"),
      `#!${process.execPath}\nprocess.exit(98);\n`,
      { mode: 0o755 },
    );
    const result = spawnSync(
      process.execPath,
      [
        fileURLToPath(new URL("./dev.mjs", import.meta.url)),
        "--ui=stream",
        "--filter=@servers/delcore-api",
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
      "exec",
      "turbo",
      "run",
      "dev",
      "--concurrency=20",
      "--continue=dependencies-successful",
      "--ui=stream",
      "--filter=@servers/delcore-api",
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

test("Rust API launchers execute shared build outputs without another Cargo process", () => {
  const config = JSON.parse(
    readFileSync(new URL("../turbo.json", import.meta.url), "utf8"),
  );
  for (const service of ["delcore", "delitools", "devbase", "git-tracker"]) {
    assert.ok(
      config.tasks[`@servers/${service}-api#dev`].passThroughEnv.includes(
        "CARGO_TARGET_DIR",
      ),
    );
  }
  const target = join(tmpdir(), "server-target-fixture");
  for (const [api, binary] of [
    [Api.Delcore, "delcore-api"],
    [Api.DeliTools, "delitools-api"],
    [Api.DevBase, "devbase-api"],
    [Api.GitTracker, "git-tracker-api"],
    [Api.Perfine, "perfine-api"],
    [Api.Lane, "lane-api"],
  ]) {
    const [command, args] = apiCommand(api, { CARGO_TARGET_DIR: target });
    assert.equal(
      command,
      join(
        target,
        "debug",
        `${binary}${process.platform === "win32" ? ".exe" : ""}`,
      ),
    );
    assert.deepEqual(args, ["serve", "--dev-env"]);
  }
});

test("root, service-local and Portless Delcore development share the catalog-applying launcher", () => {
  const read = (path) =>
    readFileSync(fileURLToPath(new URL(path, import.meta.url)), "utf8");
  const root = JSON.parse(read("../package.json"));
  const service = JSON.parse(read("../servers/delcore-api/package.json"));
  const turbo = JSON.parse(read("../turbo.json"));
  // Root development delegates route registration to each workspace task.
  assert.equal(root.scripts.dev, "node scripts/dev.mjs");
  const routes = [
    ["apps/delidev", "delidev", 46311],
    ["apps/delitools", "delitools", 46312],
    ["apps/devbase", "devbase", 46313],
    ["apps/labs-docs", "labs-docs", 46330],
    ["apps/delino-site", "delino-site", 46314],
    ["apps/delino-docs", "delino-docs", 46315],
    ["apps/git-tracker", "git-tracker", 46320],
    ["servers/delcore-api", "delcore-api", 8080],
    ["servers/delitools-api", "delitools-api", 46322],
    ["servers/devbase-api", "devbase-api", 46317],
    ["servers/git-tracker-api", "git-tracker-api", 46321],
  ];
  for (const [path, name, port] of routes) {
    const script = JSON.parse(read(`../${path}/package.json`)).scripts.dev;
    assert.ok(
      script.startsWith("infisical run --env=dev --project-config-dir=../.. "),
    );
    assert.ok(
      script.includes(` -- portless run --name ${name} --app-port ${port} `),
    );
    assert.ok(!script.includes("--force"));
  }
  assert.equal(turbo.tasks.dev.persistent, true);
  assert.deepEqual(turbo.globalPassThroughEnv, ["PORTLESS", "PORTLESS_*"]);
  // Infisical injects values before Portless runs the shared API launcher.
  assert.match(
    service.scripts.dev,
    /^infisical run .*--path=\/delcore-api -- portless run --name delcore-api --app-port 8080 node \.\.\/scripts\/dev-api\.mjs delcore$/u,
  );
  const listen = prepareApiEnvironment(Api.Delcore, {}).env.LISTEN;
  assert.equal(listen, "127.0.0.1:8080");
  // Build inputs include code and SQL. Catalog edits apply at API startup.
  const builder = turbo.tasks["//#server-build"];
  assert.ok(builder.inputs.includes("servers/**/*.rs"));
  assert.ok(builder.inputs.includes("servers/**/*.sql"));
  assert.ok(!builder.inputs.some((input) => /ya?ml/u.test(input)));
  assert.deepEqual(apiCommand(Api.Delcore, {})[1], ["serve", "--dev-env"]);
});

test("Perfine Compose database and Turbo handoff remain service-owned", async () => {
  const yaml = (await import("js-yaml")).default;
  const compose = yaml.load(
    readFileSync(new URL("../compose.yaml", import.meta.url), "utf8"),
  );
  const service = compose.services["perfine-postgres"];
  assert.deepEqual(service.ports, [
    "127.0.0.1:${PERFINE_POSTGRES_PORT:-55436}:5432",
  ]);
  assert.ok(service.healthcheck.test.join(" ").includes("-h 127.0.0.1"));
  assert.deepEqual(service.volumes, [
    "perfine-postgres-data:/var/lib/postgresql/data",
  ]);
  assert.equal(Api.Perfine, "perfine");
  const config = JSON.parse(
    readFileSync(new URL("../turbo.json", import.meta.url), "utf8"),
  );
  const task = config.tasks["@servers/perfine-api#dev"];
  for (const key of [
    "SPACE_STORAGE_QUOTA_BYTES",
    "SINGLE_UPLOAD_LIMIT_BYTES",
    "REQUESTS_PER_MINUTE",
    "CONCURRENT_CLOUD_EXECUTIONS",
    "RUST_LOG",
  ])
    assert.ok(task.passThroughEnv.includes(key), key);
  for (const key of ["OPENAI_API_KEY", "ANTHROPIC_API_KEY", "GH_TOKEN"])
    assert.equal(task.passThroughEnv.includes(key), false, key);
  assert.deepEqual(task.dependsOn, ["//#server-build"]);
  assert.ok(task.passThroughEnv.includes(databaseHandoff));
  assert.equal(config.globalPassThroughEnv.includes(databaseHandoff), false);
  assert.ok(
    !config.tasks["//#server-build"].inputs.includes("crates/perfine-core/**"),
  );
  assert.ok(!config.tasks["//#server-build"].inputs.includes("Cargo.toml"));
});

test("Lane development keeps its database and provider configuration API-local", async () => {
  const yaml = (await import("js-yaml")).default;
  const compose = yaml.load(
    readFileSync(new URL("../compose.yaml", import.meta.url), "utf8"),
  );
  const service = compose.services["lane-postgres"];
  assert.deepEqual(service.ports, [
    "127.0.0.1:${LANE_POSTGRES_PORT:-55437}:5432",
  ]);
  assert.deepEqual(service.volumes, [
    "lane-postgres-data:/var/lib/postgresql/data",
  ]);
  assert.ok(service.healthcheck.test.join(" ").includes("-h 127.0.0.1"));
  const config = JSON.parse(
    readFileSync(new URL("../turbo.json", import.meta.url), "utf8"),
  );
  const task = config.tasks["@servers/lane-api#dev"];
  assert.deepEqual(task.dependsOn, ["//#server-build"]);
  for (const key of [
    databaseHandoff,
    "MAX_ITEM_BYTES",
    "MAX_WEBHOOK_BYTES",
    "REQUESTS_PER_MINUTE",
    "WEBHOOK_REQUESTS_PER_MINUTE",
    "LANE_*",
    "APNS_*",
    "FCM_*",
    "WEB_PUSH_*",
  ]) {
    assert.ok(task.passThroughEnv.includes(key), key);
    assert.ok(!config.globalPassThroughEnv.includes(key), key);
    assert.ok(!config.globalEnv.includes(key), key);
  }
  const prepared = prepareApiEnvironment(Api.Lane, {
    ...handoff,
    LISTEN: "0.0.0.0:9999",
    DATABASE_URL: "postgres://remote.invalid/lane",
  });
  assert.equal(prepared.env.LISTEN, "127.0.0.1:46326");
  assert.equal(prepared.env.DATABASE_URL, urls[Api.Lane]);
  assert.equal(prepared.env[databaseHandoff], undefined);
});
