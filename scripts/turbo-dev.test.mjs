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

async function runFixture(t, filters = [], initialSource = "initial") {
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
  // Turbo runs a task through pnpm. The fixture only needs the running pnpm,
  // so configure pnpm in the workspace file, not in the
  // environment, because Turbo does not pass arbitrary variables to tasks.
  // Remove `pmOnFail` if the fixture stops declaring `packageManager`.
  write(
    "pnpm-workspace.yaml",
    "packages:\n  - servers/*\n  - apps/*\npmOnFail: ignore\n",
  );
  write(
    "pnpm-lock.yaml",
    "lockfileVersion: '9.0'\nimporters:\n  .: {}\n  servers/delcore-api: {}\n  servers/delitools-api: {}\n  servers/devbase-api: {}\n  servers/git-tracker-api: {}\n  servers/perfine-api: {}\n  apps/frontend: {}\n",
  );
  write(".gitignore", ".turbo/\n.fixture/\nnode_modules/\nservers/target/\n");
  write("turbo.json", JSON.stringify(config));
  write("servers/delcore-api/src/main.rs", initialSource);
  write("servers/delcore-api/catalog/example.yaml", "initial");
  for (const api of [
    "delcore",
    "delitools",
    "devbase",
    "git-tracker",
    "perfine",
  ]) {
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
    ["run", "dev", "--concurrency=20", "--ui=stream", ...filters],
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
  const completion = once(child, "close");
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
  const waitForExit = async () => {
    const timeout = setTimeout(() => child.kill("SIGKILL"), 10000);
    try {
      const [code, signal] = await completion;
      assert.equal(signal, null, output);
      return code;
    } finally {
      clearTimeout(timeout);
    }
  };
  return { child, events, wait, write, waitForExit, output: () => output };
}

test(
  "Turbo builds once before API startup and does not restart tasks on source edits",
  { skip: process.platform === "win32", timeout: 60000 },
  async (t) => {
    const fixture = await runFixture(t);
    let events = await fixture.wait(
      (events) => events.filter((event) => event.kind === "start").length === 6,
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
    for (const path of [
      "servers/delcore-api/src/main.rs",
      "servers/crates/server-common/src/lib.rs",
      "servers/delcore-api/migrations/fixture.sql",
      "servers/Cargo.toml",
      "servers/Cargo.lock",
      "protos/delcore/v1/fixture.proto",
      "servers/delcore-api/catalog/example.yaml",
      "servers/target/generated.rs",
    ]) {
      fixture.write(path, "changed");
    }
    await delay(800);
    events = fixture.events();
    assert.equal(events.filter((event) => event.kind === "build").length, 1);
    assert.equal(events.filter((event) => event.kind === "start").length, 6);
    assert.equal(events.filter((event) => event.kind === "stop").length, 0);
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
    const fixture = await runFixture(t, ["--filter=@delinoio/delcore-api"]);
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
  "Turbo ends the run and stops sibling tasks on an API failure",
  { skip: process.platform === "win32", timeout: 30000 },
  async (t) => {
    const fixture = await runFixture(t);
    const events = await fixture.wait(
      (events) => events.filter((event) => event.kind === "start").length === 6,
    );
    const crashed = events.find(
      (event) => event.kind === "start" && event.service === "delcore",
    );
    process.kill(crashed.pid, "SIGUSR2");
    assert.notEqual(await fixture.waitForExit(), 0);
    const finalEvents = fixture.events();
    assert.ok(finalEvents.some((event) => event.kind === "crash"));
    for (const service of [
      "delitools",
      "devbase",
      "git-tracker",
      "perfine",
      "frontend",
    ]) {
      assert.ok(
        finalEvents.some(
          (event) => event.kind === "stop" && event.service === service,
        ),
      );
    }
    assert.equal(
      finalEvents.filter((event) => event.kind === "build").length,
      1,
    );
    assert.equal(
      finalEvents.filter((event) => event.kind === "start").length,
      6,
    );
  },
);

test(
  "Turbo blocks API startup and ends the run after an initial build failure",
  { skip: process.platform === "win32", timeout: 30000 },
  async (t) => {
    const fixture = await runFixture(t, [], "fail");
    assert.notEqual(await fixture.waitForExit(), 0);
    const events = fixture.events();
    assert.ok(events.some((event) => event.kind === "build-failed"));
    assert.equal(events.filter((event) => event.kind === "build").length, 1);
    assert.ok(
      events
        .filter((event) => event.kind === "start")
        .every((event) => event.service === "frontend"),
    );
  },
);
