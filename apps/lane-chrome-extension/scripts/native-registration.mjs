// SPDX-License-Identifier: Apache-2.0
import { mkdir, writeFile, readFile, unlink, access } from "node:fs/promises";
import { join, isAbsolute, dirname } from "node:path";
import { homedir } from "node:os";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
export const HOST = "io.delino.lane.realqa";
// Stable outcomes of the running host's registration check.
export const RegistrationResult = Object.freeze({
  Registered: "registered",
  Current: "current",
  Translocated: "translocated",
});
const registry = `HKCU\\Software\\Google\\Chrome\\NativeMessagingHosts\\${HOST}`;
const runRegistry = (options, args) =>
  (options.execFile || promisify(execFile))("reg.exe", args, {
    encoding: "utf8",
    windowsHide: true,
  });
function location({
  platform = process.platform,
  home = homedir(),
  manifestDirectory,
}) {
  if (!["darwin", "win32"].includes(platform))
    throw new Error("UNSUPPORTED_PLATFORM");
  if (platform === "win32" && !manifestDirectory)
    throw new Error("MANIFEST_DIRECTORY_REQUIRED");
  return join(
    manifestDirectory ||
      join(
        home,
        "Library/Application Support/Google/Chrome/NativeMessagingHosts",
      ),
    `${HOST}.json`,
  );
}
function validate({ extensionIds, relayExecutable }) {
  if (
    !Array.isArray(extensionIds) ||
    !extensionIds.length ||
    extensionIds.some((id) => !/^[a-p]{32}$/.test(id))
  )
    throw new Error("REAL_EXTENSION_IDS_REQUIRED");
  if (!relayExecutable || !isAbsolute(relayExecutable))
    throw new Error("ABSOLUTE_RELAY_EXECUTABLE_REQUIRED");
}
async function registeredManifest(options, path) {
  if ((options.platform || process.platform) !== "win32") return path;
  try {
    const { stdout } = await runRegistry(options, ["QUERY", registry, "/ve"]);
    return stdout.match(/\s+REG_SZ\s+([^\r\n]+)/)?.[1].trim();
  } catch {
    // `reg QUERY` exits nonzero when the key is absent.
    return undefined;
  }
}
export async function installNativeMessaging(options) {
  validate(options);
  const {
    extensionIds,
    relayExecutable,
    platform = process.platform,
  } = options;
  const path = location(options);
  await mkdir(dirname(path), { recursive: true, mode: 0o700 });
  await writeFile(
    path,
    JSON.stringify(
      {
        name: HOST,
        description: "RealQA connection to the running Lane application",
        path: relayExecutable,
        type: "stdio",
        allowed_origins: extensionIds.map((id) => `chrome-extension://${id}/`),
      },
      null,
      2,
    ) + "\n",
    { mode: 0o600 },
  );
  if (platform === "win32")
    await runRegistry(options, [
      "ADD",
      registry,
      "/ve",
      "/t",
      "REG_SZ",
      "/d",
      path,
      "/f",
    ]);
  return path;
}
// Release archives are portable and have no installer step, so the running Lane
// host keeps Chrome pointed at its own relay, as it does for its URL scheme. The
// most recently started installation owns the registration; every relay reaches
// the running host through the shared per-user data directory.
export async function ensureNativeMessaging(options) {
  validate(options);
  const platform = options.platform || process.platform;
  // macOS runs a quarantined app from a randomized read-only copy until the user
  // moves it. Registering that copy would leave Chrome a path that disappears.
  if (
    platform === "darwin" &&
    options.relayExecutable.includes("/AppTranslocation/")
  )
    return RegistrationResult.Translocated;
  await access(options.relayExecutable);
  const path = location(options);
  let manifest;
  try {
    manifest = JSON.parse(await readFile(path, "utf8"));
  } catch (error) {
    if (error.code !== "ENOENT" && !(error instanceof SyntaxError)) throw error;
  }
  const origins = options.extensionIds.map((id) => `chrome-extension://${id}/`);
  if (
    manifest?.name === HOST &&
    manifest.path === options.relayExecutable &&
    manifest.type === "stdio" &&
    JSON.stringify(manifest.allowed_origins) === JSON.stringify(origins) &&
    (await registeredManifest(options, path)) === path
  )
    return RegistrationResult.Current;
  await installNativeMessaging(options);
  return RegistrationResult.Registered;
}
export async function removeNativeMessaging(options) {
  const path = location(options);
  let manifest;
  try {
    manifest = JSON.parse(await readFile(path, "utf8"));
  } catch (error) {
    if (error.code === "ENOENT") return;
    throw error;
  }
  // A stale uninstaller must not unregister a newer Lane installation.
  if (manifest.name !== HOST || manifest.path !== options.relayExecutable)
    throw new Error("REGISTRATION_OWNER_CHANGED");
  if ((options.platform || process.platform) === "win32") {
    if ((await registeredManifest(options, path)) !== path)
      throw new Error("REGISTRATION_OWNER_CHANGED");
    await runRegistry(options, ["DELETE", registry, "/f"]);
  }
  await unlink(path);
}
