#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Runs the verified AnyDash executable from the matching platform package and
// propagates arguments, stdio, signals and the exit status.
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { platformKey, verifiedExecutable } from "./verify.mjs";

const platform = `${process.platform}/${process.arch}`;
try {
  const root = fileURLToPath(new URL("../", import.meta.url));
  const child = spawn(verifiedExecutable(root), process.argv.slice(2), {
    stdio: "inherit",
    windowsHide: true,
  });
  const handlers = new Map(
    ["SIGINT", "SIGTERM", "SIGHUP"].map((signal) => [
      signal,
      () => {
        if (!child.killed) child.kill(signal);
      },
    ]),
  );
  for (const [signal, handler] of handlers) process.on(signal, handler);
  child.on("error", () => {
    console.error(
      `anydash: cannot start the verified executable for ${platformKey()}; reinstall the matching @delinoio/anydash version`,
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
    `anydash: installation verification failed (${platform}): ${error.message}. Reinstall the matching @delinoio/anydash version with optional dependencies enabled.`,
  );
  process.exitCode = 1;
}
