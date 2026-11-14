// SPDX-License-Identifier: FSL-1.1-ALv2
import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdtempSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

// These tests start the real launcher, which builds and runs the real server.
// They use POSIX signals and process groups, so Windows is skipped like the
// formatter signal tests.
const skip = process.platform === "win32";
const launcher = fileURLToPath(new URL("./dev.mjs", import.meta.url));
const StatusSigint = 130;
const StatusSigterm = 143;
const Timeout = 60_000;

// Reserves a free loopback port number. The port is released before returning,
// so a launcher started right after can bind it.
async function freePort() {
  const probe = createServer();
  await new Promise((resolve) => probe.listen(0, "127.0.0.1", resolve));
  const { port } = probe.address();
  await new Promise((resolve) => probe.close(resolve));
  return port;
}

// Accepts connections on port until closed; used to occupy a port.
async function occupy(port) {
  const holder = createServer();
  await new Promise((resolve, reject) => {
    holder.once("error", reject);
    holder.listen(port, "127.0.0.1", resolve);
  });
  return holder;
}

// Resolves true when binding the port fails, which means a listener holds it.
function portInUse(port) {
  return new Promise((resolve) => {
    const probe = createServer();
    probe.once("error", () => resolve(true));
    probe.listen(port, "127.0.0.1", () => probe.close(() => resolve(false)));
  });
}

// Starts the launcher in its own process group so a test can signal the group
// and so cleanup also reaches a server that a faulty launcher orphaned. The
// temporary build output goes to a test-owned TMPDIR.
function startLauncher(t, { port, env = {}, pathPrefix } = {}) {
  const tmp = mkdtempSync(join(tmpdir(), "accounts-api-dev-test-"));
  const childEnv = { ...process.env, ADDR: `127.0.0.1:${port}`, TMPDIR: tmp };
  delete childEnv.LOG_LEVEL;
  Object.assign(childEnv, env);
  if (pathPrefix) {
    childEnv.PATH = `${pathPrefix}${delimiter}${process.env.PATH}`;
  }
  const child = spawn(process.execPath, [launcher], {
    env: childEnv,
    stdio: ["ignore", "pipe", "pipe"],
    detached: true,
  });
  const run = { child, tmp, stdout: "", stderr: "" };
  child.stdout.on("data", (chunk) => (run.stdout += chunk));
  child.stderr.on("data", (chunk) => (run.stderr += chunk));
  run.exited = new Promise((resolve) =>
    child.once("close", (code, signal) => resolve({ code, signal })),
  );
  t.after(() => {
    try {
      process.kill(-child.pid, "SIGKILL");
    } catch {
      // The group is already gone.
    }
    rmSync(tmp, { recursive: true, force: true });
  });
  return run;
}

async function waitFor(run, predicate, what) {
  const deadline = Date.now() + Timeout;
  while (Date.now() < deadline) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  assert.fail(
    `timed out waiting for ${what}\nstdout: ${run.stdout}\nstderr: ${run.stderr}`,
  );
}

const waitReady = (run) =>
  waitFor(run, () => run.stdout.includes('"server_started"'), "server_started");

// The server must be gone and the port reusable once the launcher has exited.
async function assertNoListener(port) {
  assert.equal(await portInUse(port), false, "a listener survived");
}

function fakeGo(t, script) {
  const dir = mkdtempSync(join(tmpdir(), "accounts-api-fake-go-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const file = join(dir, "go");
  writeFileSync(file, `#!/bin/sh\n${script}\n`);
  chmodSync(file, 0o755);
  return dir;
}

test(
  "SIGTERM to the launcher shuts the server down gracefully",
  { skip, timeout: Timeout * 2 },
  async (t) => {
    const port = await freePort();
    const run = startLauncher(t, { port });
    await waitReady(run);
    const health = await fetch(`http://127.0.0.1:${port}/healthz`);
    assert.equal(health.status, 200);
    assert.deepEqual(await health.json(), { status: "ok" });

    // Only the launcher receives the signal, as `kill <pid>` would deliver it.
    process.kill(run.child.pid, "SIGTERM");
    const { code } = await run.exited;

    assert.equal(code, StatusSigterm, run.stderr);
    assert.match(run.stdout, /"server_shutdown_started"/);
    assert.match(run.stdout, /"server_shutdown_completed"/);
    await assertNoListener(port);
    assert.deepEqual(readdirSync(run.tmp), [], "build output was not removed");
  },
);

test(
  "a process-group SIGINT stops the launcher and the server",
  { skip, timeout: Timeout * 2 },
  async (t) => {
    const port = await freePort();
    const run = startLauncher(t, { port });
    await waitReady(run);

    // A terminal Ctrl-C reaches every process of the foreground group.
    process.kill(-run.child.pid, "SIGINT");
    const { code } = await run.exited;

    assert.equal(code, StatusSigint, run.stderr);
    assert.match(run.stdout, /"server_shutdown_completed"/);
    await assertNoListener(port);
    assert.deepEqual(readdirSync(run.tmp), [], "build output was not removed");
  },
);

test(
  "an occupied port fails without a server",
  { skip, timeout: Timeout * 2 },
  async (t) => {
    const port = await freePort();
    const holder = await occupy(port);
    t.after(() => holder.close());
    const run = startLauncher(t, { port });
    const { code } = await run.exited;

    assert.equal(code, 1, run.stderr);
    assert.match(run.stdout, /"server_exited"/);
    assert.doesNotMatch(run.stdout, /"server_started"/);
    assert.deepEqual(readdirSync(run.tmp), [], "build output was not removed");
  },
);

test(
  "an invalid LOG_LEVEL fails without a server",
  { skip, timeout: Timeout * 2 },
  async (t) => {
    const port = await freePort();
    const run = startLauncher(t, { port, env: { LOG_LEVEL: "bogus" } });
    const { code } = await run.exited;

    assert.equal(code, 1, run.stderr);
    assert.match(run.stdout, /"server_exited"/);
    assert.doesNotMatch(run.stdout, /"server_started"/);
    await assertNoListener(port);
    assert.deepEqual(readdirSync(run.tmp), [], "build output was not removed");
  },
);

test(
  "a build failure exits with the build status and starts no server",
  { skip, timeout: Timeout },
  async (t) => {
    const port = await freePort();
    const run = startLauncher(t, {
      port,
      pathPrefix: fakeGo(t, "exit 3"),
    });
    const { code } = await run.exited;

    assert.equal(code, 3, run.stderr);
    assert.doesNotMatch(run.stdout, /"server_started"/);
    await assertNoListener(port);
    assert.deepEqual(readdirSync(run.tmp), [], "build output was not removed");
  },
);

test(
  "SIGTERM during the build never starts the server",
  { skip, timeout: Timeout * 2 },
  async (t) => {
    const port = await freePort();
    const markerDir = mkdtempSync(join(tmpdir(), "accounts-api-marker-"));
    t.after(() => rmSync(markerDir, { recursive: true, force: true }));
    const marker = join(markerDir, "go-started");
    // The fake go waits, then on SIGTERM completes a real build and exits 0.
    // A launcher that ignored the termination would start the server next.
    const realGo = join(
      execFileSync("go", ["env", "GOROOT"], { encoding: "utf8" }).trim(),
      "bin",
      "go",
    );
    const run = startLauncher(t, {
      port,
      env: { REAL_GO: realGo, MARKER: marker },
      pathPrefix: fakeGo(
        t,
        [
          'touch "$MARKER"',
          "sleep 60 &",
          "pid=$!",
          'trap \'kill $pid; exec "$REAL_GO" "$@"\' TERM',
          "wait $pid",
        ].join("\n"),
      ),
    });
    await waitFor(run, () => existsSync(marker), "the build to start");

    process.kill(run.child.pid, "SIGTERM");
    const { code } = await run.exited;

    assert.equal(code, StatusSigterm, run.stderr);
    assert.doesNotMatch(run.stdout, /"server_started"/);
    await assertNoListener(port);
    assert.deepEqual(readdirSync(run.tmp), [], "build output was not removed");
  },
);
