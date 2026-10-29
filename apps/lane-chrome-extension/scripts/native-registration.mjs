// SPDX-License-Identifier: Apache-2.0
import { mkdir, writeFile, readFile, unlink } from "node:fs/promises";
import { join, isAbsolute } from "node:path";
import { homedir } from "node:os";
import { execFileSync } from "node:child_process";
export const HOST = "io.delino.lane.realqa";
const registry = `HKCU\\Software\\Google\\Chrome\\NativeMessagingHosts\\${HOST}`;
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
export async function installNativeMessaging(options) {
  const {
    extensionIds,
    relayExecutable,
    platform = process.platform,
  } = options;
  if (
    !Array.isArray(extensionIds) ||
    !extensionIds.length ||
    extensionIds.some((id) => !/^[a-p]{32}$/.test(id))
  )
    throw new Error("REAL_EXTENSION_IDS_REQUIRED");
  if (!relayExecutable || !isAbsolute(relayExecutable))
    throw new Error("ABSOLUTE_RELAY_EXECUTABLE_REQUIRED");
  const path = location(options);
  const { dirname } = await import("node:path");
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
    execFileSync(
      "reg.exe",
      ["ADD", registry, "/ve", "/t", "REG_SZ", "/d", path, "/f"],
      { stdio: "pipe", windowsHide: true },
    );
  return path;
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
    const output = execFileSync("reg.exe", ["QUERY", registry, "/ve"], {
      encoding: "utf8",
      windowsHide: true,
    });
    if (output.match(/\s+REG_SZ\s+([^\r\n]+)/)?.[1].trim() !== path)
      throw new Error("REGISTRATION_OWNER_CHANGED");
    execFileSync("reg.exe", ["DELETE", registry, "/f"], {
      stdio: "pipe",
      windowsHide: true,
    });
  }
  await unlink(path);
}
