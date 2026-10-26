// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { test } from "node:test";
import { createHash, generateKeyPairSync, sign } from "node:crypto";
import {
  mkdtemp,
  mkdir,
  readFile,
  writeFile,
  readdir,
  rm,
  stat,
  copyFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFile, spawn } from "node:child_process";
import { promisify } from "node:util";
import { once, EventEmitter } from "node:events";
import {
  archiveName,
  assetURL,
  desktopUpdateConfig,
  newerVersion,
  verifyManifest,
} from "../native/desktop/update-contract.mjs";
import {
  DesktopUpdater,
  downloadArchive,
  replacementScript,
} from "../native/desktop/updater.mjs";
import { prepareDesktopUpdate } from "./desktop-update-release.mjs";

const execute = promisify(execFile);
const keys = generateKeyPairSync("ed25519");
const publicKey = keys.publicKey
  .export({ format: "der", type: "spki" })
  .toString("base64");
const config = {
  name: "lane",
  version: "1.0.0",
  scheme: "lane",
  testing: false,
  updates: { publicKey, target: "win32-x64" },
};
const archive = Buffer.from("signed fixture archive");
function envelope(overrides = {}, key = keys.privateKey) {
  const manifest = {
    schema: 1,
    version: "1.1.0",
    target: "win32-x64",
    url: assetURL("1.1.0", archiveName("1.1.0", "win32-x64")),
    size: archive.length,
    sha256: createHash("sha256").update(archive).digest("hex"),
    ...overrides,
  };
  const payload = Buffer.from(JSON.stringify(manifest));
  return {
    payload: payload.toString("base64"),
    signature: sign(null, payload, key).toString("base64"),
  };
}
const verifyOptions = {
  publicKey,
  version: "1.1.0",
  platform: "win32",
  arch: "x64",
};
async function scratch(t) {
  const root = await mkdtemp(join(tmpdir(), "lane-update-test-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  return root;
}

test("stable versions reject downgrade, equality, prerelease and unsafe components", () => {
  assert.equal(newerVersion("1.10.0", "1.9.9"), true);
  assert.equal(newerVersion("1.0.0", "1.0.0"), false);
  assert.equal(newerVersion("0.9.0", "1.0.0"), false);
  for (const value of [
    "01.0.0",
    "1.0.0-beta",
    "1.0.0+metadata",
    "9007199254740992.0.0",
  ])
    assert.throws(() => newerVersion(value, "1.0.0"));
});

test("release builds validate the Ed25519 key and reject development/test configurations", () => {
  const input = {
    publicKey,
    scheme: "lane",
    testing: false,
    version: "1.0.0",
    platform: "darwin",
    arch: "arm64",
  };
  assert.deepEqual(desktopUpdateConfig(input), {
    publicKey,
    target: "darwin-arm64",
  });
  assert.equal(
    desktopUpdateConfig({ ...input, publicKey: undefined }),
    undefined,
  );
  for (const overrides of [
    { scheme: "lane-dev" },
    { testing: true },
    { publicKey: "bad" },
    { platform: "linux" },
  ])
    assert.throws(() => desktopUpdateConfig({ ...input, ...overrides }));
});

test("signature binds the version, platform, architecture, asset origin, hash and size", () => {
  assert.equal(verifyManifest(envelope(), verifyOptions).version, "1.1.0");
  const bad = envelope();
  bad.payload = Buffer.from("{}").toString("base64");
  assert.throws(() => verifyManifest(bad, verifyOptions), /signature/);
  const wrongKey = generateKeyPairSync("ed25519");
  assert.throws(
    () => verifyManifest(envelope({}, wrongKey.privateKey), verifyOptions),
    /signature/,
  );
  for (const overrides of [
    { schema: 2 },
    { version: "1.2.0" },
    { target: "darwin-arm64" },
    { url: "https://example.com/evil.zip" },
    { sha256: "bad" },
    { size: -1 },
    { size: 2 ** 32 },
  ])
    assert.throws(() => verifyManifest(envelope(overrides), verifyOptions));
});

test("download rejects truncated, oversized and corrupted signed archives", async (t) => {
  const root = await scratch(t);
  const manifest = verifyManifest(envelope(), verifyOptions);
  for (const [index, content] of [
    archive.subarray(1),
    Buffer.concat([archive, archive]),
    Buffer.alloc(archive.length),
  ].entries()) {
    await assert.rejects(
      downloadArchive(
        manifest,
        join(root, String(index)),
        async () => new Response(content),
      ),
    );
  }
  await downloadArchive(
    manifest,
    join(root, "valid"),
    async () => new Response(archive),
  );
  assert.deepEqual(await readFile(join(root, "valid")), archive);
});

async function updaterFixture(
  t,
  { metadata = envelope(), bytes = archive, candidate = {}, releases } = {},
) {
  const root = await scratch(t);
  const installed = join(root, "application");
  await mkdir(installed);
  const events = [],
    commands = [],
    requests = [],
    launches = [];
  let updater;
  updater = new DesktopUpdater({
    config,
    installed,
    platform: "win32",
    arch: "x64",
    log: (event) => events.push(event),
    fetcher: async (url) => {
      requests.push(url);
      if (url.includes("api.github.com"))
        return Response.json(
          releases ?? [
            { tag_name: "delino@v99.0.0" },
            { tag_name: "lane-desktop@v1.2.0", prerelease: true },
            {
              tag_name: "lane-desktop@v1.1.0",
              draft: false,
              prerelease: false,
            },
          ],
        );
      if (url.endsWith(".json")) return Response.json(metadata);
      return new Response(bytes);
    },
    executeCommand: async (...args) => {
      commands.push(args);
      const [stage] = (await readdir(root)).filter((name) =>
        name.startsWith(".lane-update-"),
      );
      const resource = join(root, stage, "unpack/application/resources/app");
      await mkdir(resource, { recursive: true });
      await writeFile(
        join(resource, "package.json"),
        JSON.stringify({ ...config, version: "1.1.0", ...candidate }),
      );
      await writeFile(join(root, stage, "unpack/application/lynxtron.exe"), "");
    },
    launch: (...args) => {
      launches.push(args);
      const child = new EventEmitter();
      child.unref = () => {};
      return child;
    },
  });
  t.after(() => updater.stop());
  return { root, installed, updater, events, commands, requests, launches };
}

test("release discovery stages a verified update and installs only after explicit shutdown", async (t) => {
  const fixture = await updaterFixture(t);
  await fixture.updater.check();
  assert.ok(fixture.updater.ready);
  assert.deepEqual(fixture.events, [
    "lane_update_checking",
    "lane_update_ready",
  ]);
  assert.equal(fixture.commands.length, 1);
  assert.equal(fixture.launches.length, 0);
  assert.ok((await stat(fixture.installed)).isDirectory());
  await fixture.updater.check();
  assert.equal(fixture.requests.length, 3);
  fixture.updater.installOnExit();
  fixture.updater.installOnExit();
  assert.equal(fixture.launches.length, 1);
  assert.equal(fixture.launches[0][0], "powershell.exe");
  assert.equal(fixture.launches[0][2].detached, true);
});

test("invalid signatures and corrupt downloads never reach extraction or installation", async (t) => {
  for (const options of [
    { metadata: envelope({ target: "darwin-x64" }) },
    { bytes: Buffer.from("corrupt") },
  ]) {
    const fixture = await updaterFixture(t, options);
    await fixture.updater.check();
    assert.equal(fixture.updater.ready, undefined);
    assert.equal(fixture.commands.length, 0);
    assert.ok(fixture.events.includes("lane_update_failed"));
    assert.deepEqual(await readdir(fixture.root), ["application"]);
  }
});

test("candidate identity mismatch and shutdown during a check discard staging", async (t) => {
  for (const candidate of [
    { testing: true },
    { version: "1.0.0" },
    { updates: undefined },
  ]) {
    const fixture = await updaterFixture(t, { candidate });
    await fixture.updater.check();
    assert.equal(fixture.updater.ready, undefined);
    assert.deepEqual(await readdir(fixture.root), ["application"]);
  }
  const fixture = await updaterFixture(t);
  const extract = fixture.updater.executeCommand;
  fixture.updater.executeCommand = async (...args) => {
    await extract(...args);
    fixture.updater.stop();
  };
  await fixture.updater.check();
  assert.equal(fixture.updater.ready, undefined);
  assert.deepEqual(await readdir(fixture.root), ["application"]);
});

test("development/test builds and older releases cause no update download", async (t) => {
  for (const overrides of [
    { updates: undefined },
    { scheme: "lane-dev" },
    { testing: true },
  ]) {
    const fixture = await updaterFixture(t);
    fixture.updater.config = { ...config, ...overrides };
    await fixture.updater.start();
    assert.equal(fixture.requests.length, 0);
  }
  const fixture = await updaterFixture(t, {
    releases: [
      { tag_name: "lane-desktop@v0.9.0" },
      { tag_name: "lane-desktop@v1.0.0" },
    ],
  });
  await fixture.updater.check();
  assert.equal(fixture.requests.length, 1);
  assert.equal(fixture.updater.ready, undefined);
});

test("network failures are retryable and concurrent checks are suppressed", async (t) => {
  const fixture = await updaterFixture(t);
  const fetcher = fixture.updater.fetcher;
  fixture.updater.fetcher = async () => {
    throw new Error("offline");
  };
  await fixture.updater.check();
  assert.ok(fixture.events.includes("lane_update_failed"));
  fixture.updater.fetcher = fetcher;
  await Promise.all([fixture.updater.check(), fixture.updater.check()]);
  assert.ok(fixture.updater.ready);
  assert.equal(fixture.requests.length, 3);
});

test(
  "macOS replacement performs a real swap and restores the old app after a failed swap",
  { skip: process.platform === "win32" },
  async (t) => {
    const root = await scratch(t);
    // An exited child gives a valid PID without waiting on the test runner.
    const parent = spawn("/bin/sh", ["-c", "exit 0"]);
    await once(parent, "exit");
    for (const fail of [false, true]) {
      const directory = join(root, `quote ' space $() ${fail}`);
      const installed = join(directory, "Lane.app"),
        stage = join(directory, ".stage"),
        candidate = join(stage, "candidate");
      await mkdir(installed, { recursive: true });
      await mkdir(stage);
      await writeFile(join(installed, "old"), "old");
      if (!fail) {
        await mkdir(candidate);
        await writeFile(join(candidate, "new"), "new");
      }
      const script = join(stage, "install.sh");
      await writeFile(
        script,
        replacementScript({
          platform: "darwin",
          pid: parent.pid,
          installed,
          candidate,
          stage,
        }),
      );
      const result = execute("/bin/sh", [script]);
      if (fail) {
        await assert.rejects(result);
        assert.equal(await readFile(join(installed, "old"), "utf8"), "old");
      } else {
        await result;
        assert.equal(await readFile(join(installed, "new"), "utf8"), "new");
      }
    }
  },
);

test("release preparation signs exact archive bytes and refuses mismatched keys", async (t) => {
  const root = await scratch(t),
    application = join(root, "application"),
    output = join(root, "release");
  await mkdir(join(application, "resources/app"), { recursive: true });
  await writeFile(
    join(application, "resources/app/package.json"),
    JSON.stringify({ ...config, version: "1.1.0" }),
  );
  await writeFile(join(application, "lynxtron.exe"), "");
  const privateKeyFile = join(root, "private.pem");
  await writeFile(
    privateKeyFile,
    keys.privateKey.export({ format: "pem", type: "pkcs8" }),
  );
  const options = {
    application,
    output,
    privateKeyFile,
    platform: "win32",
    arch: "x64",
    executeCommand: async () =>
      writeFile(join(output, archiveName("1.1.0", "win32-x64")), archive),
  };
  const manifest = await prepareDesktopUpdate(options);
  assert.deepEqual(
    verifyManifest(
      JSON.parse(await readFile(join(output, "update-win32-x64.json"), "utf8")),
      verifyOptions,
    ),
    manifest,
  );
  await assert.rejects(prepareDesktopUpdate(options), /already exists/);
  const wrong = generateKeyPairSync("ed25519");
  await writeFile(
    privateKeyFile,
    wrong.privateKey.export({ format: "pem", type: "pkcs8" }),
  );
  await assert.rejects(
    prepareDesktopUpdate({ ...options, output: join(root, "wrong") }),
    /does not match/,
  );
  await assert.rejects(
    prepareDesktopUpdate({ ...options, output: join(application, "output") }),
    /outside the application/,
  );
});

test(
  "macOS signed fixture passes actual archive, extraction and code signature checks",
  { skip: process.platform !== "darwin" },
  async (t) => {
    const root = await scratch(t);
    const application = join(root, "source/Lane.app");
    const resources = join(application, "Contents/Resources/app");
    const target = `darwin-${process.arch}`;
    const packaged = {
      ...config,
      version: "1.1.0",
      updates: { publicKey, target },
    };
    await mkdir(resources, { recursive: true });
    await mkdir(join(application, "Contents/MacOS"));
    await copyFile(
      "/usr/bin/true",
      join(application, "Contents/MacOS/lynxtron"),
    );
    await writeFile(
      join(application, "Contents/Info.plist"),
      '<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>io.delino.lane.update-fixture</string><key>CFBundleExecutable</key><string>lynxtron</string><key>CFBundleVersion</key><string>1.1.0</string></dict></plist>',
    );
    await writeFile(join(resources, "package.json"), JSON.stringify(packaged));
    await execute("/usr/bin/codesign", ["--force", "--sign", "-", application]);
    const privateKeyFile = join(root, "fixture.pem");
    await writeFile(
      privateKeyFile,
      keys.privateKey.export({ format: "pem", type: "pkcs8" }),
    );
    const output = join(root, "release");
    await prepareDesktopUpdate({ application, output, privateKeyFile });
    const installed = join(root, "installed/Lane.app");
    await mkdir(installed, { recursive: true });
    const updater = new DesktopUpdater({
      config: { ...packaged, version: "1.0.0" },
      installed,
      log: () => {},
      fetcher: async (url) =>
        url.includes("api.github.com")
          ? Response.json([{ tag_name: "lane-desktop@v1.1.0" }])
          : new Response(await readFile(join(output, url.split("/").at(-1)))),
    });
    t.after(() => updater.stop());
    await updater.check();
    assert.ok(updater.ready);
    const candidate = join(updater.ready.stage, "unpack/Lane.app");
    await execute("/usr/bin/codesign", [
      "--verify",
      "--deep",
      "--strict",
      candidate,
    ]);
    assert.equal(
      JSON.parse(
        await readFile(
          join(candidate, "Contents/Resources/app/package.json"),
          "utf8",
        ),
      ).version,
      "1.1.0",
    );
    // Do not launch a replacement helper that waits on the test runner's PID.
    await rm(updater.ready.stage, { recursive: true, force: true });
  },
);
