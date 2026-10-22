// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import test from "node:test";

const require = createRequire(import.meta.url);
const config = JSON.parse(
  readFileSync(new URL("../turbo.json", import.meta.url)),
);
const rootPackage = JSON.parse(
  readFileSync(new URL("../package.json", import.meta.url)),
);

async function watchFixture(t, filters = [], initialSource = "initial") {
  const directory = mkdtempSync(join(tmpdir(), "turbo-dev-test-"));
  const write = (path, content) => {
    const target = join(directory, path);
    mkdirSync(join(target, ".."), { recursive: true });
    writeFileSync(target, content);
  };
  write(
    "package.json",
    JSON.stringify({
      name: "fixture",
      private: true,
      packageManager: rootPackage.packageManager,
      scripts: { "server-build": "node fixture.mjs build" },
    }),
  );
  // The fixture declares `packageManager` because Turbo requires it to detect
  // the workspace. By default pnpm then resolves that package manager through
  // its metadata cache and records it in the lockfile. That fails offline on
  // fresh machines such as CI runners, and it rewrites the lockfile whenever
  // Turbo runs a task through pnpm, which restarts the watch. The fixture only
  // needs the running pnpm, so configure pnpm in the workspace file, not in the
  // environment, because Turbo does not pass arbitrary variables to tasks.
  // Remove `pmOnFail` if the fixture stops declaring `packageManager`.
  write(
    "pnpm-workspace.yaml",
    "packages:\n  - servers/*\n  - apps/*\npmOnFail: ignore\n",
  );
  write(
    "pnpm-lock.yaml",
    "lockfileVersion: '9.0'\nimporters:\n  .: {}\n  servers/delcore-api: {}\n  servers/devbase-api: {}\n  servers/git-tracker-api: {}\n  apps/frontend: {}\n",
  );
  write(".gitignore", ".turbo/\n.fixture/\nnode_modules/\nservers/target/\n");
  write("turbo.json", JSON.stringify(config));
  write("servers/delcore-api/src/main.rs", initialSource);
  write("servers/delcore-api/catalog/example.yaml", "initial");
  for (const api of ["delcore", "devbase", "git-tracker"]) {
    write(
      `servers/${api}-api/package.json`,
      JSON.stringify({
        name: `@delinoio/${api}-api`,
        scripts: { dev: `node ../../fixture.mjs ${api}` },
      }),
    );
  }
  write(
    "apps/frontend/package.json",
    JSON.stringify({
      name: "frontend",
      scripts: { dev: "node ../../fixture.mjs frontend" },
    }),
  );
  write(
    "fixture.mjs",
    `
import { appendFileSync, readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('.', import.meta.url));
const service = process.argv[2];
const event = (kind, extra = {}) => appendFileSync(root + '.fixture/events.jsonl', JSON.stringify({kind, service, pid: process.pid, ...extra}) + '\\n');
if (service === 'build') {
  const source = readFileSync(root + 'servers/delcore-api/src/main.rs', 'utf8');
  event('build', {source});
  await new Promise(resolve => setTimeout(resolve, 200));
  if (source === 'fail') { event('build-failed'); process.exit(17); }
  writeFileSync(root + '.fixture/revision', source);
  event('built', {source});
} else {
  const revision = service === 'frontend' ? null : readFileSync(root + '.fixture/revision', 'utf8');
  event('start', {revision, handoff: process.env.DEV_COMPOSE_DATABASE_URLS ?? null});
  for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => { event('stop', {signal}); process.exit(0); });
  process.on('SIGUSR2', () => { event('crash'); process.exit(23); });
  setInterval(() => {}, 1000);
}
`,
  );
  mkdirSync(join(directory, ".fixture"));
  const install = spawnSync(
    "pnpm",
    ["install", "--lockfile-only", "--ignore-scripts", "--offline"],
    { cwd: directory, encoding: "utf8" },
  );
  assert.equal(install.status, 0, install.stdout + install.stderr);
  const git = spawnSync("git", ["init", "--quiet"], { cwd: directory });
  assert.equal(git.status, 0);
  assert.equal(spawnSync("git", ["add", "."], { cwd: directory }).status, 0);
  assert.equal(
    spawnSync(
      "git",
      [
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.test",
        "-c",
        "core.hooksPath=/dev/null",
        "commit",
        "--quiet",
        "-m",
        "fixture",
      ],
      { cwd: directory },
    ).status,
    0,
  );
  const resolved = spawnSync(
    process.execPath,
    [require.resolve("turbo/bin/turbo"), "bin"],
    { encoding: "utf8" },
  );
  assert.equal(resolved.status, 0);
  const child = spawn(
    resolved.stdout.trim(),
    ["watch", "dev", "--concurrency=20", "--ui=stream", ...filters],
    {
      cwd: directory,
      env: {
        ...process.env,
        CI: "1",
        TURBO_TELEMETRY_DISABLED: "1",
        TURBO_TOKEN: "",
        TURBO_TEAM: "",
        DEV_COMPOSE_DATABASE_URLS: "fixture-database-handoff",
      },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  let output = "";
  child.stdout.on("data", (data) => {
    output += data;
  });
  child.stderr.on("data", (data) => {
    output += data;
  });
  const events = () => {
    try {
      return readFileSync(join(directory, ".fixture/events.jsonl"), "utf8")
        .trim()
        .split("\n")
        .filter(Boolean)
        .map((line) => JSON.parse(line));
    } catch (error) {
      if (error.code === "ENOENT") return [];
      throw error;
    }
  };
  t.after(async () => {
    if (child.exitCode === null && child.signalCode === null) {
      const closed = once(child, "close");
      child.kill("SIGINT");
      const timeout = setTimeout(() => child.kill("SIGKILL"), 5000);
      try {
        await closed;
      } finally {
        clearTimeout(timeout);
      }
    }
    for (const event of events().filter((event) => event.kind === "start")) {
      try {
        process.kill(event.pid, "SIGKILL");
      } catch (error) {
        if (error.code !== "ESRCH") throw error;
      }
    }
    rmSync(directory, { recursive: true, force: true });
  });
  const wait = async (predicate) => {
    const deadline = Date.now() + 15000;
    while (Date.now() < deadline) {
      if (predicate(events())) return events();
      if (child.exitCode !== null || child.signalCode !== null)
        assert.fail(`Turbo exited unexpectedly: ${output}`);
      await delay(50);
    }
    assert.fail(
      `Timed out waiting for Turbo events: ${JSON.stringify(events())}\n${output}`,
    );
  };
  return { child, events, wait, write, output: () => output };
}

test(
  "Turbo watches the shared build before restarting APIs and leaves frontend HMR running",
  { skip: process.platform === "win32", timeout: 60000 },
  async (t) => {
    const fixture = await watchFixture(t);
    let events = await fixture.wait(
      (events) => events.filter((event) => event.kind === "start").length === 4,
    );
    for (const event of events.filter((event) => event.kind === "start")) {
      assert.equal(
        event.handoff,
        event.service === "frontend" ? null : "fixture-database-handoff",
      );
    }
    const built = events.findIndex((event) => event.kind === "built");
    assert.ok(
      events.every(
        (event, index) =>
          event.kind !== "start" ||
          event.service === "frontend" ||
          index > built,
      ),
    );
    fixture.write("servers/delcore-api/src/main.rs", "second");
    events = await fixture.wait(
      (events) =>
        events.filter(
          (event) => event.kind === "start" && event.revision === "second",
        ).length === 3,
    );
    assert.equal(events.filter((event) => event.kind === "build").length, 2);
    assert.equal(
      events.filter(
        (event) => event.kind === "start" && event.service === "frontend",
      ).length,
      1,
    );
    fixture.write("servers/delcore-api/catalog/example.yaml", "changed");
    fixture.write("servers/target/generated.rs", "ignored");
    await delay(800);
    assert.equal(
      fixture.events().filter((event) => event.kind === "build").length,
      2,
    );
    assert.equal(
      fixture.events().filter((event) => event.kind === "start").length,
      7,
    );
    fixture.write("servers/delcore-api/src/main.rs", "fail");
    await fixture.wait((events) =>
      events.some((event) => event.kind === "build-failed"),
    );
    await delay(300);
    fixture.write("servers/delcore-api/src/main.rs", "recovered");
    events = await fixture.wait(
      (events) =>
        events.filter(
          (event) => event.kind === "start" && event.revision === "recovered",
        ).length === 3,
    );
    assert.equal(
      events.filter(
        (event) => event.kind === "start" && event.service === "frontend",
      ).length,
      1,
    );
    const closed = once(fixture.child, "close");
    fixture.child.kill("SIGINT");
    await closed;
    for (const event of events.filter((event) => event.kind === "start"))
      assert.throws(() => process.kill(event.pid, 0), { code: "ESRCH" });
  },
);

test(
  "Turbo includes the shared build when selecting one API",
  { skip: process.platform === "win32", timeout: 30000 },
  async (t) => {
    const fixture = await watchFixture(t, ["--filter=@delinoio/delcore-api"]);
    const events = await fixture.wait((events) =>
      events.some((event) => event.kind === "start"),
    );
    assert.deepEqual(
      events.map((event) => [event.kind, event.service]),
      [
        ["build", "build"],
        ["built", "build"],
        ["start", "delcore"],
      ],
    );
  },
);

test(
  "Turbo cancels sibling APIs on a runtime failure and can recover on the next source edit",
  { skip: process.platform === "win32", timeout: 30000 },
  async (t) => {
    const fixture = await watchFixture(t);
    let events = await fixture.wait(
      (events) => events.filter((event) => event.kind === "start").length === 4,
    );
    const crashed = events.find(
      (event) => event.kind === "start" && event.service === "delcore",
    );
    process.kill(crashed.pid, "SIGUSR2");
    await fixture.wait((events) =>
      events.some((event) => event.kind === "crash"),
    );
    await fixture.wait((events) =>
      ["devbase", "git-tracker"].every((service) =>
        events.some(
          (event) => event.kind === "stop" && event.service === service,
        ),
      ),
    );
    assert.equal(fixture.child.exitCode, null);
    fixture.write("servers/delcore-api/src/main.rs", "after-crash");
    events = await fixture.wait(
      (events) =>
        events.filter(
          (event) => event.kind === "start" && event.revision === "after-crash",
        ).length === 3,
    );
    assert.equal(
      events.filter(
        (event) => event.kind === "start" && event.service === "frontend",
      ).length,
      1,
    );
  },
);

test(
  "Turbo blocks API startup after an initial build failure and retries on an edit",
  { skip: process.platform === "win32", timeout: 30000 },
  async (t) => {
    const fixture = await watchFixture(t, [], "fail");
    await fixture.wait((events) =>
      events.some((event) => event.kind === "build-failed"),
    );
    await delay(300);
    assert.ok(
      fixture
        .events()
        .filter((event) => event.kind === "start")
        .every((event) => event.service === "frontend"),
    );
    fixture.write("servers/delcore-api/src/main.rs", "fixed");
    await fixture.wait(
      (events) =>
        events.filter(
          (event) => event.kind === "start" && event.revision === "fixed",
        ).length === 3,
    );
  },
);

test(
  "Turbo rebuilds a selected API for shared Rust, SQL, Cargo and protocol inputs",
  { skip: process.platform === "win32", timeout: 30000 },
  async (t) => {
    const fixture = await watchFixture(t, ["--filter=@delinoio/delcore-api"]);
    await fixture.wait((events) =>
      events.some((event) => event.kind === "start"),
    );
    let builds = 1;
    for (const path of [
      "servers/crates/server-common/src/lib.rs",
      "servers/delcore-api/migrations/fixture.sql",
      "servers/Cargo.toml",
      "servers/Cargo.lock",
      "protos/delcore/v1/fixture.proto",
    ]) {
      fixture.write(path, "changed");
      builds += 1;
      await fixture.wait(
        (events) =>
          events.filter((event) => event.kind === "start").length === builds,
      );
      assert.equal(
        fixture.events().filter((event) => event.kind === "build").length,
        builds,
      );
    }
  },
);
