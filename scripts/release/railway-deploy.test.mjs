// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import test from "node:test";
import { deployImage, validateInput } from "./railway-deploy.mjs";

const options = {
  token: "project-token",
  environmentId: "11111111-1111-4111-8111-111111111111",
  serviceId: "22222222-2222-4222-8222-222222222222",
  image: "ghcr.io/delinoio/delcore-api:1.2.3",
  preDeployCommand: ["/delcore-api -migrate"],
};

function fakeRailway(statuses) {
  const calls = [];
  const fetcher = async (url, init) => {
    const body = JSON.parse(init.body);
    calls.push({ url, headers: init.headers, ...body });
    const data = body.query.includes("serviceInstanceUpdate")
      ? { serviceInstanceUpdate: true }
      : body.query.includes("serviceInstanceDeployV2")
        ? { serviceInstanceDeployV2: "deployment-1" }
        : { deployment: { status: statuses.shift() } };
    return { ok: true, status: 200, json: async () => ({ data }) };
  };
  return { calls, fetcher };
}
const quiet = { sleep: async () => {}, log: () => {} };

test("deployment stages the image and migration, starts it and waits for success", async () => {
  const { calls, fetcher } = fakeRailway(["BUILDING", "DEPLOYING", "SUCCESS"]);
  const result = await deployImage(options, { ...quiet, fetcher });
  assert.deepEqual(result, { deploymentId: "deployment-1" });
  assert.deepEqual(calls[0].variables.input, {
    source: { image: options.image },
    preDeployCommand: options.preDeployCommand,
  });
  assert.match(calls[1].query, /serviceInstanceDeployV2/u);
  assert.equal(calls.length, 5);
  for (const call of calls) {
    assert.equal(call.headers["project-access-token"], options.token);
    assert.equal(call.headers.authorization, undefined);
    assert.equal(call.url, "https://backboard.railway.com/graphql/v2");
  }
});

test("an omitted pre-deploy command leaves the configured command untouched", async () => {
  const { calls, fetcher } = fakeRailway(["SUCCESS"]);
  await deployImage(
    { ...options, preDeployCommand: undefined },
    { ...quiet, fetcher },
  );
  assert.deepEqual(calls[0].variables.input, {
    source: { image: options.image },
  });
});

test("failed deployments reject and never expose the token", async () => {
  for (const status of ["FAILED", "CRASHED", "REMOVED", "SKIPPED"]) {
    const { fetcher } = fakeRailway([status]);
    await assert.rejects(
      deployImage(options, { ...quiet, fetcher }),
      (error) =>
        error.message.includes(status) &&
        !error.message.includes(options.token),
    );
  }
});

test("API errors and timeouts reject", async () => {
  const failing = async () => ({
    ok: true,
    status: 200,
    json: async () => ({ errors: [{ message: options.token }] }),
  });
  await assert.rejects(
    deployImage(options, { ...quiet, fetcher: failing }),
    (error) => !error.message.includes(options.token),
  );
  const { fetcher } = fakeRailway(Array(10).fill("DEPLOYING"));
  let clock = 0;
  await assert.rejects(
    deployImage(options, {
      ...quiet,
      fetcher,
      now: () => (clock += 600_000),
      timeoutMilliseconds: 1_000_000,
    }),
    /timed out/u,
  );
});

test("input validation rejects malformed identifiers, images and commands", () => {
  validateInput(options);
  for (const change of [
    { token: "" },
    { environmentId: "not-an-id" },
    { serviceId: "" },
    { image: "docker.io/delinoio/delcore-api:1.2.3" },
    { image: "ghcr.io/delinoio/delcore-api" },
    { preDeployCommand: "migrate" },
    { preDeployCommand: [""] },
  ]) {
    assert.throws(() => validateInput({ ...options, ...change }));
  }
});
