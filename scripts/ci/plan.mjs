import { execFileSync } from "node:child_process";
import { appendFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

export function select(paths) {
  const shared = paths.some(
    (path) => !/^(apps|protos|examples|docs)\//u.test(path),
  );
  return {
    devbase:
      shared ||
      paths.some((path) => /^(apps\/devbase\/|docs\/.*devbase)/u.test(path)),
    delino_site:
      shared ||
      paths.some((path) =>
        /^(apps\/delino-site\/|docs\/.*delino-site)/u.test(path),
      ),
    delidev:
      shared ||
      paths.some((path) => /^(apps\/delidev\/|docs\/.*delidev)/u.test(path)),
    delitools:
      shared ||
      paths.some((path) =>
        /^(apps\/delitools\/|docs\/.*delitools)/u.test(path),
      ),
    protocol:
      shared ||
      paths.some((path) =>
        /^(protos\/|examples\/|docs\/.*delcore)/u.test(path),
      ),
  };
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  const git = (...args) =>
    execFileSync("git", args, { encoding: "utf8" }).trim();
  const head = git("rev-parse", "HEAD");
  const event = process.env.GITHUB_EVENT_NAME;
  let base = process.env.COMPARISON_BASE;
  if (event === "workflow_dispatch") {
    base =
      process.env.GITHUB_REF === "refs/heads/main"
        ? git("rev-parse", "HEAD^")
        : git("merge-base", "origin/main", "HEAD");
  }
  if (!base || /^0+$/u.test(base))
    base = git("rev-list", "--max-parents=0", "HEAD");
  if (!/^[a-f0-9]{40}$/u.test(base))
    throw new Error("Invalid comparison revision");
  const paths = git("diff", "--name-only", base, head)
    .split("\n")
    .filter(Boolean);
  const selected = select(paths);
  console.log(JSON.stringify({ event: "ci_selection", base, head, selected }));
  appendFileSync(
    process.env.GITHUB_OUTPUT,
    Object.entries({ base, head, ...selected })
      .map(([key, value]) => `${key}=${value}\n`)
      .join(""),
  );
}
