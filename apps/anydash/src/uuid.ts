// SPDX-License-Identifier: Apache-2.0
/** Client-generated UUID v7 for idempotent `request_id` fields. */
export function uuidv7(now = Date.now()): string {
  const bytes = new Uint8Array(16);
  const crypto = (
    globalThis as {
      crypto?: { getRandomValues?: (array: Uint8Array) => Uint8Array };
    }
  ).crypto;
  if (crypto?.getRandomValues) crypto.getRandomValues(bytes);
  else
    for (let index = 0; index < 16; index++)
      bytes[index] = Math.floor(Math.random() * 256);
  let time = BigInt(now);
  for (let index = 5; index >= 0; index--) {
    bytes[index] = Number(time & 0xffn);
    time >>= 8n;
  }
  bytes[6] = (bytes[6] & 0x0f) | 0x70;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = Array.from(bytes, (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}
