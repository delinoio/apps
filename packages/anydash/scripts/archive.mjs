// SPDX-License-Identifier: Apache-2.0
// Deterministic ustar + gzip writer for CLI archives. Entries are sorted, owned
// by 0:0 with a zero timestamp, and executables keep mode 0755, so rebuilding
// the same inputs on any host produces identical archive bytes.
import { lstatSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join, posix } from "node:path";
import { gzipSync } from "node:zlib";

function field(buffer, offset, length, value) {
  buffer.write(value, offset, length, "utf8");
}
function octal(buffer, offset, length, value) {
  field(
    buffer,
    offset,
    length,
    `${value.toString(8).padStart(length - 1, "0")}\0`,
  );
}

function splitName(name) {
  if (Buffer.byteLength(name) <= 100) return { name, prefix: "" };
  for (
    let index = name.indexOf("/");
    index !== -1;
    index = name.indexOf("/", index + 1)
  ) {
    const prefix = name.slice(0, index);
    const rest = name.slice(index + 1);
    if (Buffer.byteLength(prefix) <= 155 && Buffer.byteLength(rest) <= 100)
      return { name: rest, prefix };
  }
  throw new Error(`archive path is too long: ${name}`);
}

function header(name, { size, mode, directory }) {
  const block = Buffer.alloc(512, 0);
  const parts = splitName(name);
  field(block, 0, 100, parts.name);
  octal(block, 100, 8, mode);
  octal(block, 108, 8, 0);
  octal(block, 116, 8, 0);
  octal(block, 124, 12, size);
  octal(block, 136, 12, 0);
  block.fill(" ", 148, 156);
  field(block, 156, 1, directory ? "5" : "0");
  field(block, 257, 6, "ustar\0");
  field(block, 263, 2, "00");
  field(block, 265, 32, "root");
  field(block, 297, 32, "root");
  field(block, 345, 155, parts.prefix);
  let sum = 0;
  for (const byte of block) sum += byte;
  field(block, 148, 8, `${sum.toString(8).padStart(6, "0")}\0 `);
  return block;
}

/** Lists a directory tree as sorted archive entries rooted at `name`. */
export function entries(directory, name, executables = new Set()) {
  const result = [{ name: `${name}/`, directory: true }];
  for (const child of readdirSync(directory).sort()) {
    const path = join(directory, child);
    const archived = posix.join(name, child);
    const stat = lstatSync(path);
    if (stat.isDirectory())
      result.push(...entries(path, archived, executables));
    else if (stat.isFile())
      result.push({
        name: archived,
        bytes: readFileSync(path),
        executable: executables.has(archived),
      });
    else throw new Error(`archives contain regular files only: ${archived}`);
  }
  return result;
}

export function tarGzip(list) {
  const blocks = [];
  for (const entry of list) {
    if (entry.directory) {
      blocks.push(
        header(entry.name, { size: 0, mode: 0o755, directory: true }),
      );
      continue;
    }
    blocks.push(
      header(entry.name, {
        size: entry.bytes.length,
        mode: entry.executable ? 0o755 : 0o644,
      }),
      entry.bytes,
      Buffer.alloc((512 - (entry.bytes.length % 512)) % 512, 0),
    );
  }
  blocks.push(Buffer.alloc(1024, 0));
  // Node's gzip header carries no timestamp or host name.
  return gzipSync(Buffer.concat(blocks), { level: 9 });
}

/** Writes `<directory>` as a deterministic archive with a top-level `<name>/`. */
export function writeArchive(directory, name, destination, executables) {
  const marked = new Set(
    [...executables].map((path) => posix.join(name, path)),
  );
  writeFileSync(destination, tarGzip(entries(directory, name, marked)));
}
