// SPDX-License-Identifier: Apache-2.0
import { readFileSync, lstatSync } from "node:fs";
import { join } from "node:path";
import { createHash } from "node:crypto";
const endpoint = "https://perfine.api.delino.io";
const uuid =
  /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
function required(env, key, pattern) {
  if (!pattern.test(env[key] ?? "")) throw new Error(`Invalid ${key}`);
  return env[key];
}
export function binding(env) {
  if (env.GITHUB_EVENT_NAME !== "workflow_dispatch")
    throw new Error("Manual execution required");
  const repository = required(env, "GITHUB_REPOSITORY", /^[\w.-]+\/[\w.-]+$/);
  const workflow = required(
    env,
    "GITHUB_WORKFLOW_REF",
    /^[\w.-]+\/[\w.-]+\/.+@refs\/.+$/,
  );
  if (!workflow.startsWith(`${repository}/`))
    throw new Error("Workflow repository mismatch");
  return {
    repositoryId: required(env, "GITHUB_REPOSITORY_ID", /^[1-9]\d*$/),
    repository,
    githubRunId: required(env, "GITHUB_RUN_ID", /^[1-9]\d*$/),
    runAttempt: Number(required(env, "GITHUB_RUN_ATTEMPT", /^[1-9]\d*$/)),
    sourceSha: required(env, "GITHUB_SHA", /^[0-9a-f]{40}$/),
    workflowSha: required(env, "GITHUB_WORKFLOW_SHA", /^[0-9a-f]{40}$/),
    workflowPath: workflow.slice(repository.length + 1).split("@")[0],
  };
}
async function request(fetcher, url, options) {
  for (let attempt = 0; attempt < 2; attempt++) {
    let response;
    try {
      response = await fetcher(url, {
        ...options,
        redirect: "error",
        signal: AbortSignal.timeout(60000),
      });
    } catch {
      if (attempt === 0) continue;
      throw new Error("Evidence transport failed; local evidence retained");
    }
    if (attempt === 0 && (response.status === 429 || response.status >= 500))
      continue;
    if (!response.ok)
      throw new Error(
        `Evidence upload HTTP ${response.status}; local evidence retained`,
      );
    return response;
  }
}
export async function upload(env, fetcher = fetch) {
  const execution = binding(env);
  const experiment = required(env, "INPUT_EXPERIMENT", uuid);
  const space = required(env, "INPUT_SPACE", uuid);
  const connection = required(env, "INPUT_CONNECTION", uuid);
  const requestId = required(env, "INPUT_REQUEST-ID", uuid);
  const envelope = JSON.parse(readFileSync(env.INPUT_ENVELOPE, "utf8"));
  const manifest = envelope.manifest;
  if (
    !manifest ||
    manifest.schemaVersion !== 1 ||
    manifest.experimentId !== experiment ||
    manifest.sourceHeadSha !== execution.sourceSha ||
    manifest.completion !== "COMPLETION_STATE_COMPLETE"
  )
    throw new Error("Upload experiment/source/completion mismatch");
  if (!Array.isArray(manifest.artifacts) || !manifest.artifacts.length)
    throw new Error("Complete evidence artifacts required");
  const artifacts = new Map();
  for (const item of manifest.artifacts) {
    if (
      !/^[0-9a-f]{64}$/.test(item.sha256) ||
      !item.name ||
      artifacts.has(item.name) ||
      item.secretsRemoved !== true ||
      item.containsSourceSnippets !== false ||
      ![
        "ARTIFACT_KIND_ENGINE_EVIDENCE",
        "ARTIFACT_KIND_MEASUREMENTS",
        "ARTIFACT_KIND_PROFILE_SUMMARY",
        "ARTIFACT_KIND_CORRECTNESS_RESULTS",
        "ARTIFACT_KIND_RAW_PROFILE",
        "ARTIFACT_KIND_RAW_TEST_LOG",
      ].includes(item.kind) ||
      item.completion !== "COMPLETION_STATE_COMPLETE"
    )
      throw new Error(
        "Only explicitly screened, source-free unique artifacts can be uploaded",
      );
    const file = join(env.INPUT_ARTIFACTS, item.sha256);
    if (!lstatSync(file).isFile())
      throw new Error("Regular artifact file required");
    const bytes = readFileSync(file);
    if (hash(bytes) !== item.sha256 || String(bytes.length) !== item.sizeBytes)
      throw new Error("Artifact hash/size mismatch");
    artifacts.set(item.name, { item, bytes });
  }
  // No shell, candidate, agent, or publication process runs in this stage. The
  // server verifies JWT signature, current connection authority and run binding.
  const tokenURL = new URL(env.ACTIONS_ID_TOKEN_REQUEST_URL);
  if (
    tokenURL.protocol !== "https:" ||
    !tokenURL.hostname.endsWith(".actions.githubusercontent.com") ||
    tokenURL.username ||
    tokenURL.password
  )
    throw new Error("Invalid GitHub OIDC token endpoint");
  if (!env.ACTIONS_ID_TOKEN_REQUEST_TOKEN)
    throw new Error("Upload job requires id-token: write");
  tokenURL.searchParams.set("audience", endpoint);
  const token = await (
    await request(fetcher, tokenURL, {
      headers: {
        Authorization: `Bearer ${env.ACTIONS_ID_TOKEN_REQUEST_TOKEN}`,
      },
    })
  ).json();
  if (typeof token.value !== "string" || token.value.split(".").length !== 3)
    throw new Error("Invalid OIDC token response");
  const claims = JSON.parse(
    Buffer.from(token.value.split(".")[1], "base64url"),
  );
  for (const [key, expected] of Object.entries({
    repository: execution.repository,
    repository_id: execution.repositoryId,
    run_id: execution.githubRunId,
    run_attempt: String(execution.runAttempt),
    sha: execution.sourceSha,
    workflow_sha: execution.workflowSha,
    workflow_ref: env.GITHUB_WORKFLOW_REF,
    event_name: "workflow_dispatch",
    aud: endpoint,
    iss: "https://token.actions.githubusercontent.com",
  })) {
    if (claims[key] !== expected)
      throw new Error("OIDC repository/execution binding mismatch");
  }
  if (!Number.isFinite(claims.exp) || claims.exp <= Date.now() / 1000)
    throw new Error("Expired OIDC token");
  const rpc = async (method, body) =>
    (
      await request(
        fetcher,
        `${endpoint}/perfine.v1.EvidenceService/${method}`,
        {
          method: "POST",
          headers: {
            Authorization: `Actions ${token.value}`,
            "Content-Type": "application/json",
            "Connect-Protocol-Version": "1",
          },
          body: JSON.stringify(body),
        },
      )
    ).json();
  const { repositoryId, repository, ...githubExecution } = execution;
  const begun = await rpc("BeginUpload", {
    requestId,
    spaceId: space,
    connectionId: connection,
    experimentId: experiment,
    specification: envelope.specification,
    manifest,
    origin: "EXECUTION_ORIGIN_GITHUB_ACTIONS",
    githubExecution,
  });
  if (
    begun.requestId !== requestId ||
    !uuid.test(begun.upload?.id) ||
    !Array.isArray(begun.targets)
  )
    throw new Error("Invalid upload response");
  if (begun.upload.state === "UPLOAD_STATE_COMPLETE") {
    // A lost completion response can leave the original request committed. Do not
    // resend that request ID with different (empty) receipts; reconcile it instead.
    const reconciled = await rpc("GetUpload", {
      requestId,
      spaceId: space,
      uploadId: begun.upload.id,
    });
    if (
      reconciled.requestId !== requestId ||
      reconciled.upload?.id !== begun.upload.id ||
      reconciled.upload?.state !== "UPLOAD_STATE_COMPLETE"
    )
      throw new Error(
        "Upload reconciliation incomplete; local evidence retained",
      );
    console.log(
      JSON.stringify({
        event: "perfine_upload_complete",
        requestId,
        experimentId: experiment,
        reconciled: true,
      }),
    );
    return reconciled;
  }
  const receipts = [];
  const seen = new Set();
  for (const target of begun.targets) {
    const artifact = artifacts.get(target.artifactName);
    if (!artifact || seen.has(target.artifactName))
      throw new Error("Unexpected upload target");
    seen.add(target.artifactName);
    const url = new URL(target.signedPutUrl);
    if (url.protocol !== "https:" || url.username || url.password)
      throw new Error("HTTPS artifact target required");
    const headers = {};
    for (const header of target.requiredHeaders ?? []) {
      if (
        ![
          "content-type",
          "content-length",
          "content-md5",
          "x-amz-checksum-sha256",
        ].includes(header.name.toLowerCase())
      )
        throw new Error("Unsupported artifact header");
      const name = header.name.toLowerCase();
      if (Object.hasOwn(headers, name))
        throw new Error("Duplicate artifact header");
      if (
        name === "content-length" &&
        header.value !== String(artifact.bytes.length)
      )
        throw new Error("Artifact content-length mismatch");
      headers[name] = header.value;
    }
    await request(fetcher, url, {
      method: "PUT",
      headers,
      body: artifact.bytes,
    });
    receipts.push({
      artifactName: target.artifactName,
      sha256: artifact.item.sha256,
      sizeBytes: artifact.item.sizeBytes,
    });
  }
  const complete = await rpc("CompleteUpload", {
    requestId,
    spaceId: space,
    uploadId: begun.upload.id,
    receipts,
  });
  if (
    complete.requestId !== requestId ||
    complete.upload?.id !== begun.upload.id ||
    complete.upload?.state !== "UPLOAD_STATE_COMPLETE" ||
    complete.evidenceCompletion !== "COMPLETION_STATE_COMPLETE"
  )
    throw new Error(
      "Upload incomplete; local evidence retained for manual resend",
    );
  console.log(
    JSON.stringify({
      event: "perfine_upload_complete",
      requestId,
      experimentId: experiment,
    }),
  );
  return complete;
}
