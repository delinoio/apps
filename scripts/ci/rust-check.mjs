// SPDX-License-Identifier: Apache-2.0
import { execFileSync } from "node:child_process";

const Check = Object.freeze({ Clippy: "clippy", Test: "test" });
const check = process.argv[2];
if (!Object.values(Check).includes(check))
  throw new Error("Unknown Rust check");
const packages = JSON.parse(process.env.RUST_PACKAGES);
if (
  !Array.isArray(packages) ||
  packages.length === 0 ||
  packages.some(
    (name) =>
      typeof name !== "string" || !/^[a-zA-Z0-9_][a-zA-Z0-9_-]*$/u.test(name),
  )
)
  throw new Error("Rust checks require a nonempty package selection");
const args = [
  check,
  "--locked",
  ...packages.flatMap((name) => ["-p", name]),
  "--all-targets",
];
if (check === Check.Clippy) args.push("--all-features", "--", "-D", "warnings");
console.log(JSON.stringify({ event: "rust_ci_check", command: "cargo", args }));
execFileSync("cargo", args, { stdio: "inherit" });
