// SPDX-License-Identifier: Apache-2.0
import { appendFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const Endpoint = "https://backboard.railway.com/graphql/v2";
export const DeploymentStatus = Object.freeze({
  Success: "SUCCESS",
  Failed: "FAILED",
  Crashed: "CRASHED",
  Removed: "REMOVED",
  Skipped: "SKIPPED",
});
const terminalFailures = new Set([
  DeploymentStatus.Failed,
  DeploymentStatus.Crashed,
  DeploymentStatus.Removed,
  DeploymentStatus.Skipped,
]);
const identifier =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/u;
const image =
  /^ghcr\.io\/[a-z0-9][a-z0-9._-]*(?:\/[a-z0-9][a-z0-9._-]*)+:[A-Za-z0-9_][A-Za-z0-9._-]{0,127}$/u;

function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}

export function validateInput({
  token,
  environmentId,
  serviceId,
  image: reference,
  preDeployCommand,
}) {
  requireValue(
    typeof token === "string" && token.length > 0,
    "Railway token is required",
  );
  requireValue(identifier.test(environmentId ?? ""), "Invalid environment ID");
  requireValue(identifier.test(serviceId ?? ""), "Invalid service ID");
  requireValue(image.test(reference ?? ""), "Invalid GHCR image reference");
  requireValue(
    preDeployCommand === undefined ||
      (Array.isArray(preDeployCommand) &&
        preDeployCommand.every(
          (command) => typeof command === "string" && command.length > 0,
        )),
    "Invalid pre-deploy command",
  );
}

async function request({ token, fetcher }, query, variables) {
  const response = await fetcher(Endpoint, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      // Project tokens use a dedicated header; bearer tokens are for
      // account and workspace tokens.
      "project-access-token": token,
    },
    body: JSON.stringify({ query, variables }),
  });
  requireValue(response.ok, `Railway API returned HTTP ${response.status}`);
  const body = await response.json();
  // Error messages can echo request values, so report only a bounded count.
  requireValue(
    !body.errors?.length,
    `Railway API rejected the request (${body.errors?.length} errors)`,
  );
  return body.data;
}

const setSource = `mutation($serviceId: String!, $environmentId: String!, $input: ServiceInstanceUpdateInput!) {
  serviceInstanceUpdate(serviceId: $serviceId, environmentId: $environmentId, input: $input)
}`;
// serviceInstanceUpdate only stages the new source; this call starts the deployment.
const deploy = `mutation($serviceId: String!, $environmentId: String!) {
  serviceInstanceDeployV2(serviceId: $serviceId, environmentId: $environmentId)
}`;
const status = `query($id: String!) { deployment(id: $id) { status } }`;

export async function deployImage(
  options,
  {
    fetcher = fetch,
    sleep = (ms) => new Promise((done) => setTimeout(done, ms)),
    now = Date.now,
    log = (event) => console.log(JSON.stringify(event)),
    pollMilliseconds = 10_000,
    timeoutMilliseconds = 20 * 60_000,
  } = {},
) {
  validateInput(options);
  const client = { token: options.token, fetcher };
  const { serviceId, environmentId } = options;
  const input = { source: { image: options.image } };
  if (options.preDeployCommand)
    input.preDeployCommand = options.preDeployCommand;
  await request(client, setSource, { serviceId, environmentId, input });
  log({ event: "railway_source_staged", serviceId, image: options.image });
  const started = await request(client, deploy, { serviceId, environmentId });
  const id = started.serviceInstanceDeployV2;
  requireValue(typeof id === "string" && id, "Railway returned no deployment");
  log({ event: "railway_deployment_started", serviceId, deploymentId: id });
  const deadline = now() + timeoutMilliseconds;
  let previous;
  for (;;) {
    const current = (await request(client, status, { id })).deployment?.status;
    if (current !== previous) {
      log({
        event: "railway_deployment_status",
        deploymentId: id,
        status: current,
      });
      previous = current;
    }
    if (current === DeploymentStatus.Success) return { deploymentId: id };
    requireValue(
      !terminalFailures.has(current),
      `Railway deployment ended as ${current}`,
    );
    requireValue(now() < deadline, "Railway deployment timed out");
    await sleep(pollMilliseconds);
  }
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  const environment = process.env;
  const preDeployCommand = environment.RAILWAY_PRE_DEPLOY_COMMAND
    ? JSON.parse(environment.RAILWAY_PRE_DEPLOY_COMMAND)
    : undefined;
  const options = {
    token: environment.RAILWAY_PROJECT_TOKEN,
    environmentId: environment.RAILWAY_ENVIRONMENT_ID,
    serviceId: environment.RAILWAY_SERVICE_ID,
    image: environment.RELEASE_IMAGE,
    preDeployCommand,
  };
  const result = await deployImage(options);
  if (environment.GITHUB_STEP_SUMMARY)
    appendFileSync(
      environment.GITHUB_STEP_SUMMARY,
      `Image: ${options.image}\n\nRailway deployment: ${result.deploymentId}\n\nRailway reported SUCCESS. Domain routing, provider callbacks and live-account behavior are not established by this step.\n`,
    );
}
