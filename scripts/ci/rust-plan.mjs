// SPDX-License-Identifier: Apache-2.0
import { execFileSync } from "node:child_process";
import { appendFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

export function selectRust(
  base,
  paths,
  run = (...args) =>
    JSON.parse(
      execFileSync("cargo", ["mono", "--output", "json", ...args], {
        encoding: "utf8",
        // cargo-mono 0.6.9 writes tracing logs to stdout, including in JSON mode.
        // Disable them for parsing; remove when upstream sends logs to stderr.
        env: { ...process.env, RUST_LOG: "off" },
      }),
    ),
) {
  const direct = run("changed", "--base", base, "--direct-only");
  const affected = run("changed", "--base", base);
  // cargo-mono uses merge-base. A rewritten push history must not omit changes
  // from the event's exact comparison; validate every package in that case.
  const global =
    direct.merge_base !== base ||
    affected.merge_base !== base ||
    paths.some((path) =>
      /^(\.github\/workflows\/CI\.yml$|\.github\/actions\/setup-ci-node\/|scripts\/ci\/|\.cargo\/|rust-toolchain(?:\.toml)?$|prebuilt-dependencies\.lock\.json$)/u.test(
        path,
      ),
    );
  const all = global ? run("list").packages.map((pkg) => pkg.name) : null;
  return {
    rust_clippy: all ?? direct.packages,
    rust_test: all ?? affected.packages,
  };
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  const base = process.env.TURBO_SCM_BASE;
  const head = process.env.TURBO_SCM_HEAD;
  const git = (...args) =>
    execFileSync("git", args, { encoding: "utf8" }).trim();
  if (!/^[a-f0-9]{40}$/u.test(base ?? "") || head !== git("rev-parse", "HEAD"))
    throw new Error(
      "Rust selection requires the planner comparison and checked-out head",
    );
  const paths = git("diff", "--name-only", base, head)
    .split("\n")
    .filter(Boolean);
  const selected = selectRust(base, paths);
  console.log(
    JSON.stringify({ event: "rust_ci_selection", base, head, ...selected }),
  );
  appendFileSync(
    process.env.GITHUB_OUTPUT,
    Object.entries(selected)
      .map(([key, packages]) => `${key}=${JSON.stringify(packages)}\n`)
      .join(""),
  );
}
