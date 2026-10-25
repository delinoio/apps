#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { verifiedExecutable, platformKey } from "./verify.mjs";
try {
  const root = fileURLToPath(new URL("../", import.meta.url));
  const child = spawn(verifiedExecutable(root), process.argv.slice(2), {
    stdio: "inherit",
    windowsHide: true,
  });
  const signals = ["SIGINT", "SIGTERM", "SIGHUP"];
  const handlers = new Map(
    signals.map((signal) => [
      signal,
      () => {
        if (!child.killed) child.kill(signal);
      },
    ]),
  );
  for (const [signal, handler] of handlers) process.on(signal, handler);
  child.on("error", () => {
    console.error(
      `lane: cannot start verified executable for ${platformKey()}; reinstall the matching package`,
    );
    process.exitCode = 1;
  });
  child.on("exit", (code, signal) => {
    for (const [name, handler] of handlers)
      process.removeListener(name, handler);
    if (signal) process.kill(process.pid, signal);
    else process.exitCode = code ?? 1;
  });
} catch (error) {
  console.error(
    `lane: installation verification failed (${process.platform}/${process.arch}): ${error.message}. Reinstall the matching @delinoio/lane version.`,
  );
  process.exitCode = 1;
}
