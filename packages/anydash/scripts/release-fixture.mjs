// SPDX-License-Identifier: Apache-2.0
// A loopback stand-in for the GitHub release list and release downloads, so the
// install scripts run unchanged against local release directories.
import { readFileSync, existsSync } from "node:fs";
import { createServer } from "node:http";
import { basename, resolve } from "node:path";

/**
 * Serves `releases` as the release list and `<tag>/<file>` from `directories`
 * (a map of tag to local directory). The list is paginated like the GitHub
 * API: `per_page` defaults to 30 and is capped at 100, and `page` defaults
 * to 1. `requests` records every requested path; `listPages` records the
 * requested release-list page numbers in order.
 */
export async function serveReleases({ releases, directories }) {
  const requests = [];
  const listPages = [];
  const server = createServer((request, response) => {
    const url = new URL(request.url, "http://x");
    const path = decodeURIComponent(url.pathname);
    requests.push(path);
    if (path === "/releases") {
      const perPage = Math.min(
        Number(url.searchParams.get("per_page") ?? 30),
        100,
      );
      const page = Number(url.searchParams.get("page") ?? 1);
      listPages.push(page);
      response.setHeader("Content-Type", "application/json");
      return response.end(
        JSON.stringify(releases.slice((page - 1) * perPage, page * perPage)),
      );
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
    listPages,
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
