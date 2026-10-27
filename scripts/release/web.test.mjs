// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import test from "node:test";
import { App, prepareProject } from "./web.mjs";

const env = {
  WEB_APP: App.DevBase,
  CLOUDFLARE_API_TOKEN: "fixture-token",
  CLOUDFLARE_ACCOUNT_ID: "a".repeat(32),
};
const success = (result) => Response.json({ success: true, result });

test("Pages preparation reuses an existing direct-upload main project", async () => {
  const calls = [];
  assert.equal(
    await prepareProject(env, async (url, options) => {
      calls.push({ url, options });
      return success({ name: "devbase", production_branch: "main" });
    }),
    "devbase",
  );
  assert.equal(calls.length, 1);
  assert.equal(calls[0].options.method, undefined);
  assert.ok(calls[0].url.endsWith("/pages/projects/devbase"));
});

test("missing Pages projects are created with the selected name and main branch", async () => {
  const calls = [];
  const project = await prepareProject(
    { ...env, CLOUDFLARE_PAGES_PROJECT: "custom-devbase" },
    async (url, options) => {
      calls.push({ url, options });
      if (calls.length === 1) return new Response(null, { status: 404 });
      return success(JSON.parse(options.body));
    },
  );
  assert.equal(project, "custom-devbase");
  assert.equal(calls.length, 2);
  assert.equal(calls[1].options.method, "POST");
  assert.deepEqual(JSON.parse(calls[1].options.body), {
    name: project,
    production_branch: "main",
  });
});

for (const status of [401, 403, 429, 500]) {
  test(`Pages HTTP ${status} fails without creating a project or exposing the token`, async () => {
    let calls = 0;
    await assert.rejects(
      prepareProject(env, async () => {
        calls++;
        return new Response(env.CLOUDFLARE_API_TOKEN, { status });
      }),
      (error) => {
        assert.ok(error.message.includes(`HTTP ${status}`));
        assert.ok(!error.message.includes(env.CLOUDFLARE_API_TOKEN));
        return true;
      },
    );
    assert.equal(calls, 1);
  });
}

test("Pages preparation refuses conflicting production or automatic Git deployment", async () => {
  for (const result of [
    { name: "devbase", production_branch: "other" },
    { name: "devbase", production_branch: "main", source: { type: "github" } },
    { name: "another-project", production_branch: "main" },
  ]) {
    await assert.rejects(prepareProject(env, async () => success(result)));
  }
});

test("invalid project configuration fails before contacting Cloudflare", async () => {
  for (const override of [
    { WEB_APP: "unknown" },
    { CLOUDFLARE_API_TOKEN: "" },
    { CLOUDFLARE_ACCOUNT_ID: "invalid" },
    { CLOUDFLARE_PAGES_PROJECT: "invalid\nproject" },
  ]) {
    await assert.rejects(
      prepareProject({ ...env, ...override }, async () =>
        assert.fail("Unexpected API request"),
      ),
    );
  }
});
