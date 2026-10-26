// SPDX-License-Identifier: Apache-2.0
import { access, readFile, rename, rm, writeFile } from "node:fs/promises";
import { spawn } from "node:child_process";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { executablePath, writeResult } from "./update-files.mjs";

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
export async function launchApp(app, platform) {
  const child = spawn(executablePath(app, platform), [], {
    detached: true,
    windowsHide: true,
    stdio: "ignore",
    cwd: app,
    // A copied Node runtime runs this installer; never inherit a Node-only
    // startup mode into the desktop runtime.
    env: Object.fromEntries(
      Object.entries(process.env).filter(
        ([key]) =>
          !["NODE_OPTIONS", "NODE_PATH", "ELECTRON_RUN_AS_NODE"].includes(key),
      ),
    ),
  });
  await new Promise((resolve, reject) => {
    child.once("spawn", resolve);
    child.once("error", reject);
  });
  child.unref();
  return child;
}

// Rename within one volume, keep the previous bundle until the replacement
// acknowledges a loaded window, and restore it on swap or startup failure.
export async function installUpdate(
  job,
  { launch = launchApp, healthy, wait = delay } = {},
) {
  const backup = join(job.stage, "previous");
  const move = async (source, target) => {
    for (let attempt = 0; ; attempt++) {
      try {
        await rename(source, target);
        return;
      } catch (error) {
        // Windows renderer/relay handles can outlive the host briefly. Bound
        // retries to ten seconds; persistent locks or permissions keep the old app.
        if (attempt >= 40 || !["EPERM", "EBUSY", "EACCES"].includes(error.code))
          throw error;
        await wait(250);
      }
    }
  };
  let moved = false,
    replaced = false,
    child;
  try {
    await access(executablePath(job.candidate, job.platform));
    await move(job.install, backup);
    moved = true;
    await move(job.candidate, job.install);
    replaced = true;
    await writeResult(job.result, { phase: "launching", release: job.release });
    child = await launch(job.install, job.platform);
    const check =
      healthy ??
      (async () => {
        const value = JSON.parse(await readFile(job.result, "utf8"));
        return value.phase === "healthy" && value.release === job.release;
      });
    let ready = false;
    for (let i = 0; i < 120; i++) {
      if (child?.exitCode != null) break;
      if (await check()) {
        ready = true;
        break;
      }
      await wait(500);
    }
    if (!ready) throw new Error("The updated app did not start.");
    await writeResult(job.result, { phase: "complete", release: job.release });
  } catch {
    if (child?.pid) {
      try {
        process.kill(child.pid);
      } catch {
        /* The failed app may already have exited. */
      }
      for (let i = 0; i < 40; i++) {
        try {
          process.kill(child.pid, 0);
        } catch {
          break;
        }
        await wait(250);
      }
    }
    if (moved) {
      if (replaced) await move(job.install, job.candidate);
      await move(backup, job.install);
    }
    await writeResult(job.result, {
      phase: "error",
      message:
        "The update could not be installed. The previous app was retained. Close other Lane windows and Chrome capture sessions, check folder permissions, and retry.",
    });
    await launch(job.install, job.platform);
    return false;
  }
  // Cleanup failure must never roll back an app that already started correctly.
  await rm(backup, { recursive: true, force: true }).catch(() => {});
  return true;
}

async function main() {
  const job = JSON.parse(await readFile(process.argv[2], "utf8"));
  await writeFile(join(job.stage, "installer-pid"), String(process.pid));
  await writeFile(join(job.stage, "ready"), "ready");
  let exited = false;
  for (let i = 0; i < 240; i++) {
    try {
      await access(join(job.stage, "cancel"));
      return;
    } catch {
      /* No cancellation requested. */
    }
    try {
      process.kill(job.parentPid, 0);
    } catch (error) {
      if (error.code !== "ESRCH") throw error;
      exited = true;
      break;
    }
    await delay(500);
  }
  if (!exited) {
    await writeResult(job.result, {
      phase: "error",
      message:
        "Lane could not close for the update. Finish pending operations and retry.",
    });
    return;
  }
  // Quit during download never authorizes installation; only the explicit
  // update shutdown writes this marker after native workers finish.
  try {
    await access(join(job.stage, "install"));
  } catch {
    return;
  }
  await installUpdate(job);
}
if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  main().catch(() => {
    process.exitCode = 1;
  });
}
