import { execFileSync } from "node:child_process";
import { appendFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { lockfilePaths } from "./lockfile-paths.mjs";

export function select(paths, { baseLockfile, headLockfile } = {}) {
  if (paths.includes("pnpm-lock.yaml")) {
    const affected = lockfilePaths(baseLockfile, headLockfile);
    // Unknown ownership must retain global validation instead of skipping jobs.
    if (
      !affected.includes("pnpm-lock.yaml") &&
      affected.every((path) => Object.values(select([path])).some(Boolean))
    ) {
      paths = paths
        .filter((path) => path !== "pnpm-lock.yaml")
        .concat(affected);
    }
  }
  const nativeLock = paths.some((path) =>
    /^(Cargo\.lock|prebuilt-dependencies\.lock\.json)$/u.test(path),
  );
  const goLock = paths.includes("go.sum");
  const shared = paths.some(
    (path) =>
      !/^(Cargo\.lock|go\.sum|prebuilt-dependencies\.lock\.json)$/u.test(
        path,
      ) &&
      !/^(apps|actions|protos|examples|docs|servers)\//u.test(path) &&
      !/^(crates\/fixkey-core\/|\.github\/workflows\/release-fixkey\.yml$)/u.test(
        path,
      ),
  );
  const serverShared = paths.some((path) =>
    /^servers\/(?:[^/]+$|scripts\/)/u.test(path),
  );
  const serverRust = paths.some((path) =>
    /^(servers\/crates\/|servers\/Cargo\.(toml|lock)$)/u.test(path),
  );
  return {
    delcore_api:
      shared ||
      goLock ||
      serverShared ||
      paths.some((path) =>
        /^(servers\/delcore-api\/|protos\/|docs\/(?:project-delcore|servers-delcore|protos-delcore-v1-contract\.md$))/u.test(
          path,
        ),
      ),
    devbase_api:
      serverRust ||
      shared ||
      goLock ||
      serverShared ||
      paths.some((path) =>
        /^(servers\/devbase-api\/|protos\/|examples\/devbase|docs\/(?:project-devbase|servers-devbase|protos-devbase-v1-contract\.md$))/u.test(
          path,
        ),
      ),
    git_tracker_api:
      serverRust ||
      shared ||
      goLock ||
      serverShared ||
      paths.some((path) =>
        /^(servers\/git-tracker-api\/|protos\/|docs\/(?:project-git-tracker|servers-git-tracker|protos-git-tracker-v1-contract\.md$))/u.test(
          path,
        ),
      ),
    git_tracker:
      shared ||
      paths.some((path) =>
        /^(apps\/git-tracker\/|actions\/git-tracker\/|protos\/|docs\/.*git-tracker)/u.test(
          path,
        ),
      ),
    fixkey:
      shared ||
      nativeLock ||
      paths.some((path) =>
        /^(apps\/fixkey-app\/|crates\/fixkey-core\/|\.github\/workflows\/release-fixkey\.yml$|docs\/.*fixkey)/u.test(
          path,
        ),
      ),
    devbase:
      shared ||
      paths.some((path) =>
        /^(apps\/devbase\/|docs\/(?:project-devbase|apps-devbase))/u.test(path),
      ),
    delino_site:
      shared ||
      paths.some((path) =>
        /^(apps\/delino-site\/|docs\/.*delino-site)/u.test(path),
      ),
    delino_docs:
      shared ||
      paths.some((path) =>
        /^(apps\/delino-docs\/|docs\/.*delino-docs)/u.test(path),
      ),
    delidev:
      shared ||
      nativeLock ||
      paths.some((path) => /^(apps\/delidev\/|docs\/.*delidev)/u.test(path)),
    delitools:
      shared ||
      nativeLock ||
      paths.some((path) =>
        /^(apps\/delitools\/|docs\/.*delitools)/u.test(path),
      ),
    protocol:
      shared ||
      goLock ||
      paths.some((path) =>
        /^(protos\/|examples\/|docs\/.*(?:delcore|devbase-v1))/u.test(path),
      ),
  };
}

export function selectRustServers(selected) {
  return [
    {
      service: "devbase-api",
      owner: "devbase_api",
      port: 55433,
      user: "devbase",
      database: "devbase_api_test",
    },
    {
      service: "git-tracker-api",
      owner: "git_tracker_api",
      port: 55432,
      user: "delcore",
      database: "delcore_api_test",
    },
  ]
    .filter(({ owner }) => selected[owner])
    .map(({ owner, ...entry }) => entry);
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  const git = (...args) =>
    execFileSync("git", args, {
      encoding: "utf8",
      maxBuffer: 16 * 1024 * 1024,
    }).trim();
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
  let lockfiles;
  if (paths.includes("pnpm-lock.yaml")) {
    try {
      lockfiles = {
        baseLockfile: git("show", `${base}:pnpm-lock.yaml`),
        headLockfile: git("show", `${head}:pnpm-lock.yaml`),
      };
    } catch {
      console.warn(
        JSON.stringify({
          event: "ci_lockfile_fallback",
          reason: "comparison_unavailable",
        }),
      );
    }
  }
  const selected = select(paths, lockfiles);
  console.log(JSON.stringify({ event: "ci_selection", base, head, selected }));
  appendFileSync(
    process.env.GITHUB_OUTPUT,
    Object.entries({
      base,
      head,
      ...selected,
      rust_servers: JSON.stringify(selectRustServers(selected)),
    })
      .map(([key, value]) => `${key}=${value}\n`)
      .join(""),
  );
}
