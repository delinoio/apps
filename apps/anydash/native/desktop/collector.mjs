// SPDX-License-Identifier: Apache-2.0
// Shared collector integration: locate the embedded `anydash` executable,
// connect to the running collector over LocalCollectorService local IPC and
// start `anydash run --owner desktop` when none runs. The app and the CLI
// share one collector per OS user, so the app never collects twice.
import { execFileSync, spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { request } from "node:http";
import { homedir } from "node:os";
import { join } from "node:path";

export const SERVICE = "anydash.v1.LocalCollectorService";

/** Data directory from the CLI contract; ANYDASH_HOME overrides it. */
export function dataDirectory(platform = process.platform, env = process.env) {
  if (env.ANYDASH_HOME) return join(env.ANYDASH_HOME, "data");
  if (platform === "darwin")
    return join(homedir(), "Library/Application Support/io.delino.anydash");
  if (platform === "win32")
    return join(
      env.LOCALAPPDATA || join(homedir(), "AppData/Local"),
      "Delino/AnyDash/Data",
    );
  return join(env.XDG_DATA_HOME || join(homedir(), ".local/share"), "anydash");
}

/** Current user SID for the Windows named pipe name. */
export function windowsUserSid(run = execFileSync) {
  const output = run("whoami", ["/user", "/fo", "csv", "/nh"], {
    encoding: "utf8",
    windowsHide: true,
  });
  const sid = /"(S-1-[0-9-]+)"/.exec(String(output))?.[1];
  if (!sid) throw new Error("The Windows user SID is unavailable.");
  return sid;
}

/** Unix domain socket on macOS, named pipe restricted to the user on Windows. */
export function socketPath(
  platform = process.platform,
  env = process.env,
  sid,
) {
  if (platform === "win32")
    return `\\\\.\\pipe\\io.delino.anydash.collector.${sid ?? windowsUserSid()}`;
  if (platform === "linux" && env.XDG_RUNTIME_DIR && !env.ANYDASH_HOME)
    return join(env.XDG_RUNTIME_DIR, "anydash", "collector.sock");
  return join(dataDirectory(platform, env), "collector.sock");
}

/** The embedded executable ships in the app resources next to main.mjs. */
export function collectorExecutable(
  resources,
  platform = process.platform,
  env = process.env,
) {
  const name = platform === "win32" ? "anydash.exe" : "anydash";
  // Development builds may point at a locally built collector.
  if (env.ANYDASH_COLLECTOR && existsSync(env.ANYDASH_COLLECTOR))
    return env.ANYDASH_COLLECTOR;
  const embedded = join(resources, "bin", name);
  return existsSync(embedded) ? embedded : undefined;
}

/**
 * One unary Connect call over local IPC (HTTP/1.1, binary Protobuf).
 * Returns the raw status and body; the shared client decodes them.
 */
export function ipcCall(path, method, body, { timeoutMs = 10_000 } = {}) {
  return new Promise((resolve, reject) => {
    const req = request(
      {
        socketPath: path,
        method: "POST",
        path: `/${SERVICE}/${method}`,
        headers: {
          "content-type": "application/proto",
          "connect-protocol-version": "1",
          "content-length": body.length,
          host: "localhost",
        },
        timeout: timeoutMs,
      },
      (response) => {
        const chunks = [];
        response.on("data", (chunk) => chunks.push(chunk));
        response.on("end", () =>
          resolve({
            status: response.statusCode ?? 0,
            body: Buffer.concat(chunks),
            contentType: response.headers["content-type"],
          }),
        );
        response.on("error", reject);
      },
    );
    req.on("timeout", () =>
      req.destroy(new Error("The collector did not respond.")),
    );
    req.on("error", reject);
    req.end(body);
  });
}

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

export class Collector {
  /** True when this app started the collector and therefore stops it on Quit. */
  started = false;
  #child;

  constructor({
    resources,
    path = socketPath(),
    call = ipcCall,
    launch = spawn,
    log = () => {},
  }) {
    this.resources = resources;
    this.path = path;
    this.call = call;
    this.launch = launch;
    this.log = log;
  }

  async running() {
    try {
      const reply = await this.call(
        this.path,
        "GetCollectorStatus",
        Buffer.alloc(0),
        { timeoutMs: 2000 },
      );
      return reply.status === 200;
    } catch {
      return false;
    }
  }

  /** Connects to the shared collector, starting the embedded one if needed. */
  async ensure({ attempts = 40, interval = 250 } = {}) {
    if (await this.running()) return true;
    const executable = collectorExecutable(this.resources);
    if (!executable) {
      this.log({ event: "anydash_collector_missing" });
      return false;
    }
    this.#child = this.launch(executable, ["run", "--owner", "desktop"], {
      detached: false,
      stdio: "ignore",
      windowsHide: true,
    });
    this.#child.on?.("exit", (code) =>
      this.log({ event: "anydash_collector_exited", code }),
    );
    this.started = true;
    this.log({ event: "anydash_collector_started" });
    for (let attempt = 0; attempt < attempts; attempt++) {
      await delay(interval);
      if (await this.running()) return true;
    }
    this.log({ event: "anydash_collector_unreachable" });
    return false;
  }

  /** Forwards a client request; the bridge already validated the method. */
  async forward(method, body) {
    let reply;
    try {
      reply = await this.call(this.path, method, body);
    } catch {
      // A collector that exited is started again once, then the call retries.
      if (!(await this.ensure()))
        throw new Error("The AnyDash collector isn’t running.");
      reply = await this.call(this.path, method, body);
    }
    return reply;
  }

  /**
   * Quit stops a collector this app started. A service-managed collector
   * answers SERVICE_MANAGED and keeps running; a CLI foreground run stays.
   */
  async quit() {
    if (!this.started) return { stopped: false };
    try {
      const reply = await this.call(this.path, "Quit", Buffer.alloc(0), {
        timeoutMs: 5000,
      });
      return { stopped: reply.status === 200 };
    } catch {
      this.#child?.kill?.();
      return { stopped: true };
    }
  }
}
