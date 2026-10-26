// SPDX-License-Identifier: Apache-2.0
import { createReadStream } from "node:fs";
import { mkdir, open, symlink, realpath } from "node:fs/promises";
import { dirname, join, posix, sep } from "node:path";
import { createGunzip } from "node:zlib";

const invalid = () => new Error("The update archive is invalid or unsafe.");
function safeName(value, root) {
  if (
    !value ||
    value.includes("\\") ||
    /[\x00-\x1f]/.test(value) ||
    value.startsWith("/")
  )
    throw invalid();
  const parts = value.replace(/\/$/, "").split("/");
  if (
    parts.some(
      (part) => !part || part === "." || part === ".." || part.includes(":"),
    ) ||
    parts[0] !== root
  )
    throw invalid();
  return parts.join("/");
}
function paxFields(bytes) {
  const fields = {};
  let offset = 0;
  while (offset < bytes.length) {
    const space = bytes.indexOf(32, offset);
    const length = Number(bytes.subarray(offset, space).toString());
    if (
      space < offset ||
      !Number.isSafeInteger(length) ||
      length <= space - offset + 1 ||
      offset + length > bytes.length ||
      bytes[offset + length - 1] !== 10
    )
      throw invalid();
    const field = bytes
      .subarray(space + 1, offset + length - 1)
      .toString("utf8");
    const equals = field.indexOf("=");
    if (equals < 1) throw invalid();
    const key = field.slice(0, equals);
    // Sparse files have a different payload interpretation. Desktop packages
    // never need them, so reject rather than extracting ambiguous content.
    if (/sparse/i.test(key)) throw invalid();
    fields[key] = field.slice(equals + 1);
    offset += length;
  }
  return fields;
}

// Extract regular files first and create symlinks last. Archive paths can never
// write through a symlink; macOS framework links are retained within the bundle.
export async function extractUpdate(archive, destination, root) {
  const source = createReadStream(archive);
  const stream = source.pipe(createGunzip());
  source.on("error", (error) => stream.destroy(error));
  const iterator = stream[Symbol.asyncIterator]();
  let buffer = Buffer.alloc(0),
    total = 0;
  async function take(size) {
    while (buffer.length < size) {
      const next = await iterator.next();
      if (next.done) throw invalid();
      total += next.value.length;
      if (total > 4 * 1024 ** 3) throw invalid();
      buffer = Buffer.concat([buffer, next.value]);
    }
    const bytes = buffer.subarray(0, size);
    buffer = buffer.subarray(size);
    return bytes;
  }
  let extended = {},
    global = {},
    ended = false;
  const names = new Set(),
    links = [];
  try {
    for (;;) {
      const header = await take(512);
      if (header.every((byte) => byte === 0)) {
        if (!(await take(512)).every((byte) => byte === 0)) throw invalid();
        ended = true;
        break;
      }
      const field = (start, length) =>
        header
          .subarray(start, start + length)
          .toString("utf8")
          .replace(/\0.*$/s, "");
      const octal = (start, length) => {
        const text = field(start, length).trim();
        if (!/^[0-7]+$/.test(text)) throw invalid();
        return parseInt(text, 8);
      };
      const sum = header.reduce(
        (sum, byte, i) => sum + (i >= 148 && i < 156 ? 32 : byte),
        0,
      );
      if (sum !== octal(148, 8) || field(257, 5) !== "ustar") throw invalid();
      const type = field(156, 1),
        headerSize = octal(124, 12);
      if (type === "x" || type === "g") {
        if (headerSize > 1024 * 1024) throw invalid();
        const fields = paxFields(await take(headerSize));
        await take((512 - (headerSize % 512)) % 512);
        if (type === "g") global = { ...global, ...fields };
        else extended = { ...extended, ...fields };
        continue;
      }
      const attributes = { ...global, ...extended };
      extended = {};
      const size =
        attributes.size === undefined ? headerSize : Number(attributes.size);
      if (
        !Number.isSafeInteger(size) ||
        size < 0 ||
        size > 2 * 1024 ** 3 ||
        !["", "0", "5", "2"].includes(type)
      )
        throw invalid();
      const prefix = field(345, 155);
      const rawName =
        attributes.path ?? `${prefix ? `${prefix}/` : ""}${field(0, 100)}`;
      // BSD tar may add AppleDouble metadata next to the top-level bundle.
      // These records are not application files; validate their owning path
      // and discard them instead of materializing metadata outside the bundle.
      const components = rawName.replace(/\/$/, "").split("/");
      if (components.at(-1).startsWith("._") && ["", "0"].includes(type)) {
        components[components.length - 1] = components.at(-1).slice(2);
        safeName(components.join("/"), root);
        let remaining = size;
        while (remaining) {
          const amount = Math.min(remaining, 65536);
          await take(amount);
          remaining -= amount;
        }
        await take((512 - (size % 512)) % 512);
        continue;
      }
      const name = safeName(rawName, root);
      if (names.has(name) || names.size > 100000) throw invalid();
      names.add(name);
      const path = join(destination, name);
      await mkdir(dirname(path), { recursive: true });
      if (type === "5") {
        if (size) throw invalid();
        await mkdir(path, { recursive: true });
      } else if (type === "2") {
        if (size || name === root) throw invalid();
        const target = attributes.linkpath ?? field(157, 100);
        if (
          !target ||
          target.includes("\\") ||
          target.startsWith("/") ||
          /[\x00-\x1f:]/.test(target)
        )
          throw invalid();
        safeName(
          posix.normalize(posix.join(posix.dirname(name), target)),
          root,
        );
        links.push({ path, target });
      } else {
        if (name === root) throw invalid();
        const file = await open(path, "wx", octal(100, 8) & 0o777);
        try {
          let remaining = size;
          while (remaining) {
            const bytes = await take(Math.min(remaining, 65536));
            await file.writeFile(bytes);
            remaining -= bytes.length;
          }
        } finally {
          await file.close();
        }
      }
      await take((512 - (size % 512)) % 512);
    }
    if (buffer.some((byte) => byte !== 0)) throw invalid();
    for await (const bytes of iterator) {
      total += bytes.length;
      if (total > 4 * 1024 ** 3 || bytes.some((byte) => byte !== 0))
        throw invalid();
    }
    const actualRoot = await realpath(join(destination, root));
    for (const { path, target } of links) await symlink(target, path);
    for (const { path } of links) {
      const actual = await realpath(path);
      if (!actual.startsWith(actualRoot + sep)) throw invalid();
    }
    if (!ended || !names.size) throw invalid();
    return join(destination, root);
  } finally {
    source.destroy();
    stream.destroy();
  }
}
