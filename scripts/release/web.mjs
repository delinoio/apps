// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { appendFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

export const App = Object.freeze({
  Site: "delino-site",
  Docs: "delino-docs",
  GitTracker: "git-tracker",
  Lane: "lane",
  DevBase: "devbase",
  DeliTools: "delitools",
});

export async function prepareProject(env, fetcher = fetch) {
  assert.ok(
    Object.values(App).includes(env.WEB_APP),
    "Unknown web application",
  );
  assert.ok(env.CLOUDFLARE_API_TOKEN, "Missing CLOUDFLARE_API_TOKEN");
  assert.match(
    env.CLOUDFLARE_ACCOUNT_ID ?? "",
    /^[a-f0-9]{32}$/,
    "Invalid CLOUDFLARE_ACCOUNT_ID",
  );
  const project = env.CLOUDFLARE_PAGES_PROJECT || env.WEB_APP;
  assert.match(
    project,
    /^[a-z0-9][a-z0-9-]{0,57}$/,
    "Invalid Pages project name",
  );
  const endpoint = `https://api.cloudflare.com/client/v4/accounts/${env.CLOUDFLARE_ACCOUNT_ID}/pages/projects`;
  const headers = {
    Authorization: `Bearer ${env.CLOUDFLARE_API_TOKEN}`,
    "Content-Type": "application/json",
  };
  const request = async (url, options = {}) => {
    const response = await fetcher(url, {
      ...options,
      headers,
      signal: AbortSignal.timeout(30000),
    });
    if (response.status === 404 && !options.method) return null;
    // Report status only: account response bodies must not expose credentials.
    assert.ok(
      response.ok,
      `Pages API request failed (HTTP ${response.status})`,
    );
    const body = await response.json();
    assert.equal(body.success, true, "Pages API rejected the request");
    return body.result;
  };
  let result = await request(`${endpoint}/${encodeURIComponent(project)}`);
  if (!result) {
    result = await request(endpoint, {
      method: "POST",
      body: JSON.stringify({ name: project, production_branch: "main" }),
    });
  }
  assert.equal(result.name, project, "Pages project identity mismatch");
  assert.equal(
    result.production_branch,
    "main",
    "Pages production branch must be main",
  );
  assert.ok(
    !result.source,
    "Pages project must use direct upload without Git integration",
  );
  return project;
}

if (
  process.argv[1] &&
  pathToFileURL(resolve(process.argv[1])).href === import.meta.url
) {
  try {
    const project = await prepareProject(process.env);
    appendFileSync(process.env.GITHUB_OUTPUT, `project=${project}\n`);
    console.log(
      JSON.stringify({
        event: "release_web_project_ready",
        app: process.env.WEB_APP,
        project,
      }),
    );
  } catch (error) {
    console.error(
      JSON.stringify({
        event: "release_web_project_error",
        message: error.message,
      }),
    );
    process.exitCode = 1;
  }
}
