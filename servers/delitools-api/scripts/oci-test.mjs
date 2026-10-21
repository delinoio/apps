// SPDX-License-Identifier: FSL-1.1-ALv2
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { setTimeout as delay } from "node:timers/promises";

const exec = promisify(execFile);
const root = fileURLToPath(new URL("../../../", import.meta.url));
const name = `delitools-oci-${process.pid}-${Date.now()}`;
const database = `${name}-db`;
const provider = `${name}-provider`;
const app = `${name}-app`;
const image = "delitools-api:validation";
const containers = [];
const directory = await mkdtemp(join(tmpdir(), "delitools-oci-"));
async function docker(...args) {
  const { stdout, stderr } = await exec("docker", args, {
    cwd: root,
    timeout: 120000,
    maxBuffer: 8 * 1024 * 1024,
  });
  return `${stdout}${stderr}`.trim();
}
async function rejected(args) {
  try {
    await docker(...args);
  } catch (error) {
    // A timeout or signal is not evidence that the server rejected startup.
    assert.equal(error.killed, false, "command must reject before its timeout");
    assert.equal(typeof error.code, "number");
    assert.notEqual(error.code, 0);
    return;
  }
  assert.fail("expected container command to reject");
}
async function until(label, probe, attempts = 60) {
  for (let count = 0; count < attempts; count++) {
    if (await probe()) return;
    await delay(1000);
  }
  throw new Error(`timed out: ${label}`);
}
const sql = (statement) =>
  docker(
    "exec",
    database,
    "psql",
    "-U",
    "postgres",
    "-d",
    "delitools",
    "-v",
    "ON_ERROR_STOP=1",
    "-Atc",
    statement,
  );
// The distroless image has no shell or HTTP client: probe through the fixture container, which
// shares the database container's network namespace.
const status = (url) =>
  docker(
    "exec",
    provider,
    "node",
    "--input-type=module",
    "-e",
    `const r = await fetch(${JSON.stringify(url)}); console.log(r.status);`,
  );
const metrics = () =>
  docker(
    "exec",
    provider,
    "node",
    "--input-type=module",
    "-e",
    `console.log(await (await fetch("http://127.0.0.1:46322/metrics")).text());`,
  );
try {
  console.log("Building DeliTools OCI image");
  await exec(
    "docker",
    ["build", "-f", "servers/delitools-api/Dockerfile", "-t", image, "."],
    { cwd: root, timeout: 45 * 60 * 1000, maxBuffer: 32 * 1024 * 1024 },
  );
  assert.equal(
    await docker("image", "inspect", "--format", "{{.Config.User}}", image),
    "65532:65532",
  );
  // Server and protocol license notices ship with the image.
  const licenses = join(directory, "licenses");
  const created = await docker("create", image);
  try {
    await docker("cp", `${created}:/licenses`, licenses);
  } finally {
    await docker("rm", "-f", created);
  }
  for (const file of [
    "servers/LICENSE",
    "servers/NOTICE",
    "protocols/LICENSE",
    "protocols/NOTICE",
  ])
    assert.ok((await readFile(join(licenses, file), "utf8")).length > 0, file);

  containers.push(database);
  await docker(
    "run",
    "-d",
    "--name",
    database,
    "-e",
    "POSTGRES_PASSWORD=fixture-only",
    "-e",
    "POSTGRES_DB=delitools",
    "postgres:17-bookworm",
  );
  await until("PostgreSQL startup", async () => {
    try {
      await docker(
        "exec",
        database,
        "pg_isready",
        "-h",
        "127.0.0.1",
        "-U",
        "postgres",
      );
      return true;
    } catch {
      return false;
    }
  });
  containers.push(provider);
  await docker(
    "run",
    "-d",
    "--name",
    provider,
    "--network",
    `container:${database}`,
    "--mount",
    `type=bind,src=${join(root, "servers/delitools-api/scripts/oci-provider.mjs")},dst=/provider.mjs,readonly`,
    "node:24-bookworm-slim",
    "node",
    "/provider.mjs",
  );
  await until(
    "mock identity provider startup",
    async () => (await status("http://127.0.0.1:8099/keys")) === "200",
  );

  const config = Object.fromEntries(
    (await readFile(join(root, "servers/delitools-api/.env.example"), "utf8"))
      .split("\n")
      .filter((line) => /^[A-Z][A-Z_]*=/u.test(line))
      .map((line) => {
        const at = line.indexOf("=");
        return [
          line.slice(0, at),
          line.slice(at + 1).replace(/^(['"])(.*)\1$/u, "$2"),
        ];
      }),
  );
  Object.assign(config, {
    DATABASE_URL:
      "postgres://postgres:fixture-only@127.0.0.1:5432/delitools?sslmode=disable",
    ISSUER: "http://127.0.0.1:8099",
    LISTEN: "127.0.0.1:46322",
    TIMEOUT_SECONDS: "5",
  });
  const env = join(directory, "fixture.env");
  const writeEnvironment = (values) =>
    writeFile(
      env,
      Object.entries(values)
        .map(([key, value]) => `${key}=${value}`)
        .join("\n") + "\n",
      { mode: 0o600 },
    );
  await writeEnvironment(config);
  const options = ["--network", `container:${database}`, "--env-file", env];
  const run = ["run", "--rm", ...options, image];

  // No configuration, and no schema yet: serving is rejected and never migrates implicitly.
  await rejected(["run", "--rm", image]);
  await rejected([...run, "serve"]);
  assert.equal(
    await sql("SELECT to_regclass('delitools_schema') IS NULL"),
    "t",
  );

  // Operator commands need only DATABASE_URL.
  await writeEnvironment({ DATABASE_URL: config.DATABASE_URL });
  await docker(...run, "--migrate");
  await docker(...run, "--migrate");
  await docker(...run, "check-schema");
  await writeEnvironment(config);
  const version = await sql("SELECT max(version) FROM delitools_schema");
  assert.match(version, /^\d+$/u);
  await sql(`UPDATE delitools_schema SET version=999 WHERE version=${version}`);
  await rejected([...run, "serve"]);
  await rejected([...run, "--migrate"]);
  await rejected([...run, "check-schema"]);
  await sql(`UPDATE delitools_schema SET version=${version} WHERE version=999`);

  // Invalid configuration is rejected before serving, without echoing the value.
  await writeEnvironment({ ...config, TENANT_CONCURRENCY: "0" });
  await rejected([...run, "serve"]);
  await writeEnvironment(config);

  containers.push(app);
  await docker("run", "-d", "--name", app, ...options, image, "serve");
  await until(
    "API liveness",
    async () => (await status("http://127.0.0.1:46322/healthz")) === "200",
  );
  // Settings are seeded once from the environment and the instance lock rejects a second process.
  assert.equal(await sql("SELECT count(*) FROM settings"), "1");
  await rejected([...run, "serve"]);
  // Readiness stays false until the catalog holds a synchronized price.
  assert.equal(await status("http://127.0.0.1:46322/readyz"), "503");
  await sql(
    "INSERT INTO catalog_prices(id,model,effective_at,input_usd_per_token,output_usd_per_token,synced_at,created_at) VALUES('01900000-0000-7000-8000-000000000010','openai/gpt-6-luna',now(),0.000001,0.000002,now(),now())",
  );
  assert.equal(await status("http://127.0.0.1:46322/readyz"), "200");
  assert.match(await metrics(), /delitools_catalog_price_available/u);
  await docker("stop", "--time", "15", app);
  assert.equal(
    await docker("inspect", "--format", "{{.State.ExitCode}}", app),
    "0",
  );
  console.log("DeliTools OCI lifecycle and schema rejection passed");
} catch (error) {
  for (const container of containers) {
    console.error(`OCI fixture logs: ${container}`);
    console.error(
      await docker("logs", container).catch(() => "logs unavailable"),
    );
  }
  throw error;
} finally {
  for (const container of containers.reverse()) {
    await docker("rm", "-f", container).catch(() => {});
  }
  await rm(directory, { recursive: true, force: true });
}
