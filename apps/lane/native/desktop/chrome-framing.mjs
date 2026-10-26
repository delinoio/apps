// SPDX-License-Identifier: Apache-2.0
import { endianness } from "node:os";
const little = endianness() === "LE";
export const MAX_MESSAGE = 64 * 1024 * 1024;
export function encodeMessage(value) {
  const body = Buffer.from(JSON.stringify(value));
  if (body.length > MAX_MESSAGE) throw new Error("MESSAGE_TOO_LARGE");
  const size = Buffer.alloc(4);
  little ? size.writeUInt32LE(body.length) : size.writeUInt32BE(body.length);
  return Buffer.concat([size, body]);
}
export function readMessages(stream, onMessage, onError, max = MAX_MESSAGE) {
  let buffer = Buffer.alloc(0);
  const data = (chunk) => {
    try {
      buffer = Buffer.concat([buffer, chunk]);
      while (buffer.length >= 4) {
        const size = little ? buffer.readUInt32LE() : buffer.readUInt32BE();
        if (size === 0 || size > max) throw new Error("INVALID_FRAME");
        if (buffer.length < size + 4) break;
        const value = JSON.parse(buffer.subarray(4, size + 4).toString("utf8"));
        buffer = buffer.subarray(size + 4);
        onMessage(value);
      }
    } catch {
      stream.off("data", data);
      onError(new Error("INVALID_FRAME"));
    }
  };
  stream.on("data", data);
  stream.on("end", () => {
    if (buffer.length) onError(new Error("TRUNCATED_FRAME"));
  });
}
