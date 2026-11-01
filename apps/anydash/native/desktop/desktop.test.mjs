// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { createHash, generateKeyPairSync, sign } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { NativeAuth, pkcePair, subject } from "./auth.mjs";
import { Collector, ipcCall, socketPath } from "./collector.mjs";
import { DesktopHost, withDelcoreToken } from "./host.mjs";
import { memoryStore } from "./secure-store.mjs";
import {
  compareVersions,
  expectedDigest,
  platformKey,
  selectRelease,
  verifySums,
} from "./updater.mjs";

async function socketServer(handler) {
  const dir = await mkdtemp(join(tmpdir(), "anydash-ipc-"));
  const path = join(dir, "collector.sock");
  const server = createServer(handler);
  await new Promise((resolve) => server.listen(path, resolve));
  return {
    path,
    close: () =>
      new Promise((resolve) =>
        server.close(() =>
          rm(dir, { recursive: true, force: true }).then(resolve),
        ),
      ),
  };
}

test("speaks Connect binary over the collector socket", async () => {
  const seen = [];
  const server = await socketServer((request, response) => {
    seen.push({
      url: request.url,
      type: request.headers["content-type"],
      version: request.headers["connect-protocol-version"],
    });
    request.resume();
    request.on("end", () => {
      response.writeHead(200, { "content-type": "application/proto" });
      response.end(Buffer.from([8, 1]));
    });
  });
  try {
    const reply = await ipcCall(
      server.path,
      "GetCollectorStatus",
      Buffer.alloc(0),
    );
    assert.equal(reply.status, 200);
    assert.deepEqual([...reply.body], [8, 1]);
    assert.deepEqual(seen[0], {
      url: "/anydash.v1.LocalCollectorService/GetCollectorStatus",
      type: "application/proto",
      version: "1",
    });
  } finally {
    await server.close();
  }
});

test("connects to a running collector and stops only one it started", async () => {
  const calls = [];
  const running = new Collector({
    resources: "/missing",
    path: "/sock",
    call: async (_path, method) => {
      calls.push(method);
      return { status: 200, body: Buffer.alloc(0) };
    },
  });
  assert.equal(await running.ensure(), true);
  assert.equal(running.started, false);
  assert.deepEqual(await running.quit(), { stopped: false });
  assert.deepEqual(calls, ["GetCollectorStatus"]);

  let up = false;
  const launched = [];
  const embedded = await mkdtemp(join(tmpdir(), "anydash-bin-"));
  process.env.ANYDASH_COLLECTOR = process.execPath;
  try {
    const started = new Collector({
      resources: embedded,
      path: "/sock",
      call: async (_path, method) => {
        if (!up) throw new Error("ENOENT");
        calls.push(method);
        return { status: 200, body: Buffer.alloc(0) };
      },
      launch: (command, args) => {
        launched.push([command, args]);
        up = true;
        return { on() {} };
      },
    });
    assert.equal(await started.ensure({ interval: 1 }), true);
    assert.deepEqual(launched[0][1], ["run", "--owner", "desktop"]);
    assert.deepEqual(await started.quit(), { stopped: true });
    assert.equal(calls.at(-1), "Quit");
  } finally {
    delete process.env.ANYDASH_COLLECTOR;
    await rm(embedded, { recursive: true, force: true });
  }
});

test("uses the contract socket and pipe names", () => {
  assert.match(
    socketPath("darwin", {}),
    /Library\/Application Support\/io\.delino\.anydash\/collector\.sock$/,
  );
  assert.equal(
    socketPath("win32", {}, "S-1-5-21-1"),
    "\\\\.\\pipe\\io.delino.anydash.collector.S-1-5-21-1",
  );
});

function host(overrides = {}) {
  const requests = [];
  const forwarded = [];
  const cacheDirectory = join(
    tmpdir(),
    `anydash-cache-${process.pid}-${Math.random()}`,
  );
  const value = new DesktopHost({
    config: {
      version: "0.1.0",
      api: "https://anydash.api.delino.io",
      anydashResource: "https://anydash.api.delino.io",
      delcoreResource: "https://delcore.api.delino.io",
    },
    auth: {
      signedIn: async () => true,
      userKey: async () => "key",
      accessToken: async (resource) => `token:${resource}`,
      signIn: async () => {},
      signOut: async () => {},
    },
    collector: {
      forward: async (method, body) => {
        forwarded.push([method, body]);
        return {
          status: 200,
          body: Buffer.from([1]),
          contentType: "application/proto",
        };
      },
    },
    fetcher: async (url, init) => {
      requests.push({ url, init });
      return new Response(new Uint8Array([2]), {
        status: 200,
        headers: { "x-anydash-correlation-id": "c1" },
      });
    },
    clipboard: { writeText() {} },
    openExternal: async () => {},
    loginItems: {
      value: false,
      get() {
        return this.value;
      },
      set(enabled) {
        this.value = enabled;
      },
    },
    updater: { act: async () => ({ phase: "unavailable" }) },
    cacheDirectory,
    platformLabel: "macOS arm64",
    ...overrides,
  });
  return { value, requests, forwarded, cacheDirectory };
}

test("routes API and collector requests with the right credentials", async () => {
  const { value, requests, forwarded } = host();
  const api = JSON.parse(
    await value.reply(
      "rpc",
      JSON.stringify({
        target: "api",
        service: "anydash.v1.DashboardService",
        method: "ListSpaces",
        body: "AQ==",
        headers: { "X-AnyDash-Client-Kind": "CLIENT_KIND_DESKTOP", Host: "x" },
      }),
    ),
  );
  assert.equal(api.ok, true);
  assert.equal(api.value.correlationId, "c1");
  assert.equal(
    requests[0].url,
    "https://anydash.api.delino.io/anydash.v1.DashboardService/ListSpaces",
  );
  assert.equal(
    requests[0].init.headers.authorization,
    "Bearer token:https://anydash.api.delino.io",
  );
  assert.equal(requests[0].init.headers.Host, undefined);
  const local = JSON.parse(
    await value.reply(
      "rpc",
      JSON.stringify({
        target: "collector",
        service: "anydash.v1.LocalCollectorService",
        method: "PutNativeWidgetSnapshots",
        body: "AQ==",
      }),
    ),
  );
  assert.equal(local.ok, true);
  assert.equal(forwarded[0][0], "PutNativeWidgetSnapshots");
  // The renderer cannot stop the shared collector or reach other services.
  assert.equal(
    JSON.parse(
      await value.reply(
        "rpc",
        JSON.stringify({
          target: "collector",
          service: "anydash.v1.LocalCollectorService",
          method: "Quit",
        }),
      ),
    ).ok,
    false,
  );
  assert.equal(
    JSON.parse(
      await value.reply(
        "rpc",
        JSON.stringify({
          target: "api",
          service: "anydash.v1.LocalCollectorService",
          method: "Quit",
        }),
      ),
    ).ok,
    false,
  );
  assert.equal(
    JSON.parse(
      await value.reply(
        "openURL",
        JSON.stringify({ url: "file:///etc/passwd" }),
      ),
    ).ok,
    false,
  );
});

test("injects the Delcore token in the host instead of returning it", async () => {
  const { value, requests } = host();
  assert.equal(
    JSON.parse(await value.reply("delcoreToken", "{}")).error.code,
    "UNSUPPORTED",
  );
  for (const [service, method] of [
    ["anydash.v1.AccountService", "ConnectAccount"],
    ["anydash.v1.DashboardService", "ListSpaces"],
    ["anydash.v1.AccountService", "GetAccount"],
  ])
    await value.call("rpc", {
      target: "api",
      service,
      method,
      body: "",
      headers: {},
    });
  const token = Buffer.from("token:https://delcore.api.delino.io");
  const expected = Buffer.concat([Buffer.from([0x0a, token.length]), token]);
  assert.deepEqual(requests[0].init.body, expected);
  assert.deepEqual(requests[1].init.body, expected);
  assert.equal(requests[2].init.body.length, 0);
  // JWT-length tokens use a multi-byte varint length.
  const long = withDelcoreToken(Buffer.from([0x0a, 0]), "j".repeat(300));
  assert.deepEqual([...long.subarray(0, 5)], [0x0a, 0, 0x0a, 0xac, 0x02]);
  assert.equal(long.length, 5 + 300);
});

test("keeps the widget cache by user key and purges by prefix", async () => {
  const { value, cacheDirectory } = host();
  try {
    await value.call("cacheWrite", {
      key: "anydash.v1/u/widget/a",
      value: "A",
    });
    await value.call("cacheWrite", {
      key: "anydash.v1/u/widget/b",
      value: "B",
    });
    assert.deepEqual(
      await value.call("cacheRead", { key: "anydash.v1/u/widget/a" }),
      { value: "A" },
    );
    await value.call("cacheRemove", { prefix: "anydash.v1/u/widget/a" });
    assert.deepEqual(
      await value.call("cacheRead", { key: "anydash.v1/u/widget/a" }),
      {},
    );
    assert.deepEqual(
      await value.call("cacheRead", { key: "anydash.v1/u/widget/b" }),
      { value: "B" },
    );
    await value.call("signOut", {});
    assert.deepEqual(
      await value.call("cacheRead", { key: "anydash.v1/u/widget/b" }),
      {},
    );
  } finally {
    await rm(cacheDirectory, { recursive: true, force: true });
  }
});

test("toggles start at login on request", async () => {
  const { value } = host();
  assert.deepEqual(await value.call("autostart", {}), { enabled: false });
  assert.deepEqual(await value.call("autostart", { enabled: true }), {
    enabled: true,
  });
});

test("selects the newest stable signed release for this platform", () => {
  const asset = (tag, name, size = 10) => ({
    name,
    size,
    browser_download_url: `https://github.com/delinoio/apps/releases/download/${tag}/${name}`,
  });
  const release = (tag, extra = {}) => ({
    tag_name: tag,
    assets: [
      asset(tag, `anydash-desktop-darwin-arm64-${tag.slice(9)}.tar.gz`),
      asset(tag, "SHA256SUMS"),
      asset(tag, "SHA256SUMS.sig"),
    ],
    ...extra,
  });
  const releases = [
    release("anydash@v1.2.0"),
    release("anydash@v1.10.0"),
    release("anydash@v2.0.0", { prerelease: true }),
    release("lane@v9.0.0"),
  ];
  const selected = selectRelease(releases, "1.2.0", "darwin-arm64");
  assert.equal(selected.version, "1.10.0");
  assert.equal(selectRelease(releases, "1.10.0", "darwin-arm64"), undefined);
  assert.throws(() =>
    selectRelease(
      [release("anydash@v3.0.0", { assets: [] })],
      "1.0.0",
      "darwin-arm64",
    ),
  );
  assert.equal(compareVersions("anydash@v1.10.0", "1.9.9"), 1);
  assert.equal(platformKey("win32", "arm64"), undefined);
  assert.equal(platformKey("darwin", "x64"), "darwin-x64");
});

test("verifies the Ed25519 signature and checksum before installing", () => {
  const { privateKey, publicKey } = generateKeyPairSync("ed25519");
  const archive = Buffer.from("archive");
  const name = "anydash-desktop-darwin-arm64-1.0.0.tar.gz";
  const sums = Buffer.from(
    `${createHash("sha256").update(archive).digest("hex")}  ${name}\n`,
  );
  const signature = Buffer.from(
    sign(null, sums, privateKey).toString("base64"),
  );
  const key = publicKey
    .export({ format: "der", type: "spki" })
    .toString("base64");
  assert.equal(verifySums(sums, signature, key), true);
  assert.equal(verifySums(Buffer.from(`${sums}x`), signature, key), false);
  assert.equal(
    expectedDigest(sums, name),
    createHash("sha256").update(archive).digest("hex"),
  );
  assert.throws(() => expectedDigest(sums, "other.tar.gz"));
});

test("signs in with PKCE and requests per-resource tokens", async () => {
  const store = memoryStore();
  const bodies = [];
  let opened;
  const idToken = `x.${Buffer.from(JSON.stringify({ sub: "user-1" })).toString("base64url")}.y`;
  const auth = new NativeAuth({
    endpoint: "https://auth.example",
    appId: "native-app",
    resources: [
      "https://anydash.api.delino.io",
      "https://delcore.api.delino.io",
    ],
    store,
    openBrowser: (url) => {
      opened = new URL(url);
    },
    fetcher: async (_url, init) => {
      const body = new URLSearchParams(init.body);
      bodies.push(body);
      return new Response(
        JSON.stringify(
          body.get("grant_type") === "authorization_code"
            ? { refresh_token: "r1", id_token: idToken }
            : {
                access_token: `a:${body.get("resource")}`,
                refresh_token: "r2",
                expires_in: 3600,
              },
        ),
        { status: 200 },
      );
    },
  });
  const signing = auth.signIn();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(opened.searchParams.get("code_challenge_method"), "S256");
  assert.deepEqual(opened.searchParams.getAll("resource"), [
    "https://anydash.api.delino.io",
    "https://delcore.api.delino.io",
  ]);
  assert.equal(
    await auth.callback("anydash://auth/callback?code=c&state=wrong"),
    false,
  );
  assert.equal(
    await auth.callback(
      `anydash://auth/callback?code=c&state=${opened.searchParams.get("state")}`,
    ),
    true,
  );
  await signing;
  assert.ok(bodies[0].get("code_verifier"));
  assert.equal(
    await auth.accessToken("https://delcore.api.delino.io"),
    "a:https://delcore.api.delino.io",
  );
  assert.equal(store.values.get("refresh-token"), "r2");
  assert.equal(
    store.values.get("user-key"),
    createHash("sha256").update("anydash:user-1").digest("hex"),
  );
  await auth.signOut();
  assert.equal(await auth.signedIn(), false);
  assert.equal(subject("bad"), undefined);
  const { verifier, challenge } = pkcePair();
  assert.equal(
    challenge,
    createHash("sha256").update(verifier).digest("base64url"),
  );
});
