// SPDX-License-Identifier: Apache-2.0
// This relay is launched by Chrome. It never imports or starts the Lane app.
import net from "node:net";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { encodeMessage, readMessages } from "./chrome-framing.mjs";
export async function runNativeRelay({
  dataRoot,
  origin,
  input = process.stdin,
  output = process.stdout,
}) {
  const match = /^chrome-extension:\/\/([a-p]{32})\/$/.exec(origin || "");
  if (!match) throw new Error("INVALID_ORIGIN");
  let socket;
  const failure = () => {
    output.write(encodeMessage({ ok: false, reason: "LANE_NOT_RUNNING" }));
  };
  try {
    const runtime = JSON.parse(
      await readFile(join(dataRoot, "realqa-chrome-runtime.json"), "utf8"),
    );
    socket = net.connect(runtime.endpoint);
    await new Promise((resolve, reject) => {
      socket.once("connect", resolve);
      socket.once("error", reject);
    });
    socket.write(
      encodeMessage({
        type: "relay",
        extensionId: match[1],
        secret: runtime.secret,
      }),
    );
  } catch {
    failure();
    return;
  }
  socket.on("error", () => {});
  readMessages(
    input,
    (message) => socket.write(encodeMessage(message)),
    () => socket.destroy(),
  );
  readMessages(
    socket,
    (message) => output.write(encodeMessage(message)),
    () => socket.destroy(),
    1024 * 1024,
  );
  input.on("end", () => socket.end());
  await new Promise((resolve) => socket.on("close", resolve));
}
if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  const index = process.argv.indexOf("--data-root");
  const origin = process.argv.find((arg) =>
    arg.startsWith("chrome-extension://"),
  );
  if (index < 0 || !process.argv[index + 1]) process.exitCode = 1;
  else
    await runNativeRelay({ dataRoot: process.argv[index + 1], origin }).catch(
      () => {
        process.exitCode = 1;
      },
    );
}
