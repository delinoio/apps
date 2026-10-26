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
  const stop = () => socket?.destroy();
  input.on("error", stop);
  output.on("error", stop);
  try {
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
  } finally {
    // Chrome leaves stdin open while its native port exists. Release both stdio
    // ends when Lane disconnects so Chrome drops that port and can reconnect to
    // a later Lane instance. Flush the final framed response before ending stdout.
    socket?.destroy();
    input.destroy();
    if (!output.destroyed && !output.writableFinished) {
      await new Promise((resolve) => output.end(resolve));
    }
  }
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
