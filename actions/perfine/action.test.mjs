// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import {
  mkdtempSync,
  rmSync,
  readFileSync,
  writeFileSync,
  cpSync,
  mkdirSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { executionPlan } from "./main.mjs";
import { upload } from "./upload.mjs";
const id = "01900000-0000-7000-8000-000000000001";
const sha = "a".repeat(40);
function temporary(t) {
  const dir = mkdtempSync(join(tmpdir(), "perfine-action-test-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}
const env = {
  INPUT_COMMAND: "measure",
  INPUT_EXPERIMENT: id,
  INPUT_CONFIG: "schema_version: 1\nexperiment: {}\n",
  GITHUB_EVENT_NAME: "workflow_dispatch",
  GITHUB_RUN_ID: "42",
  GITHUB_RUN_ATTEMPT: "2",
  GITHUB_WORKSPACE: "/work",
  OPENAI_API_KEY: "excluded",
  GH_TOKEN: "excluded",
  ACTIONS_ID_TOKEN_REQUEST_TOKEN: "excluded",
  HOME: "/private",
};
test("inline YAML is immutable and runner secrets/cached home do not enter CLI execution", (t) => {
  const dir = temporary(t);
  for (const values of [
    { GITHUB_RUN_ID: "" },
    { GITHUB_RUN_ATTEMPT: "0" },
    { GITHUB_RUN_ATTEMPT: "4294967296" },
  ])
    assert.throws(
      () => executionPlan({ ...env, ...values }, dir),
      /run ID and attempt/,
    );
  const plan = executionPlan(env, dir);
  assert.deepEqual(plan.args.slice(-3), ["measure", "--experiment", id]);
  assert.equal(plan.env.HOME, dir);
  assert.equal(plan.env.PERFINE_GITHUB_RUN_ID, "42");
  assert.equal(plan.env.PERFINE_GITHUB_RUN_ATTEMPT, "2");
  assert.equal(plan.env.GH_TOKEN, undefined);
  assert.equal(plan.env.OPENAI_API_KEY, undefined);
  assert.equal(plan.env.ACTIONS_ID_TOKEN_REQUEST_TOKEN, undefined);
  assert.equal(
    readFileSync(join(dir, "perfine.yaml"), "utf8"),
    env.INPUT_CONFIG,
  );
  assert.equal(plan.configHash.length, 64);
  for (const command of ["optimize", "resume", "publish", "sh"])
    assert.throws(() => executionPlan({ ...env, INPUT_COMMAND: command }, dir));
  assert.throws(
    () =>
      executionPlan(
        { ...env, INPUT_CONFIG: "agent: { auth_file: /secret }" },
        dir,
      ),
    /authentication/,
  );
  assert.throws(
    () => executionPlan({ ...env, GITHUB_EVENT_NAME: "push" }, dir),
    /manual/,
  );
});
test("built action runs from the distributed layout and invokes CLI with sanitized environment", (t) => {
  const dir = temporary(t);
  cpSync("action.cjs", join(dir, "action.cjs"));
  mkdirSync(join(dir, "runtime"));
  // This shell stub observes the process boundary; it is not native CLI acceptance.
  const script =
    '#!/bin/sh\n[ -z "$GH_TOKEN$OPENAI_API_KEY$ACTIONS_ID_TOKEN_REQUEST_TOKEN" ] || exit 42\nprintf "%s\\n" "$@"\n';
  writeFileSync(join(dir, "runtime/perfine"), script, { mode: 0o755 });
  writeFileSync(
    join(dir, "runtime/SHA256SUMS"),
    `${createHash("sha256").update(script).digest("hex")}  perfine\n`,
  );
  writeFileSync(
    join(dir, "platform.cjs"),
    'Object.defineProperty(process, "platform", { value: "linux" }); Object.defineProperty(process, "arch", { value: "x64" });',
  );
  const result = spawnSync(
    process.execPath,
    ["--require", join(dir, "platform.cjs"), join(dir, "action.cjs")],
    {
      env: { ...env, GITHUB_WORKSPACE: dir, RUNNER_TEMP: dir },
      encoding: "utf8",
    },
  );
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /measure\n--experiment/);
  writeFileSync(join(dir, "runtime/perfine"), "tampered");
  const bad = spawnSync(
    process.execPath,
    ["--require", join(dir, "platform.cjs"), join(dir, "action.cjs")],
    {
      env: { ...env, GITHUB_WORKSPACE: dir, RUNNER_TEMP: dir },
      encoding: "utf8",
    },
  );
  assert.equal(bad.status, 1);
  assert.match(bad.stderr, /checksum/);
});
function uploadFixture(t) {
  const dir = temporary(t);
  const bytes = Buffer.from("screened evidence");
  const hash = createHash("sha256").update(bytes).digest("hex");
  writeFileSync(join(dir, hash), bytes);
  const artifact = {
    name: "engine.json",
    sha256: hash,
    sizeBytes: String(bytes.length),
    secretsRemoved: true,
    containsSourceSnippets: false,
    kind: "ARTIFACT_KIND_ENGINE_EVIDENCE",
    completion: "COMPLETION_STATE_COMPLETE",
  };
  const envelope = {
    specification: {},
    manifest: {
      schemaVersion: 1,
      experimentId: id,
      sourceHeadSha: sha,
      completion: "COMPLETION_STATE_COMPLETE",
      artifacts: [artifact],
    },
  };
  writeFileSync(join(dir, "envelope.json"), JSON.stringify(envelope));
  const vars = {
    GITHUB_EVENT_NAME: "workflow_dispatch",
    GITHUB_REPOSITORY: "owner/repo",
    GITHUB_REPOSITORY_ID: "123",
    GITHUB_RUN_ID: "456",
    GITHUB_RUN_ATTEMPT: "1",
    GITHUB_SHA: sha,
    GITHUB_WORKFLOW_SHA: sha,
    GITHUB_WORKFLOW_REF:
      "owner/repo/.github/workflows/perfine.yml@refs/heads/main",
    INPUT_EXPERIMENT: id,
    INPUT_SPACE: id,
    INPUT_CONNECTION: id,
    "INPUT_REQUEST-ID": id,
    INPUT_ENVELOPE: join(dir, "envelope.json"),
    INPUT_ARTIFACTS: dir,
    ACTIONS_ID_TOKEN_REQUEST_URL:
      "https://run.actions.githubusercontent.com/token",
    ACTIONS_ID_TOKEN_REQUEST_TOKEN: "fixture-only",
    OPENAI_API_KEY: "must-not-send",
  };
  const claims = {
    repository: "owner/repo",
    repository_id: "123",
    run_id: "456",
    run_attempt: "1",
    sha,
    workflow_sha: sha,
    workflow_ref: vars.GITHUB_WORKFLOW_REF,
    event_name: "workflow_dispatch",
    aud: "https://perfine.api.delino.io",
    iss: "https://token.actions.githubusercontent.com",
    exp: Math.floor(Date.now() / 1000) + 600,
  };
  return { dir, vars, claims, artifact, envelope };
}
test("separate OIDC upload binds repository/run and never forwards credentials to signed storage", async (t) => {
  const { vars, claims, artifact } = uploadFixture(t);
  const calls = [];
  const fetcher = async (url, options) => {
    calls.push({ url: String(url), ...options });
    let data;
    if (String(url).includes("/token"))
      data = {
        value: `fixture.${Buffer.from(JSON.stringify(claims)).toString("base64url")}.signature`,
      };
    else if (String(url).endsWith("/BeginUpload"))
      data = {
        requestId: id,
        upload: { id },
        targets: [
          {
            artifactName: artifact.name,
            signedPutUrl: "https://bucket.example.invalid/object",
            requiredHeaders: [
              { name: "Content-Length", value: artifact.sizeBytes },
            ],
          },
        ],
      };
    else if (String(url).endsWith("/CompleteUpload"))
      data = {
        requestId: id,
        upload: { id, state: "UPLOAD_STATE_COMPLETE" },
        evidenceCompletion: "COMPLETION_STATE_COMPLETE",
      };
    else data = {};
    return { ok: true, json: async () => data };
  };
  await upload(vars, fetcher);
  const begin = JSON.parse(calls[1].body);
  assert.equal(begin.requestId, id);
  assert.equal(JSON.parse(calls[3].body).requestId, id);
  assert.equal(calls[2].headers["content-length"], artifact.sizeBytes);
  assert.equal(begin.githubExecution.githubRunId, "456");
  assert.equal(begin.githubExecution.sourceSha, sha);
  assert.match(calls[1].headers.Authorization, /^Actions /);
  assert.equal(calls[2].headers.Authorization, undefined);
  assert.ok(!JSON.stringify(calls).includes("must-not-send"));
  for (const invalid of [
    "request-1",
    `${id}:complete`,
    id.replace("-7000-", "-4000-"),
  ]) {
    await assert.rejects(
      upload({ ...vars, "INPUT_REQUEST-ID": invalid }, async () => {
        assert.fail("Invalid IDs must fail before obtaining credentials");
      }),
      /INPUT_REQUEST-ID/,
    );
  }
  for (const headers of [
    [{ name: "content-length", value: "1" }],
    [{ name: "Authorization", value: "forbidden" }],
    [
      { name: "Content-Length", value: artifact.sizeBytes },
      { name: "content-length", value: artifact.sizeBytes },
    ],
  ]) {
    await assert.rejects(
      upload(vars, async (url, options) => {
        assert.notEqual(
          options.method,
          "PUT",
          "Reject invalid signed headers before storage transfer",
        );
        const response = await fetcher(url, options);
        const data = await response.json();
        if (data.targets) data.targets[0].requiredHeaders = headers;
        return { ok: true, json: async () => data };
      }),
      /header|content-length/,
    );
  }
  claims.repository_id = "another";
  await assert.rejects(upload(vars, fetcher), /binding/);
});
test("unsafe or incomplete evidence fails without discarding local files or obtaining credentials", async (t) => {
  const { vars, envelope } = uploadFixture(t);
  envelope.manifest.artifacts[0].containsSourceSnippets = true;
  writeFileSync(vars.INPUT_ENVELOPE, JSON.stringify(envelope));
  let calls = 0;
  await assert.rejects(
    upload(vars, async () => {
      calls++;
    }),
    /screened/,
  );
  assert.equal(calls, 0);
  assert.ok(readFileSync(vars.INPUT_ENVELOPE).length);
});
test("bundle includes dependency license texts and action metadata", () => {
  assert.doesNotMatch(
    readFileSync("action.cjs", "utf8"),
    /\.pnpm-store|setup-pnpm\/node_modules|\/Users\//u,
  );
  assert.match(readFileSync("ACTION_NOTICES", "utf8"), /js-yaml/);
  assert.match(readFileSync("ACTION_NOTICES", "utf8"), /argparse/);
  assert.match(readFileSync("action.yml", "utf8"), /main: action.cjs/);
  const tracked = execFileSync("git", ["ls-files", "."], { encoding: "utf8" });
  assert.ok(!tracked.split("\n").some((p) => p.split("/").includes("dist")));
});

test("same-ID retry reconciles a committed upload after a lost response", async (t) => {
  const { vars, claims, artifact } = uploadFixture(t);
  let committed = false;
  let reconciliationState = "UPLOAD_STATE_COMPLETE";
  const methods = [];
  const fetcher = async (url, options) => {
    const method = new URL(url).pathname.split("/").at(-1);
    methods.push(method);
    let data;
    if (method === "token")
      data = {
        value: `fixture.${Buffer.from(JSON.stringify(claims)).toString("base64url")}.signature`,
      };
    else if (method === "BeginUpload")
      data = {
        requestId: id,
        upload: {
          id,
          state: committed ? "UPLOAD_STATE_COMPLETE" : "UPLOAD_STATE_PENDING",
        },
        targets: committed
          ? []
          : [
              {
                artifactName: artifact.name,
                signedPutUrl: "https://bucket.example.invalid/object",
              },
            ],
      };
    else if (method === "CompleteUpload") {
      assert.equal(
        committed,
        false,
        "must not repeat completion with empty receipts",
      );
      assert.equal(JSON.parse(options.body).receipts.length, 1);
      committed = true;
      throw new Error("lost completion response");
    } else if (method === "GetUpload")
      data = { requestId: id, upload: { id, state: reconciliationState } };
    else data = {};
    return { ok: true, json: async () => data };
  };
  await assert.rejects(upload(vars, fetcher), /Evidence transport failed/);
  methods.length = 0;
  const result = await upload(vars, fetcher);
  assert.equal(result.upload.state, "UPLOAD_STATE_COMPLETE");
  assert.deepEqual(methods, ["token", "BeginUpload", "GetUpload"]);
  reconciliationState = "UPLOAD_STATE_PENDING";
  await assert.rejects(upload(vars, fetcher), /reconciliation incomplete/);
});
