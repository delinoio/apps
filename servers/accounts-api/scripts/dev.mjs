// SPDX-License-Identifier: FSL-1.1-ALv2

// Development launcher for accounts-api (`pnpm --filter accounts-api dev`).
//
// It replaces `go run ./cmd/accounts-api`. `go run` forwards only SIGINT and
// SIGQUIT to the compiled server and does not tie the server's lifetime to its
// own, so a SIGTERM sent to the launcher process (`kill`, `timeout`, a
// supervisor, pnpm or Turbo) ended `go run` and left the server listening
// without a graceful shutdown. This launcher builds the server into a
// temporary directory, runs the binary as its direct child, forwards SIGINT
// and SIGTERM to the active child, waits for it to exit and removes the build
// output afterwards. SIGHUP is not handled.
//
// Child processes use explicit argument arrays and `shell: false` according to
// the repository environment contract. Configuration (`ADDR`, `LOG_LEVEL`),
// standard streams and the working directory reach the server unchanged.
import { spawn } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { constants, tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Signals that stop the launcher. Each one is forwarded to the active child.
const ForwardedSignal = Object.freeze({
  Interrupt: "SIGINT",
  Terminate: "SIGTERM",
});
const signalExitCode = (signal) => 128 + constants.signals[signal];

const packageDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const binaryName =
  process.platform === "win32" ? "accounts-api.exe" : "accounts-api";

let received = null;
let activeChild = null;
const handlers = Object.values(ForwardedSignal).map((signal) => {
  const handler = () => {
    received ??= signal;
    console.error(JSON.stringify({ event: "accounts_api_dev_signal", signal }));
    // Between phases there is no child; the phase boundary checks `received`.
    activeChild?.kill(signal);
  };
  process.on(signal, handler);
  return [signal, handler];
});

// Runs one child with inherited stdio and resolves with its exit status once
// it has exited: its exit code, or the shell convention for a fatal signal.
function run(command, args) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(command, args, {
      cwd: packageDir,
      stdio: "inherit",
      shell: false,
    });
    activeChild = child;
    child.once("error", (error) => {
      activeChild = null;
      reject(error);
    });
    child.once("close", (code, signal) => {
      activeChild = null;
      resolvePromise(code ?? (signal ? signalExitCode(signal) : 1));
    });
  });
}

// A stopped launcher reports the shell convention for the received signal.
const stoppedStatus = () => signalExitCode(received);

async function main() {
  const buildDir = await mkdtemp(join(tmpdir(), "accounts-api-dev-"));
  try {
    const binary = join(buildDir, binaryName);
    // -buildvcs=false matches the `build` script: in a linked Git worktree Go's
    // VCS discovery can fail with "error obtaining VCS status". Remove it
    // together with the `build` script flag as documented in
    // docs/servers-accounts-api-foundation.md.
    const buildStatus = await run("go", [
      "build",
      "-buildvcs=false",
      "-o",
      binary,
      "./cmd/accounts-api",
    ]);
    // A signal during the build stops the launcher even when the build still
    // finished: the server must never start after a termination request.
    if (received) return stoppedStatus();
    if (buildStatus !== 0) return buildStatus;

    const serverStatus = await run(binary, []);
    return received ? stoppedStatus() : serverStatus;
  } finally {
    // Reached only after the last child has exited, so nothing uses the binary.
    await rm(buildDir, { recursive: true, force: true });
  }
}

try {
  process.exitCode = await main();
} catch (error) {
  console.error(
    JSON.stringify({ event: "accounts_api_dev_failed", error: String(error) }),
  );
  process.exitCode = 1;
} finally {
  for (const [signal, handler] of handlers) process.off(signal, handler);
}
