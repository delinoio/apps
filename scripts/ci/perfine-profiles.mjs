// SPDX-License-Identifier: Apache-2.0
import { execFileSync, spawnSync } from "node:child_process";
import {
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  realpathSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export function profileRuns(language, route) {
  const native = ["rust", "c", "cpp", "zig"].includes(language);
  if (
    !(
      native
        ? ["callgrind", "gungraun"]
        : ["go", "javascript"].includes(language)
          ? ["default"]
          : []
    ).includes(route)
  )
    throw new Error("Unsupported fixture language or route");
  return ["bulk", "protected"].flatMap((caseId) =>
    ["baseline", "baseline-repeat", "candidate"].map((run) => ({
      id: `${caseId}-${run}`,
      args: [
        "crates/perfine-core/tests/fixtures/run.py",
        "measure",
        language,
        "--variant",
        run === "candidate" ? "candidate" : "baseline",
        "--case",
        caseId,
        ...(native ? ["--route", route] : []),
      ],
    })),
  );
}

export function preserveResult(output, destination) {
  mkdirSync(destination, { recursive: true });
  const result = JSON.parse(readFileSync(join(output, "result.json"), "utf8"));
  const files = new Set([
    "result.json",
    ...readdirSync(output).filter((name) =>
      /^command-\d+\.(stdout|stderr)\.txt$/u.test(name),
    ),
    ...(result.profile_artifacts ?? []),
  ]);
  for (const name of files) {
    const source = realpathSync(join(output, name));
    const local = relative(realpathSync(output), source);
    if (isAbsolute(name) || local.startsWith("..") || isAbsolute(local))
      throw new Error("Fixture artifact escapes its output directory");
    mkdirSync(dirname(join(destination, name)), { recursive: true });
    copyFileSync(source, join(destination, name));
  }
  return result;
}

export function main(language, route) {
  const runs = profileRuns(language, route);
  if (process.platform !== "linux" || process.arch !== "x64")
    throw new Error("Fixture gate requires native Linux x86_64");
  const revision = execFileSync("git", ["rev-parse", "HEAD"], {
    encoding: "utf8",
  }).trim();
  const base = process.env.TURBO_SCM_BASE;
  if (
    !/^[a-f0-9]{40}$/u.test(base ?? "") ||
    process.env.TURBO_SCM_HEAD !== revision
  )
    throw new Error(
      "Fixture gate requires the planner comparison and checked-out head",
    );
  const directory = mkdtempSync(
    join(process.env.RUNNER_TEMP || tmpdir(), "perfine-profiles-"),
  );
  const evidence = resolve(".ci-evidence", `${language}-${route}`);
  mkdirSync(evidence, { recursive: true });
  const record = {
    revision,
    comparison: { base, head: revision },
    language,
    route,
    platform: { os: process.platform, arch: process.arch },
    runs: [],
    verdict: "not_evaluated",
    limitations: [
      "Representative fixture profiling only; baseline-repeat uses identical source content, not a candidate commit.",
      "The driver does not run the engine lifecycle or evaluate a performance policy.",
      "Live AI, GitHub App/OIDC, npm installation and deployment acceptance are unperformed.",
    ],
  };
  let failed = false;
  try {
    for (const run of runs) {
      const output = join(directory, run.id);
      const args = [...run.args, "--out", output];
      const execution = spawnSync("python3", args, { stdio: "inherit" });
      const entry = {
        id: run.id,
        command: ["python3", ...args],
        exitCode: execution.status,
        signal: execution.signal,
        result: "failed",
      };
      record.runs.push(entry);
      try {
        const result = preserveResult(output, join(evidence, run.id));
        if (
          execution.status !== 0 ||
          result.status !== "measured" ||
          !result.linux_x86_64 ||
          result.source_revision !== revision ||
          !result.profile_artifacts?.length
        )
          throw new Error("Incomplete or failed fixture profiling");
        entry.result = "measured";
      } catch (error) {
        entry.error = error.message;
        failed = true;
      }
    }
  } finally {
    writeFileSync(
      join(evidence, "execution.json"),
      JSON.stringify(record, null, 2) + "\n",
    );
  }
  if (failed)
    throw new Error(
      "One or more fixture profiles failed; inspect preserved evidence",
    );
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
)
  main(...process.argv.slice(2));
