// SPDX-License-Identifier: Apache-2.0

// Demonstrates the DevBase end-user directory without an SDK package.
//
// A tenant Owner or Admin logs in and creates a project and a CRUD API key. The
// application then registers, looks up, updates, lists and deletes an end user with
// that key. The program removes everything it created before it exits.
//
// Environment:
//   DEVBASE_URL        API base URL, default http://127.0.0.1:46317
//   DEVBASE_TOKEN      Delino login token of a tenant Owner or Admin
//   DEVBASE_TENANT_ID  tenant that owns the example project

import { Code, ConnectError, createClient } from "@connectrpc/connect";
import { createConnectTransport } from "@connectrpc/connect-web";
import {
  ApiKeyPermission,
  DevBaseService,
} from "@repo/protos/devbase/v1/devbase_pb";
import { randomUUID } from "node:crypto";

const base = process.env.DEVBASE_URL || "http://127.0.0.1:46317";
const token = process.env.DEVBASE_TOKEN;
const tenantId = process.env.DEVBASE_TENANT_ID;
if (!token || !tenantId) {
  console.error("DEVBASE_TOKEN and DEVBASE_TENANT_ID are required");
  process.exit(1);
}

// Every request carries a bearer token. Mutations also carry an idempotency key, which
// lets a client repeat a request safely when a response is lost.
function clientFor(bearer: string) {
  return createClient(
    DevBaseService,
    createConnectTransport({
      baseUrl: base,
      interceptors: [
        (next) => (request) => {
          request.header.set("Authorization", `Bearer ${bearer}`);
          return next(request);
        },
      ],
    }),
  );
}

const developer = clientFor(token);
let projectId: string | undefined;
let projectVersion = 0n;
try {
  // 1. Developer: create a project and a key with a login token.
  const { project } = await developer.createProject({
    idempotencyKey: randomUUID(),
    tenantId,
    name: `example-${randomUUID().slice(0, 8)}`,
  });
  if (!project) throw new Error("project missing from response");
  projectId = project.id;
  projectVersion = project.version;
  console.log(`Created project ${project.id} (version ${project.version})`);

  const issued = await developer.issueApiKey({
    idempotencyKey: randomUUID(),
    projectId: project.id,
    permission: ApiKeyPermission.CRUD,
  });
  // The secret is shown once. Store it in your application's secret store; never print it.
  const application = clientFor(issued.secret);
  console.log(`Issued CRUD key ${issued.apiKey?.id}`);

  // 2. Application: manage end users with the project key.
  const { endUser } = await application.createEndUser({
    idempotencyKey: randomUUID(),
    projectId: project.id,
    externalId: "user-1001",
    name: "Ada",
    email: "ada@example.com",
    metadata: { plan: "pro" },
  });
  if (!endUser) throw new Error("end user missing from response");
  console.log(`Registered end user ${endUser.id} for external ID user-1001`);

  // Pass the stable internal ID to other services. Look users up by either identifier.
  const found = await application.lookupEndUser({
    projectId: project.id,
    externalId: "user-1001",
  });
  if (found.endUser?.id !== endUser.id)
    throw new Error("lookup returned a different user");

  // Update with a field mask and the current version. A stale version fails with "aborted".
  const updated = await application.updateEndUser({
    idempotencyKey: randomUUID(),
    projectId: project.id,
    endUserId: endUser.id,
    name: "Ada Lovelace",
    updateMask: { paths: ["name"] },
    version: endUser.version,
  });
  console.log(`Updated end user to version ${updated.endUser?.version}`);

  // Lists use cursors: repeat with nextPageToken until it is empty.
  let total = 0;
  let pageToken = "";
  do {
    const page = await application.listEndUsers({
      projectId: project.id,
      pageSize: 50,
      pageToken,
    });
    total += page.endUsers.length;
    pageToken = page.nextPageToken;
  } while (pageToken);
  console.log(`Listed ${total} end user(s)`);

  await application.deleteEndUser({
    idempotencyKey: randomUUID(),
    projectId: project.id,
    endUserId: endUser.id,
    version: updated.endUser!.version,
  });
  console.log("Deleted the end user");

  // 3. Developer: rotate by revoking the key. Revocation takes effect immediately.
  await developer.revokeApiKey({
    idempotencyKey: randomUUID(),
    projectId: project.id,
    apiKeyId: issued.apiKey!.id,
  });
  try {
    await application.listEndUsers({ projectId: project.id });
    throw new Error("a revoked key was still accepted");
  } catch (error) {
    if (!(error instanceof ConnectError) || error.code !== Code.Unauthenticated)
      throw error;
  }
  console.log("Revoked the key; it is rejected as unauthenticated");
} catch (error) {
  console.error(
    "DevBase example failed:",
    error instanceof Error ? error.message : error,
  );
  process.exitCode = 1;
} finally {
  if (projectId) {
    // Deleting the project removes its users and keys.
    await developer
      .deleteProject({
        idempotencyKey: randomUUID(),
        projectId,
        version: projectVersion,
      })
      .catch(() =>
        console.error(
          `Cleanup failed; delete the example project manually: ${projectId}`,
        ),
      );
  }
}
