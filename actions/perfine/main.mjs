// SPDX-License-Identifier: Apache-2.0
import { createHash } from "node:crypto";
import {
  mkdtempSync,
  readFileSync,
  writeFileSync,
  rmSync,
  appendFileSync,
  lstatSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import yaml from "js-yaml";
import { upload } from "./upload.mjs";
export function executionPlan(env, directory) {
  const command = env.INPUT_COMMAND;
  if (command === "optimize" || command === "resume")
    throw new Error(
      "Cloud AI unavailable: adapters implement local auth files only. Use local perfine optimize with supported local authentication.",
    );
  if (command !== "measure")
    throw new Error(
      "Use measure or the separate upload stage. Publication requires explicit CLI approval.",
    );
  if (env.GITHUB_EVENT_NAME !== "workflow_dispatch")
    throw new Error("Perfine requires a manual workflow");
  if (
    !/^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(
      env.INPUT_EXPERIMENT ?? "",
    )
  )
    throw new Error("Explicit UUID v7 experiment required");
  if (
    !/^[1-9][0-9]*$/.test(env.GITHUB_RUN_ID ?? "") ||
    BigInt(env.GITHUB_RUN_ID) > 18446744073709551615n ||
    !/^[1-9][0-9]*$/.test(env.GITHUB_RUN_ATTEMPT ?? "") ||
    BigInt(env.GITHUB_RUN_ATTEMPT) > 4294967295n
  )
    throw new Error("Canonical GitHub run ID and attempt required");
  const config = env.INPUT_CONFIG;
  if (!config || Buffer.byteLength(config) > 1024 * 1024)
    throw new Error("Bounded inline configuration required");
  let parsed;
  try {
    parsed = yaml.load(config, { schema: yaml.JSON_SCHEMA });
  } catch {
    throw new Error("Invalid inline YAML configuration");
  }
  if (!parsed || typeof parsed !== "object" || parsed.agent)
    throw new Error(
      "Cloud agent authentication is unavailable; omit agent configuration",
    );
  const path = join(directory, "perfine.yaml");
  writeFileSync(path, config, { mode: 0o400, flag: "wx" });
  // Candidate and agent isolation is owned by the Rust Docker adapters. No runner
  // credentials or cached user home are inherited by the orchestrating CLI.
  const clean = {
    PATH: "/usr/local/bin:/usr/bin:/bin",
    HOME: directory,
    TMPDIR: directory,
    LANG: "C.UTF-8",
    PERFINE_GITHUB_RUN_ID: env.GITHUB_RUN_ID,
    PERFINE_GITHUB_RUN_ATTEMPT: env.GITHUB_RUN_ATTEMPT,
  };
  return {
    configHash: createHash("sha256").update(config).digest("hex"),
    args: [
      "--repository",
      resolve(env.GITHUB_WORKSPACE),
      "--config",
      path,
      "--json",
      "measure",
      "--experiment",
      env.INPUT_EXPERIMENT,
    ],
    env: clean,
  };
}
export async function main(env = process.env) {
  if (process.platform !== "linux" || process.arch !== "x64")
    throw new Error("Linux x86_64 required");
  if (env.INPUT_COMMAND === "upload") return upload(env);
  const directory = mkdtempSync(join(env.RUNNER_TEMP, "perfine-action-"));
  try {
    const plan = executionPlan(env, directory);
    const binary = join(__dirname, "runtime/perfine");
    if (!lstatSync(binary).isFile())
      throw new Error("Invalid distribution binary");
    const hash = createHash("sha256")
      .update(readFileSync(binary))
      .digest("hex");
    if (
      readFileSync(join(__dirname, "runtime/SHA256SUMS"), "utf8") !==
      `${hash}  perfine\n`
    )
      throw new Error("Distribution checksum mismatch");
    if (env.GITHUB_OUTPUT)
      appendFileSync(env.GITHUB_OUTPUT, `config-sha256=${plan.configHash}\n`);
    const result = spawnSync(binary, plan.args, {
      cwd: env.GITHUB_WORKSPACE,
      env: plan.env,
      stdio: "inherit",
      shell: false,
    });
    if (result.error || result.status !== 0)
      throw new Error(
        "Perfine measurement failed; retained evidence remains in the workspace",
      );
    if (
      createHash("sha256")
        .update(readFileSync(join(directory, "perfine.yaml")))
        .digest("hex") !== plan.configHash
    )
      throw new Error("Configuration changed during execution");
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}
// esbuild defines CommonJS entry point checks in the published bundle.
if (typeof require !== "undefined" && require.main === module)
  main().catch((error) => {
    console.error(
      JSON.stringify({
        event: "perfine_action_failed",
        message: error.message,
      }),
    );
    process.exitCode = 1;
  });
