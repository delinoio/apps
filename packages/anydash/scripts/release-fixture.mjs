// SPDX-License-Identifier: Apache-2.0
// A loopback stand-in for the GitHub release list and release downloads, so the
// install scripts run unchanged against local release directories.
import { readFileSync, existsSync } from "node:fs";
import { createServer } from "node:http";
import { basename, resolve } from "node:path";

/**
 * Serves `releases` as the release list and `<tag>/<file>` from `directories`
 * (a map of tag to local directory). `requests` records every requested path.
 */
export async function serveReleases({ releases, directories }) {
  const requests = [];
  const server = createServer((request, response) => {
    const path = decodeURIComponent(new URL(request.url, "http://x").pathname);
    requests.push(path);
    if (path === "/releases") {
      response.setHeader("Content-Type", "application/json");
      return response.end(JSON.stringify(releases));
    }
    const match = /^\/download\/([^/]+)\/([^/]+)$/.exec(path);
    const directory = match && directories[match[1]];
    const file = directory && resolve(directory, basename(match[2]));
    if (!file || !existsSync(file)) {
      response.writeHead(404);
      return response.end();
    }
    response.end(readFileSync(file));
  });
  await new Promise((done, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", done);
  });
  const origin = `http://127.0.0.1:${server.address().port}`;
  return {
    requests,
    env: {
      ANYDASH_RELEASES_URL: `${origin}/releases`,
      ANYDASH_DOWNLOAD_URL: `${origin}/download`,
    },
    close: () => new Promise((done) => server.close(done)),
  };
}

export const release = (tag, extra = {}) => ({
  tag_name: tag,
  draft: false,
  prerelease: false,
  ...extra,
});
