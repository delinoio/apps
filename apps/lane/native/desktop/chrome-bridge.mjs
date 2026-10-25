// SPDX-License-Identifier: Apache-2.0
import net from "node:net";
import { randomBytes, createHash, timingSafeEqual } from "node:crypto";
import {
  mkdir,
  readFile,
  writeFile,
  rename,
  chmod,
  unlink,
} from "node:fs/promises";
import { join } from "node:path";
import { encodeMessage, readMessages } from "./chrome-framing.mjs";
const hash = (value) => createHash("sha256").update(value).digest("hex");
const equal = (a, b) =>
  typeof a === "string" &&
  typeof b === "string" &&
  a.length === b.length &&
  timingSafeEqual(Buffer.from(a), Buffer.from(b));
export function chromeEndpoint(dataRoot, platform = process.platform) {
  return platform === "win32"
    ? `\\\\.\\pipe\\lane-realqa-${hash(dataRoot).slice(0, 32)}`
    : join(dataRoot, "realqa-chrome.sock");
}
const validId = (value) =>
  typeof value === "string" && /^[a-p]{32}$/.test(value);
function validateCapture(message) {
  const { pngBase64, width, height, metadata, clipped } = message;
  if (
    typeof pngBase64 !== "string" ||
    !Number.isSafeInteger(width) ||
    !Number.isSafeInteger(height) ||
    width < 1 ||
    height < 1 ||
    !/^[A-Za-z0-9+/]*={0,2}$/.test(pngBase64)
  )
    throw new Error("INVALID_CAPTURE");
  const png = Buffer.from(pngBase64, "base64");
  if (
    png.length < 33 ||
    png.toString("base64") !== pngBase64 ||
    !png
      .subarray(0, 8)
      .equals(Buffer.from([137, 80, 78, 71, 13, 10, 26, 10])) ||
    png.toString("ascii", 12, 16) !== "IHDR" ||
    png.readUInt32BE(16) !== width ||
    png.readUInt32BE(20) !== height
  )
    throw new Error("INVALID_CAPTURE");
  if (
    !metadata ||
    !["pageUrl", "pageTitle", "cssSelector"].every(
      (key) => typeof metadata[key] === "string",
    )
  )
    throw new Error("INVALID_METADATA");
  const url = new URL(metadata.pageUrl);
  if (
    !["http:", "https:"].includes(url.protocol) ||
    url.username ||
    url.password
  )
    throw new Error("INVALID_METADATA");
  return {
    pngBase64,
    width,
    height,
    metadata: {
      pageUrl: url.href,
      pageTitle: metadata.pageTitle,
      cssSelector: metadata.cssSelector,
    },
    clipped: clipped === true,
  };
}
export async function createChromeBridge({
  dataRoot,
  extensionIds,
  requestApproval,
  acceptCapture,
  onStatus = () => {},
}) {
  if (
    !Array.isArray(extensionIds) ||
    !extensionIds.length ||
    extensionIds.some((id) => !validId(id))
  )
    throw new Error("CHROME_EXTENSION_ID_REQUIRED");
  if (
    typeof requestApproval !== "function" ||
    typeof acceptCapture !== "function"
  )
    throw new Error("CHROME_CALLBACK_REQUIRED");
  await mkdir(dataRoot, { recursive: true, mode: 0o700 });
  const statePath = join(dataRoot, "realqa-chrome-connections.json"),
    endpoint = chromeEndpoint(dataRoot),
    discovery = join(dataRoot, "realqa-chrome-runtime.json");
  let approvals = {};
  try {
    approvals = JSON.parse(await readFile(statePath, "utf8"));
    if (!approvals || Array.isArray(approvals) || typeof approvals !== "object")
      throw new Error("INVALID_STATE");
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
  const persist = async () => {
    const temporary = `${statePath}.tmp`;
    await writeFile(temporary, JSON.stringify(approvals), { mode: 0o600 });
    await rename(temporary, statePath);
  };
  const connections = new Set();
  const approving = new Set();
  const approvalEpoch = new Map();
  const selections = new Map();
  let closed = false;
  let mutations = Promise.resolve();
  const mutate = (action) => {
    const result = mutations.then(action);
    mutations = result.catch(() => {});
    return result;
  };
  const secret = randomBytes(32).toString("hex");
  const authenticated = (message, origin) => {
    const value = approvals[message.profileId];
    return (
      value &&
      value.extensionId === origin &&
      typeof message.token === "string" &&
      equal(value.tokenHash, hash(message.token))
    );
  };
  const revoke = (profileId) =>
    mutate(async () => {
      approvalEpoch.set(profileId, (approvalEpoch.get(profileId) || 0) + 1);
      delete approvals[profileId];
      await persist();
      for (const item of connections)
        if (item.profileId === profileId) {
          item.profileId = undefined;
          item.socket.end(
            encodeMessage({ type: "revoked", ok: false, reason: "REVOKED" }),
          );
        }
      onStatus();
    });
  const server = net.createServer((socket) => {
    const client = { socket, origin: null, profileId: null };
    connections.add(client);
    socket.on("error", () => {});
    socket.on("close", () => {
      connections.delete(client);
      onStatus();
    });
    const send = (value) => {
      if (!socket.destroyed) socket.write(encodeMessage(value));
    };
    let queue = Promise.resolve();
    const processMessage = async (message) => {
      if (closed) return;
      if (!message || typeof message !== "object")
        throw new Error("INVALID_REQUEST");
      if (!client.origin) {
        if (
          message.type !== "relay" ||
          !equal(message.secret, secret) ||
          !extensionIds.includes(message.extensionId)
        ) {
          socket.destroy();
          return;
        }
        client.origin = message.extensionId;
        return;
      }
      const id = typeof message.id === "string" ? message.id : "";
      try {
        if (
          typeof message.profileId !== "string" ||
          !/^[0-9a-f-]{36}$/.test(message.profileId)
        )
          throw new Error("INVALID_PROFILE");
        if (message.type === "connect") {
          if (authenticated(message, client.origin)) {
            client.profileId = message.profileId;
            send({ id, ok: true, token: message.token });
            return;
          }
          if (approving.has(message.profileId))
            throw new Error("APPROVAL_PENDING");
          approving.add(message.profileId);
          const epoch = approvalEpoch.get(message.profileId) || 0;
          try {
            const profileName =
              typeof message.profileName === "string"
                ? message.profileName.slice(0, 80)
                : "Chrome profile";
            const approved = await requestApproval({
              extensionId: client.origin,
              profileId: message.profileId,
              profileName,
            });
            if (
              !approved ||
              closed ||
              socket.destroyed ||
              (approvalEpoch.get(message.profileId) || 0) !== epoch
            )
              throw new Error("APPROVAL_REQUIRED");
            const token = randomBytes(32).toString("hex");
            await mutate(async () => {
              if (
                closed ||
                socket.destroyed ||
                (approvalEpoch.get(message.profileId) || 0) !== epoch
              )
                throw new Error("APPROVAL_REQUIRED");
              approvals[message.profileId] = {
                extensionId: client.origin,
                profileName,
                tokenHash: hash(token),
              };
              await persist();
            });
            client.profileId = message.profileId;
            send({ id, ok: true, token });
            onStatus();
          } finally {
            approving.delete(message.profileId);
          }
          return;
        }
        if (!authenticated(message, client.origin)) throw new Error("REVOKED");
        client.profileId = message.profileId;
        if (message.type === "selectionResult") {
          const pending = selections.get(message.selectionId);
          if (pending?.client === client) {
            pending.resolve(message.reason || null);
            selections.delete(message.selectionId);
          }
          send({ id, ok: true });
          return;
        }
        if (message.type === "status") {
          send({ id, ok: true });
          return;
        }
        if (message.type === "revoke") {
          send({ id, ok: true });
          await revoke(message.profileId);
          return;
        }
        if (message.type === "capture") {
          const capture = validateCapture(message);
          // Serialize acceptance with revocation so a queued capture cannot cross a revoke.
          await mutate(async () => {
            if (!authenticated(message, client.origin) || closed)
              throw new Error("REVOKED");
            await acceptCapture(capture);
          });
          send({ id, ok: true });
          return;
        }
        throw new Error("UNSUPPORTED_REQUEST");
      } catch (error) {
        const allowed = new Set([
          "APPROVAL_REQUIRED",
          "APPROVAL_PENDING",
          "REVOKED",
          "INVALID_PROFILE",
          "INVALID_CAPTURE",
          "INVALID_METADATA",
          "UNSUPPORTED_REQUEST",
        ]);
        send({
          id,
          ok: false,
          reason: allowed.has(error.message) ? error.message : "CAPTURE_FAILED",
        });
      }
    };
    readMessages(
      socket,
      (message) => {
        queue = queue
          .then(() => processMessage(message))
          .catch(() => socket.destroy());
      },
      () => socket.destroy(),
    );
  });
  // Never unlink a live instance's endpoint. Stale sockets are removed only after
  // a failed connection; Lane's single-instance lifecycle owns startup serialization.
  if (process.platform !== "win32") {
    const live = await new Promise((resolve) => {
      const probe = net.connect(endpoint);
      probe.on("connect", () => {
        probe.destroy();
        resolve(true);
      });
      probe.on("error", () => resolve(false));
    });
    if (live) throw new Error("CHROME_BRIDGE_ALREADY_RUNNING");
    await unlink(endpoint).catch((error) => {
      if (error.code !== "ENOENT") throw error;
    });
  }
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(endpoint, resolve);
  });
  if (process.platform !== "win32") await chmod(endpoint, 0o600);
  await writeFile(discovery, JSON.stringify({ endpoint, secret }), {
    mode: 0o600,
  });
  await chmod(discovery, 0o600);
  return {
    listConnections() {
      return Object.entries(approvals).map(([profileId, value]) => ({
        profileId,
        profileName: value.profileName,
        extensionId: value.extensionId,
        connected: [...connections].some((c) => c.profileId === profileId),
      }));
    },
    revoke,
    async startSelection() {
      const active = [...connections].filter(
        (c) => c.profileId && approvals[c.profileId],
      );
      if (!active.length) throw new Error("CHROME_NOT_CONNECTED");
      const results = await Promise.all(
        active.map(
          (client) =>
            new Promise((resolve) => {
              const selectionId = randomBytes(16).toString("hex");
              const timer = setTimeout(() => {
                selections.delete(selectionId);
                resolve("CHROME_NOT_RESPONDING");
              }, 10000);
              selections.set(selectionId, {
                client,
                resolve: (reason) => {
                  clearTimeout(timer);
                  resolve(reason);
                },
              });
              client.socket.write(
                encodeMessage({ type: "select", selectionId }),
              );
            }),
        ),
      );
      if (results.includes(null)) return;
      const allowed = new Set([
        "UNSUPPORTED_PAGE",
        "PERMISSION_REQUIRED",
        "CHROME_NOT_RESPONDING",
        "REVOKED",
      ]);
      throw new Error(
        results.find((reason) => allowed.has(reason)) || "CHROME_NOT_AVAILABLE",
      );
    },
    async close() {
      closed = true;
      for (const pending of selections.values())
        pending.resolve("CHROME_NOT_AVAILABLE");
      selections.clear();
      for (const client of connections) client.socket.destroy();
      await new Promise((resolve) => server.close(resolve));
      await unlink(discovery).catch(() => {});
      if (process.platform !== "win32") await unlink(endpoint).catch(() => {});
    },
  };
}
