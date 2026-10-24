// SPDX-License-Identifier: FSL-1.1-ALv2
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdtemp, readFile, rm, mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { setTimeout as delay } from "node:timers/promises";

const exec = promisify(execFile);
const root = fileURLToPath(new URL("../../../", import.meta.url));
const name = `delcore-oci-${process.pid}-${Date.now()}`;
const database = `${name}-db`;
const provider = `${name}-provider`;
const app = `${name}-app`;
const image = "delcore-api:validation";
const containers = [];
const directory = await mkdtemp(join(tmpdir(), "delcore-oci-"));
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
const sql = (statement, db = "delcore") =>
  docker(
    "exec",
    database,
    "psql",
    "-U",
    "postgres",
    "-d",
    db,
    "-v",
    "ON_ERROR_STOP=1",
    "-Atc",
    statement,
  );
const fetchFixture = (url) =>
  docker(
    "exec",
    provider,
    "node",
    "--input-type=module",
    "-e",
    `const r = await fetch(${JSON.stringify(url)}); if (!r.ok) process.exit(1); console.log(await r.text());`,
  );
try {
  console.log("Building Delcore OCI image");
  await exec(
    "docker",
    ["build", "-f", "servers/delcore-api/Dockerfile", "-t", image, "."],
    { cwd: root, timeout: 30 * 60 * 1000, maxBuffer: 32 * 1024 * 1024 },
  );
  assert.equal(
    await docker("image", "inspect", "--format", "{{.Config.User}}", image),
    "65532:65532",
  );
  containers.push(database);
  await docker(
    "run",
    "-d",
    "--name",
    database,
    "-e",
    "POSTGRES_PASSWORD=fixture-only",
    "-e",
    "POSTGRES_DB=delcore",
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
    `type=bind,src=${join(root, "servers/delcore-api/scripts/oci-provider.mjs")},dst=/provider.mjs,readonly`,
    "node:24-bookworm-slim",
    "node",
    "/provider.mjs",
  );
  await until("mock provider startup", async () => {
    try {
      await fetchFixture("http://127.0.0.1:8099/keys");
      return true;
    } catch {
      return false;
    }
  });
  const config = Object.fromEntries(
    (await readFile(join(root, "servers/delcore-api/.env.example"), "utf8"))
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
      "postgres://postgres:fixture-only@127.0.0.1:5432/delcore?sslmode=disable",
    ISSUER: "http://127.0.0.1:8099",
    PADDLE_URL: "http://127.0.0.1:8099",
    LISTEN: "127.0.0.1:8080",
    TIMEOUT_SECONDS: "5",
    DEVELOPMENT: "true",
    CATALOG_DIR: "/catalog",
  });
  const catalog = join(directory, "catalog");
  await mkdir(catalog, { mode: 0o755 });
  await writeFile(
    join(catalog, "service.yaml"),
    "kind: service\nkey: oci-fixture\nname: OCI Fixture\nclient_id: oci-fixture\ncapabilities: [execute]\nscopes: [authorize]\n",
    { mode: 0o644 },
  );
  const env = join(directory, "fixture.env");
  const writeEnvironment = () =>
    writeFile(
      env,
      Object.entries(config)
        .map(([key, value]) => `${key}=${value}`)
        .join("\n") + "\n",
      { mode: 0o600 },
    );
  await writeEnvironment();
  const options = [
    "--network",
    `container:${database}`,
    "--mount",
    `type=bind,src=${catalog},dst=/catalog,readonly`,
    "--env-file",
    env,
  ];
  const run = ["run", "--rm", ...options, image];
  await rejected(["run", "--rm", image]);
  await rejected([...run, "serve"]);
  await docker(...run, "migrate");
  await docker(...run, "check-schema");
  const version = await sql("SELECT max(version) FROM delcore_schema");
  assert.match(version, /^\d+$/u);
  await sql(`UPDATE delcore_schema SET version=999 WHERE version=${version}`);
  await rejected([...run, "serve"]);
  await rejected([...run, "migrate"]);
  await rejected([...run, "check-schema"]);
  await sql(`UPDATE delcore_schema SET version=${version} WHERE version=999`);
  config.CATALOG_DIR = "/missing-catalog";
  await writeEnvironment();
  await rejected([...run, "serve"]);
  config.CATALOG_DIR = "/catalog";
  await writeEnvironment();
  containers.push(app);
  await docker("run", "-d", "--name", app, ...options, image, "serve");
  await until("API readiness", async () => {
    try {
      await fetchFixture("http://127.0.0.1:8080/readyz");
      return true;
    } catch {
      return false;
    }
  });
  await rejected([...run, "serve"]);
  await sql(`
BEGIN;
INSERT INTO users VALUES('01900000-0000-7000-8000-000000000001','fixture','fixture','fixture@example.test',NULL);
INSERT INTO plans VALUES('01900000-0000-7000-8000-000000000002','free',0,true,true,false);
INSERT INTO catalog_entities(id,kind,scope,key) VALUES('01900000-0000-7000-8000-000000000002','plan','','free');
INSERT INTO catalog_versions(id,entity_id,semantic_hash,definition) VALUES('01900000-0000-7000-8000-000000000005','01900000-0000-7000-8000-000000000002','9a5611e2ab7fe315273f427050c2676dd887cd08ccae40f306c3c86dd4df1a91','{"features":[],"free":true,"postpaid":false,"prices":[],"rank":0}');
UPDATE catalog_entities SET active_version_id='01900000-0000-7000-8000-000000000005' WHERE id='01900000-0000-7000-8000-000000000002';
INSERT INTO tenants(id,name,personal,owner_id,plan_id,period_start,period_end) VALUES('01900000-0000-7000-8000-000000000003','Fixture',true,'01900000-0000-7000-8000-000000000001','01900000-0000-7000-8000-000000000002',now(),now()+interval '1 month');
INSERT INTO tenant_catalog_selections(id,tenant_id,product_id,version_id,starts_at) VALUES('01900000-0000-7000-8000-000000000006','01900000-0000-7000-8000-000000000003','01900000-0000-7000-8000-000000000002','01900000-0000-7000-8000-000000000005',now());
INSERT INTO jobs(id,tenant_id,kind,dedup_key,payload,available_at,created_at,updated_at) VALUES('01900000-0000-7000-8000-000000000004','01900000-0000-7000-8000-000000000003','topup','crash-fixture','{"amount":"10","minor_units":"1000","currency":"USD","return_url":"https://consumer.example.invalid/delcore/return"}',now(),now(),now());
COMMIT;`);
  await until(
    "payment acceptance",
    async () =>
      JSON.parse(await fetchFixture("http://127.0.0.1:8099/transactions")).data
        .length === 1,
  );
  await docker("kill", app);
  assert.equal(
    await sql("SELECT state FROM jobs WHERE dedup_key='crash-fixture'"),
    "running",
  );
  await docker("start", app);
  await until(
    "payment reconciliation after process death",
    async () =>
      (await sql("SELECT state FROM jobs WHERE dedup_key='crash-fixture'")) ===
      "done",
  );
  assert.equal(
    Number(await sql("SELECT sum(amount) FROM ledger WHERE kind='credit'")),
    10,
  );
  assert.equal(
    JSON.parse(await fetchFixture("http://127.0.0.1:8099/transactions")).data
      .length,
    1,
    "recovery must not create another payment",
  );
  await docker("stop", "--time", "15", app);
  assert.equal(
    await docker("inspect", "--format", "{{.State.ExitCode}}", app),
    "0",
  );
  await docker(
    "exec",
    database,
    "pg_dump",
    "-U",
    "postgres",
    "-d",
    "delcore",
    "-Fc",
    "-f",
    "/tmp/backup.dump",
  );
  await docker("exec", database, "createdb", "-U", "postgres", "restored");
  await docker(
    "exec",
    database,
    "pg_restore",
    "-U",
    "postgres",
    "-d",
    "restored",
    "/tmp/backup.dump",
  );
  config.DATABASE_URL =
    "postgres://postgres:fixture-only@127.0.0.1:5432/restored?sslmode=disable";
  await writeEnvironment();
  await docker(...run, "migrate");
  await docker(...run, "check-schema");
  assert.equal(
    await sql("SELECT max(version) FROM delcore_schema", "restored"),
    version,
  );
  console.log(
    "Delcore OCI lifecycle, crash recovery and backup restoration passed",
  );
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
