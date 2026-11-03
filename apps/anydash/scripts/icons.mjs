// SPDX-License-Identifier: Apache-2.0
// Renders the AnyDash mark (the web favicon's three tiles) as PNG and ICO at
// build time, so no generated image binaries are committed.
import { deflateSync } from "node:zlib";

const crcTable = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
const crc = (buffer) => {
  let c = 0xffffffff;
  for (const byte of buffer) c = crcTable[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};
const chunk = (type, data) => {
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const sum = Buffer.alloc(4);
  sum.writeUInt32BE(crc(body));
  return Buffer.concat([length, body, sum]);
};

/** RGBA pixels of the mark; `template` draws black tiles for macOS menu bars. */
export function markPixels(size, { template = false } = {}) {
  const pixels = Buffer.alloc(size * size * 4);
  const s = size / 64;
  const inRounded = (x, y, left, top, width, height, radius) => {
    const cx = Math.min(Math.max(x, left + radius), left + width - radius);
    const cy = Math.min(Math.max(y, top + radius), top + height - radius);
    return (
      (x - cx) ** 2 + (y - cy) ** 2 <= radius ** 2 &&
      x >= left &&
      x <= left + width &&
      y >= top &&
      y <= top + height
    );
  };
  const tiles = [
    [12, 12, 18, 18, [255, 255, 255]],
    [34, 12, 18, 18, [152, 184, 255]],
    [12, 34, 40, 18, [255, 255, 255]],
  ];
  for (let y = 0; y < size; y++)
    for (let x = 0; x < size; x++) {
      const px = (x + 0.5) / s;
      const py = (y + 0.5) / s;
      let color;
      if (template) {
        if (tiles.some(([l, t, w, h]) => inRounded(px, py, l, t, w, h, 4)))
          color = [0, 0, 0, 255];
      } else if (inRounded(px, py, 0, 0, 64, 64, 14)) {
        color = [32, 33, 36, 255];
        for (const [l, t, w, h, rgb] of tiles)
          if (inRounded(px, py, l, t, w, h, 4)) color = [...rgb, 255];
      }
      if (color)
        color.forEach(
          (value, index) => (pixels[(y * size + x) * 4 + index] = value),
        );
    }
  return pixels;
}

export function png(size, options) {
  const pixels = markPixels(size, options);
  const rows = Buffer.alloc(size * (size * 4 + 1));
  for (let y = 0; y < size; y++)
    pixels.copy(rows, y * (size * 4 + 1) + 1, y * size * 4, (y + 1) * size * 4);
  const header = Buffer.alloc(13);
  header.writeUInt32BE(size, 0);
  header.writeUInt32BE(size, 4);
  header.set([8, 6, 0, 0, 0], 8);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(rows)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

/** ICO with embedded PNG images. */
export function ico(sizes = [16, 32, 48, 256]) {
  const images = sizes.map((size) => png(size));
  const header = Buffer.alloc(6 + sizes.length * 16);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(sizes.length, 4);
  let offset = header.length;
  sizes.forEach((size, index) => {
    const entry = 6 + index * 16;
    header[entry] = size >= 256 ? 0 : size;
    header[entry + 1] = size >= 256 ? 0 : size;
    header.writeUInt16LE(1, entry + 4);
    header.writeUInt16LE(32, entry + 6);
    header.writeUInt32LE(images[index].length, entry + 8);
    header.writeUInt32LE(offset, entry + 12);
    offset += images[index].length;
  });
  return Buffer.concat([header, ...images]);
}
