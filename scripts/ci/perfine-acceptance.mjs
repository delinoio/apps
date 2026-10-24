// SPDX-License-Identifier: Apache-2.0
import { execFileSync, spawnSync } from "node:child_process";
import {
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const language = process.argv[2];
if (!["rust", "c", "cpp", "zig", "go", "javascript"].includes(language))
  throw new Error("Expected one of the six fixture languages");
const native = process.platform === "linux" && process.arch === "x64";
if (!native && process.env.PERFINE_ALLOW_EMULATED !== "1")
  throw new Error(
    "Requires native Linux x86_64; local emulation requires PERFINE_ALLOW_EMULATED=1",
  );
const capture = (args) =>
  execFileSync(args[0], args.slice(1), { encoding: "utf8" }).trim();
const execute = (args, env = process.env) => {
  const result = spawnSync(args[0], args.slice(1), { stdio: "inherit", env });
  if (result.status !== 0)
    throw new Error(`Command failed (${result.status}): ${args.join(" ")}`);
};
const work = mkdtempSync(join(tmpdir(), "perfine-six-"));
const evidence = resolve(
  process.env.PERFINE_SIX_EVIDENCE || ".ci-evidence/perfine-acceptance",
  language,
);
mkdirSync(evidence, { recursive: true });
const measurementTag = `perfine-six-languages:test-${language}-${process.pid}`;
const agentTag = `perfine-six-agent:test-${language}-${process.pid}`;
const network = `perfine-six-${language}-${process.pid}`;
const record = {
  revision: capture(["git", "rev-parse", "HEAD"]),
  language,
  sourceWorktreeDirty: capture(["git", "status", "--porcelain"]).length > 0,
  host: { platform: process.platform, architecture: process.arch },
  nativeLinuxX86_64: native,
  measurement: "actual Docker compiler/correctness/profiler execution",
  agent: "deterministic structured-response fixture; no live AI",
  limitations: [
    "No live provider, account, publication, or deployment acceptance.",
    ...(native
      ? []
      : ["Emulated linux/amd64; not native Linux x86_64 acceptance."]),
  ],
  result: "failed",
};
let networkCreated = false;
try {
  execute([
    "docker",
    "build",
    "--platform=linux/amd64",
    "--file",
    "cmds/perfine/tests/fixtures/six-languages.Dockerfile",
    "--tag",
    measurementTag,
    "cmds/perfine/tests/fixtures",
  ]);
  record.measurementImage = capture([
    "docker",
    "image",
    "inspect",
    measurementTag,
    "--format",
    "{{.Id}}",
  ]);
  for (const file of ["agent.mjs", "six-agent.Dockerfile"])
    copyFileSync(join("cmds/perfine/tests/fixtures", file), join(work, file));
  for (const [lang, ext] of [
    ["rust", "rs"],
    ["c", "c"],
    ["cpp", "cpp"],
    ["zig", "zig"],
    ["go", "go"],
    ["javascript", "mjs"],
  ])
    for (const variant of ["baseline", "candidate"])
      copyFileSync(
        join("crates/perfine-core/tests/fixtures", lang, `${variant}.${ext}`),
        join(work, `${variant}.${ext}`),
      );
  const baseTag = `perfine-fixture-base:${record.measurementImage.slice(7)}`;
  execute(["docker", "tag", record.measurementImage, baseTag]);
  execute([
    "docker",
    "build",
    "--platform=linux/amd64",
    "--build-arg",
    `BASE=${baseTag}`,
    "--file",
    join(work, "six-agent.Dockerfile"),
    "--tag",
    agentTag,
    work,
  ]);
  record.agentImage = capture([
    "docker",
    "image",
    "inspect",
    agentTag,
    "--format",
    "{{.Id}}",
  ]);
  execute(["docker", "network", "create", "--internal", network]);
  networkCreated = true;
  record.command = [
    "cargo",
    "test",
    "--locked",
    "-p",
    "perfine",
    "--test",
    "local",
    `six_languages::six_language_${language}`,
    "--",
    "--exact",
    "--ignored",
    "--nocapture",
    "--test-threads=1",
  ];
  const testEnvironment = {
    ...process.env,
    PERFINE_TEST_DOCKER: capture(["which", "docker"]),
    PERFINE_TEST_IMAGE: record.measurementImage,
    PERFINE_TEST_AGENT_IMAGE: record.agentImage,
    PERFINE_TEST_AGENT_NETWORK: network,
    PERFINE_SIX_EVIDENCE: evidence,
  };
  if (language === "c") {
    record.adapterCommand = [
      "cargo",
      "test",
      "--locked",
      "-p",
      "perfine-core",
      "--test",
      "adapter_execution",
      "--",
      "--ignored",
      "--nocapture",
    ];
    execute(record.adapterCommand, testEnvironment);
  }
  execute(record.command, testEnvironment);
  record.result = "passed";
} finally {
  writeFileSync(
    join(evidence, "execution.json"),
    JSON.stringify(record, null, 2) + "\n",
  );
  if (networkCreated) execute(["docker", "network", "rm", network]);
  rmSync(work, { recursive: true, force: true });
}
