// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import {
  chmod,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  rm,
  stat,
  symlink,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { gzipSync, gunzipSync } from "node:zlib";
import {
  compareReleases,
  DesktopUpdater,
  selectRelease,
} from "../native/desktop/updater.mjs";
import { extractUpdate } from "../native/desktop/update-archive.mjs";
import { installUpdate } from "../native/desktop/update-install.mjs";
import {
  executablePath,
  writeResult,
} from "../native/desktop/update-files.mjs";
import { desktopModules } from "../src/desktop.ts";
import { invokeNative } from "../native/desktop/bridge.mjs";
import { packArchive } from "../../../scripts/archive.mjs";

const installed = "delino@v1.2.3",
  next = "delino@v1.3.0";
const config = {
  name: "lane",
  scheme: "lane",
  releaseTag: installed,
  sourceRevision: "a".repeat(40),
  testing: false,
};
const archiveName = "lane-desktop-darwin-arm64-v1.3.0.tar.gz";
function release(tag = next, archive = archiveName, size = 100) {
  return {
    tag_name: tag,
    draft: false,
    prerelease: false,
    assets: [archive, "SHA256SUMS"].map((name) => ({
      name,
      size,
      browser_download_url: `https://github.com/delinoio/apps/releases/download/${tag}/${name}`,
    })),
  };
}
async function scratch(t: { after: (fn: () => Promise<unknown>) => void }) {
  const root = await mkdtemp(join(tmpdir(), "lane-update-test-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  return root;
}
async function write(path: string, value: string | Buffer, mode?: number) {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, value, { mode });
}

test("release selection compares numeric versions and ignores drafts, prereleases and unrelated tags", () => {
  assert.equal(compareReleases("delino@v1.10.0", "delino@v1.9.99"), 1);
  assert.equal(compareReleases("delino@v1.2.3", installed), 0);
  const candidate = release();
  assert.equal(
    selectRelease(
      [
        release("other@v9.0.0"),
        { ...release("delino@v9.0.0"), draft: true },
        { ...release("delino@v8.0.0"), prerelease: true },
        candidate,
      ],
      installed,
      "darwin-arm64",
    )?.release,
    next,
  );
  assert.equal(
    selectRelease([release(installed)], installed, "darwin-arm64"),
    undefined,
  );
  assert.equal(
    selectRelease([release("delino@v1.0.0")], installed, "darwin-arm64"),
    undefined,
  );
  assert.throws(
    () => selectRelease([candidate], installed, "windows-amd64"),
    /complete Lane update/,
  );
  assert.throws(
    () =>
      selectRelease(
        [{ ...candidate, assets: [...candidate.assets, candidate.assets[0]] }],
        installed,
        "darwin-arm64",
      ),
    /complete Lane update/,
  );
  assert.throws(
    () =>
      selectRelease(
        [
          {
            ...candidate,
            assets: candidate.assets.map((a) => ({
              ...a,
              browser_download_url: "https://example.org/update",
            })),
          },
        ],
        installed,
        "darwin-arm64",
      ),
    /address/,
  );
});

test("the updater bridge exposes only listed actions through the main host facade", async () => {
  const calls: string[] = [];
  const modules = desktopModules({
    call: (_channel, request, reply) => {
      void invokeNative(
        {
          host: {
            update: (action: string, callback: (raw: string) => void) => {
              calls.push(action);
              callback(
                JSON.stringify({ ok: true, response: { phase: "checking" } }),
              );
            },
          },
        },
        request,
      ).then(reply);
    },
  });
  const raw = await new Promise<string>((resolve) =>
    modules.host.update!("start", resolve),
  );
  assert.equal(JSON.parse(raw).response.phase, "checking");
  assert.deepEqual(calls, ["start"]);
});

test("tar extraction preserves executable modes, PAX long paths and internal framework symlinks", async (t) => {
  const root = await scratch(t),
    source = join(root, "source"),
    archive = join(root, "update.tar.gz"),
    destination = join(root, "extracted");
  const framework = "Lane.app/Contents/Frameworks/Test.framework";
  await write(join(source, framework, "Versions/A/engine"), "runtime", 0o755);
  await symlink("A", join(source, framework, "Versions/Current"));
  await symlink("Versions/Current/engine", join(source, framework, "engine"));
  const long = `Lane.app/Contents/Resources/${"very-long-path-".repeat(12)}/asset`;
  await write(join(source, long), "asset");
  execFileSync("tar", [
    "--format=pax",
    "-czf",
    archive,
    "-C",
    source,
    "Lane.app",
  ]);
  await mkdir(destination);
  await extractUpdate(archive, destination, "Lane.app");
  assert.equal(
    await readFile(join(destination, framework, "engine"), "utf8"),
    "runtime",
  );
  assert.equal(
    (await stat(join(destination, framework, "Versions/A/engine"))).mode &
      0o777,
    0o755,
  );
  assert.equal(await readFile(join(destination, long), "utf8"), "asset");
});

function rewriteHeader(bytes: Buffer, edit: (header: Buffer) => void) {
  const tar = gunzipSync(bytes),
    header = tar.subarray(0, 512);
  edit(header);
  header.fill(32, 148, 156);
  const sum = header.reduce((a, b) => a + b, 0);
  header.write(sum.toString(8).padStart(6, "0") + "\0 ", 148, 8, "ascii");
  return gzipSync(tar);
}
for (const [name, edit] of [
  [
    "parent traversal",
    (h: Buffer) => {
      h.fill(0, 0, 100);
      h.write("Lane.app/../../outside", 0);
    },
  ],
  [
    "absolute path",
    (h: Buffer) => {
      h.fill(0, 0, 100);
      h.write("/tmp/outside", 0);
    },
  ],
  [
    "external symlink",
    (h: Buffer) => {
      h[156] = 50;
      h.write("/tmp/outside", 157);
    },
  ],
  [
    "hard link",
    (h: Buffer) => {
      h[156] = 49;
    },
  ],
  [
    "device node",
    (h: Buffer) => {
      h[156] = 51;
    },
  ],
] as const) {
  test(`tar extraction rejects ${name}`, async (t) => {
    const root = await scratch(t),
      archive = join(root, "bad.tar.gz");
    const bytes = packArchive([{ name: "Lane.app/file", data: "" }]);
    await writeFile(archive, rewriteHeader(bytes, edit));
    await assert.rejects(
      extractUpdate(archive, join(root, "out"), "Lane.app"),
      /unsafe/,
    );
  });
}

test("tar extraction rejects truncation and writes through framework symlinks", async (t) => {
  const root = await scratch(t),
    archive = join(root, "bad.tar.gz");
  await writeFile(archive, Buffer.from("broken gzip"));
  await assert.rejects(extractUpdate(archive, join(root, "out"), "Lane.app"));
  const source = join(root, "source");
  await write(join(source, "Lane.app/link/file"), "payload");
  // A symlink replacing a previously created directory must fail, even when
  // both paths would otherwise remain within the bundle.
  const base = packArchive([{ name: "Lane.app/link/file", data: "payload" }]);
  const link = rewriteHeader(
    packArchive([{ name: "Lane.app/link", data: "" }]),
    (h) => {
      h[156] = 50;
      h.write("target", 157);
    },
  );
  await writeFile(
    archive,
    gzipSync(
      Buffer.concat([gunzipSync(base).subarray(0, -1024), gunzipSync(link)]),
    ),
  );
  await assert.rejects(extractUpdate(archive, join(root, "out2"), "Lane.app"));
});

async function updaterFixture(
  t: Parameters<typeof scratch>[0],
  overrides: {
    checksum?: string;
    tag?: string;
    downloadSize?: number;
    canRestart?: () => boolean;
    fail?: boolean;
  } = {},
) {
  const root = await scratch(t),
    install = join(root, "Lane.app"),
    resources = join(install, "Contents/Resources/app");
  await write(join(resources, "package.json"), JSON.stringify(config));
  const archive = packArchive([
    {
      name: "Lane.app/Contents/MacOS/lynxtron",
      data: "new runtime",
      mode: 0o755,
    },
    {
      name: "Lane.app/Contents/Resources/app/package.json",
      data: JSON.stringify({ ...config, releaseTag: overrides.tag || next }),
    },
  ]);
  const candidate = release(
    next,
    archiveName,
    overrides.downloadSize ?? archive.length,
  );
  const requests: string[] = [];
  let restarts = 0,
    prepared = 0;
  const updater = new DesktopUpdater({
    resources,
    install,
    stateDirectory: join(root, "state"),
    config,
    platform: "darwin",
    arch: "arm64",
    canRestart: overrides.canRestart,
    restart: () => restarts++,
    fetcher: async (url: string) => {
      requests.push(url);
      if (overrides.fail) throw new TypeError("private network information");
      if (url.includes("api.github.com"))
        return new Response(JSON.stringify([candidate]));
      if (url.endsWith("SHA256SUMS"))
        return new Response(
          `${overrides.checksum || createHash("sha256").update(archive).digest("hex")}  ${archiveName}\n`,
        );
      return new Response(archive);
    },
  });
  updater.prepareInstaller = async () => {
    prepared++;
  };
  return {
    root,
    updater,
    requests,
    restarts: () => restarts,
    prepared: () => prepared,
  };
}

test("one click downloads, verifies and prepares a matching release exactly once", async (t) => {
  const f = await updaterFixture(t);
  await Promise.all([f.updater.start(), f.updater.start()]);
  await f.updater.pending;
  assert.equal(f.updater.state.phase, "installing");
  assert.equal(f.restarts(), 1);
  assert.equal(f.prepared(), 1);
  assert.equal(f.requests.length, 3);
  assert.equal(
    await readFile(
      join(f.root, "Lane.app/Contents/Resources/app/package.json"),
      "utf8",
    ),
    JSON.stringify(config),
  );
});
for (const [name, options, message] of [
  ["checksum mismatch", { checksum: "0".repeat(64) }, /checksum/],
  ["truncated download", { downloadSize: 99999 }, /incomplete/],
  ["wrong release metadata", { tag: "delino@v9.0.0" }, /selected release/],
  ["network failure", { fail: true }, /connection/],
] as const) {
  test(`update failure preserves the installation: ${name}`, async (t) => {
    const f = await updaterFixture(t, options);
    await f.updater.start();
    await f.updater.pending;
    assert.equal(f.updater.state.phase, "error");
    assert.match(f.updater.state.message, message);
    assert.equal(f.prepared(), 0);
    assert.equal(f.restarts(), 0);
    assert.equal(
      (await readdir(f.root)).some((name) => name.startsWith(".lane-update-")),
      false,
    );
  });
}

test("open capture drafts prevent update and Settings navigation cancels installation", async (t) => {
  const blocked = await updaterFixture(t, { canRestart: () => false });
  await blocked.updater.start();
  assert.match(blocked.updater.state.message, /RealQA/);
  assert.equal(blocked.requests.length, 0);
  const cancelled = await updaterFixture(t);
  await cancelled.updater.start();
  cancelled.updater.cancel();
  await cancelled.updater.pending;
  assert.equal(cancelled.updater.state.phase, "idle");
  assert.equal(cancelled.restarts(), 0);
});

test("development, native-testing and unsupported builds cannot download updates", async (t) => {
  const f = await updaterFixture(t);
  for (const options of [
    { scheme: "lane-dev" },
    { testing: true },
    { releaseTag: "" },
  ]) {
    f.updater.config = { ...config, ...options };
    const blocked = new DesktopUpdater({
      resources: "unused",
      install: "unused",
      stateDirectory: f.root,
      config: f.updater.config,
      platform: "darwin",
      arch: "arm64",
      restart: () => assert.fail("must not restart"),
      fetcher: () => assert.fail("must not fetch"),
    });
    await blocked.start();
    assert.equal(blocked.state.phase, "unavailable");
  }
});

async function installFixture(t: Parameters<typeof scratch>[0]) {
  const root = await scratch(t),
    stage = join(root, ".lane-update-test"),
    install = join(root, "Lane.app"),
    candidate = join(stage, "extracted/Lane.app");
  for (const [app, text] of [
    [install, "old"],
    [candidate, "new"],
  ])
    await write(executablePath(app, "darwin"), text);
  return {
    stage,
    install,
    candidate,
    platform: "darwin",
    result: join(root, "result.json"),
    release: next,
  };
}
test("installer swaps the bundle and deletes backup only after a healthy startup", async (t) => {
  const job = await installFixture(t);
  assert.equal(
    await installUpdate(job, {
      launch: async () => {
        assert.equal(
          await readFile(executablePath(job.install, "darwin"), "utf8"),
          "new",
        );
        assert.equal(
          await readFile(
            executablePath(join(job.stage, "previous"), "darwin"),
            "utf8",
          ),
          "old",
        );
        return {};
      },
      healthy: async () => true,
    }),
    true,
  );
  assert.equal(
    JSON.parse(await readFile(job.result, "utf8")).phase,
    "complete",
  );
  await assert.rejects(stat(join(job.stage, "previous")));
});
for (const fail of ["spawn", "startup"]) {
  test(`installer rolls back after ${fail} failure and relaunches the previous app`, async (t) => {
    const job = await installFixture(t);
    let launches = 0;
    assert.equal(
      await installUpdate(job, {
        launch: async () => {
          launches++;
          if (launches === 1 && fail === "spawn")
            throw new Error("simulated launch failure");
          return {};
        },
        healthy: async () => false,
        wait: async () => {},
      }),
      false,
    );
    assert.equal(launches, 2);
    assert.equal(
      await readFile(executablePath(job.install, "darwin"), "utf8"),
      "old",
    );
    assert.equal(JSON.parse(await readFile(job.result, "utf8")).phase, "error");
  });
}
test("failed preflight leaves the old app in place", async (t) => {
  const job = await installFixture(t);
  await rm(job.candidate, { recursive: true });
  assert.equal(await installUpdate(job, { launch: async () => ({}) }), false);
  assert.equal(
    await readFile(executablePath(job.install, "darwin"), "utf8"),
    "old",
  );
});

test("a loaded replacement acknowledges the exact release and installer errors survive relaunch", async (t) => {
  const f = await updaterFixture(t);
  await mkdir(dirname(f.updater.result), { recursive: true });
  await writeResult(f.updater.result, {
    phase: "launching",
    release: installed,
  });
  await f.updater.healthy();
  assert.equal(
    JSON.parse(await readFile(f.updater.result, "utf8")).phase,
    "healthy",
  );
  await writeResult(f.updater.result, {
    phase: "error",
    message: "Retry safely.",
  });
  assert.equal((await f.updater.status()).message, "Retry safely.");
});

test("standalone installer waits for explicit shutdown permission and starts the replacement", async (t) => {
  const job = await installFixture(t);
  const { spawn } = await import("node:child_process");
  const childScript = `#!${process.execPath}\nrequire("node:fs").writeFileSync(${JSON.stringify(job.result)}, JSON.stringify({phase:"healthy",release:${JSON.stringify(next)}}));\nrequire("node:fs").writeFileSync(${JSON.stringify(join(job.stage, "app-pid"))},String(process.pid));\nsetTimeout(()=>{},30000);\n`;
  await writeFile(executablePath(job.candidate, "darwin"), childScript);
  await chmod(executablePath(job.candidate, "darwin"), 0o755);
  const jobFile = join(job.stage, "job.json");
  await writeFile(jobFile, JSON.stringify({ ...job, parentPid: 2147483647 }));
  const script = new URL(
    "../native/desktop/update-install.mjs",
    import.meta.url,
  );
  const run = () =>
    new Promise<number | null>((resolve, reject) => {
      const child = spawn(process.execPath, [script.pathname, jobFile], {
        stdio: "inherit",
      });
      child.once("error", reject);
      child.once("exit", resolve);
    });
  assert.equal(await run(), 0);
  assert.equal(
    await readFile(executablePath(job.install, "darwin"), "utf8"),
    "old",
  );
  await writeFile(join(job.stage, "install"), "install");
  assert.equal(await run(), 0);
  const pid = Number(await readFile(join(job.stage, "app-pid"), "utf8"));
  try {
    process.kill(pid);
  } catch {
    /* Fixture process may already be gone. */
  }
  assert.equal(
    JSON.parse(await readFile(job.result, "utf8")).phase,
    "complete",
  );
});
