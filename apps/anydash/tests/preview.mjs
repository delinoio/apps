// SPDX-License-Identifier: Apache-2.0
// Serves the built web host with the in-memory preview host for browser tests.
import { build } from "esbuild";
import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const dist = join(root, "dist");
const port = Number(process.env.ANYDASH_PREVIEW_PORT || 46410);
const bundle = await build({
  absWorkingDir: root,
  entryPoints: ["tests/preview-host.ts"],
  bundle: true,
  format: "esm",
  platform: "browser",
  target: "es2022",
  write: false,
  external: ["../dist/runtime/*"],
  define: { __ANYDASH_VERSION__: JSON.stringify("0.1.0-preview") },
});
const hostSource = Buffer.from(bundle.outputFiles[0].contents)
  .toString()
  .replace(/"\.\.\/dist\/runtime\//g, '"/runtime/');
const types = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript",
  ".css": "text/css",
  ".svg": "image/svg+xml",
  ".wasm": "application/wasm",
  ".bundle": "application/octet-stream",
};
const server = createServer(async (request, response) => {
  const url = new URL(request.url, `http://127.0.0.1:${port}`);
  if (url.pathname === "/preview-host.js") {
    response.writeHead(200, { "content-type": types[".js"] });
    response.end(hostSource);
    return;
  }
  const file = normalize(join(dist, url.pathname));
  try {
    if (!file.startsWith(dist) || !extname(file)) throw new Error("spa");
    const body = await readFile(file);
    response.writeHead(200, {
      "content-type": types[extname(file)] ?? "application/octet-stream",
    });
    response.end(body);
  } catch {
    // Single-page fallback, as Cloudflare Pages serves it.
    const html = (await readFile(join(dist, "index.html"), "utf8")).replace(
      "/host.js",
      "/preview-host.js",
    );
    response.writeHead(200, { "content-type": types[".html"] });
    response.end(html);
  }
});
server.listen(port, "127.0.0.1", () =>
  console.log(
    JSON.stringify({
      event: "anydash_preview_ready",
      url: `http://127.0.0.1:${port}`,
    }),
  ),
);
