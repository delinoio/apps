import { execFileSync } from "node:child_process";
import { appendFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { lockfilePaths } from "./lockfile-paths.mjs";

export const isInstructionPath = (path) =>
  /(?:^|\/)(?:AGENTS|CLAUDE)\.md$/u.test(path);

// Internal contracts and server runbooks are prose, not runtime build inputs.
// Keep application documentation eligible: public docs can be compiled content.
export const isValidationExcludedPath = (path) =>
  isInstructionPath(path) ||
  /^docs\/.*\.md$/u.test(path) ||
  /^(?:README\.md|servers\/(?:.*\/)?README\.md)$/u.test(path);

function affectedPaths(paths, { baseLockfile, headLockfile } = {}) {
  paths = paths.filter((path) => !isValidationExcludedPath(path));
  if (paths.includes("pnpm-lock.yaml")) {
    const affected = lockfilePaths(baseLockfile, headLockfile);
    // Unknown ownership must retain global validation instead of skipping jobs.
    if (
      !affected.includes("pnpm-lock.yaml") &&
      affected.every(
        (path) =>
          Object.values(select([path])).some(Boolean) ||
          classifyShared([path]).shared.lynxUi,
      )
    ) {
      paths = paths
        .filter((path) => path !== "pnpm-lock.yaml")
        .concat(affected);
    }
  }
  return paths;
}

function classifyShared(paths) {
  const sharedPatterns = {
    frontend: /^packages\/ui\//u,
    lynxUi: /^packages\/lynx-ui\//u,
    native:
      /^(Cargo\.(toml|lock)$|prebuilt-dependencies\.lock\.json$|scripts\/(?:tauri-cli|prebuilt-dependencies|archive)(?:\.test)?\.mjs$)/u,
    rust: /^(rust-toolchain(?:\.toml)?$|\.cargo\/)/u,
    go: /^go\.(mod|sum)$/u,
    protocol: /^buf(?:\.gen)?\.yaml$/u,
    localization: /^scripts\/localization\//u,
  };
  const shared = Object.fromEntries(
    Object.entries(sharedPatterns).map(([kind, pattern]) => [
      kind,
      paths.some((path) => pattern.test(path)),
    ]),
  );
  // Unclassified shared paths retain global validation for unknown consumers.
  const repositoryShared = paths.some(
    (path) =>
      !Object.values(sharedPatterns).some((pattern) => pattern.test(path)) &&
      // Release records do not change runtime inputs. Configuration files keep
      // the conservative shared-path fallback; record validation always runs
      // in check-formatting.
      !/^\.changeset\/[^/]+\.md$/u.test(path) &&
      // Namespace metadata has no runtime consumers. Keep this list exact so
      // future Apache service code retains fallback until its CI is registered.
      !/^oss\/(?:LICENSE|NOTICE|servers\/\.gitkeep)$/u.test(path) &&
      !/^(crates\/perfine-core\/|cmds\/(?:perfine|lane)\/|packages\/(?:perfine|lane)\/)/u.test(
        path,
      ) &&
      !/^(apps|actions|protos|examples|docs|servers)\//u.test(path),
  );
  return { shared, repositoryShared };
}

export function selectChecks(paths, lockfiles) {
  paths = affectedPaths(paths, lockfiles);
  const { shared, repositoryShared } = classifyShared(paths);
  return {
    tooling:
      repositoryShared ||
      paths.some((path) =>
        /^(\.github\/|scripts\/|prebuilt-dependencies\.lock\.json$|servers\/(?:[^/]+\/)?scripts\/|apps\/[^/]+\/package\.json$|apps\/delitools\/scripts\/)/u.test(
          path,
        ),
      ),
    shared_ui: repositoryShared || shared.frontend || shared.lynxUi,
    workflows:
      repositoryShared ||
      shared.go ||
      paths.some((path) => /^\.github\//u.test(path)),
  };
}

export function select(paths, lockfiles) {
  paths = affectedPaths(paths, lockfiles);
  const { shared, repositoryShared } = classifyShared(paths);
  const serverShared = paths.some((path) =>
    /^servers\/(?:[^/]+$|scripts\/)/u.test(path),
  );
  const serverRust = paths.some((path) =>
    /^(servers\/crates\/|servers\/Cargo\.(toml|lock)$)/u.test(path),
  );
  return {
    delcore_api:
      serverRust ||
      shared.rust ||
      repositoryShared ||
      shared.protocol ||
      serverShared ||
      paths.some((path) => /^(servers\/delcore-api\/|protos\/)/u.test(path)),
    delitools_api:
      serverRust ||
      shared.rust ||
      repositoryShared ||
      shared.protocol ||
      serverShared ||
      paths.some((path) =>
        /^(servers\/delitools-api\/|protos\/|examples\/delitools)/u.test(path),
      ),
    devbase_api:
      serverRust ||
      shared.rust ||
      repositoryShared ||
      shared.protocol ||
      serverShared ||
      paths.some((path) =>
        /^(servers\/devbase-api\/|protos\/|examples\/devbase)/u.test(path),
      ),
    git_tracker_api:
      serverRust ||
      shared.rust ||
      repositoryShared ||
      shared.protocol ||
      serverShared ||
      paths.some((path) =>
        /^(servers\/git-tracker-api\/|protos\/)/u.test(path),
      ),
    git_tracker:
      repositoryShared ||
      shared.frontend ||
      shared.protocol ||
      paths.some((path) =>
        /^(apps\/git-tracker\/|actions\/git-tracker\/|protos\/)/u.test(path),
      ),
    devbase:
      repositoryShared ||
      shared.frontend ||
      paths.some((path) => /^apps\/devbase\//u.test(path)),
    delino_console:
      repositoryShared ||
      shared.localization ||
      paths.some((path) => /^apps\/delino-console\//u.test(path)),
    delino_site:
      repositoryShared ||
      shared.frontend ||
      shared.localization ||
      paths.some((path) => /^apps\/delino-site\//u.test(path)),
    delino_docs:
      repositoryShared ||
      shared.frontend ||
      shared.localization ||
      paths.some((path) => /^apps\/delino-docs\//u.test(path)),
    delidev:
      repositoryShared ||
      shared.frontend ||
      shared.native ||
      shared.rust ||
      paths.some((path) => /^apps\/delidev\//u.test(path)),
    delitools:
      repositoryShared ||
      shared.frontend ||
      shared.native ||
      shared.rust ||
      paths.some((path) => /^apps\/delitools\//u.test(path)),
    perfine_api:
      serverRust ||
      shared.rust ||
      repositoryShared ||
      shared.protocol ||
      serverShared ||
      paths.some((path) => /^(servers\/perfine-api\/|protos\/)/u.test(path)),
    lane_api:
      serverRust ||
      shared.rust ||
      repositoryShared ||
      shared.protocol ||
      serverShared ||
      paths.some((path) => /^(servers\/lane-api\/|protos\/)/u.test(path)),
    lane_cli:
      repositoryShared ||
      shared.rust ||
      shared.protocol ||
      paths.some((path) =>
        /^(Cargo\.(toml|lock)$|cmds\/lane\/|packages\/lane\/|apps\/lane\/native\/store\/|protos\/lane\/)/u.test(
          path,
        ),
      ),
    lane:
      repositoryShared ||
      shared.lynxUi ||
      shared.protocol ||
      paths.some((path) =>
        /^(apps\/(?:lane|lane-chrome-extension)\/|protos\/lane\/)/u.test(path),
      ),
    perfine_fixtures:
      repositoryShared ||
      shared.rust ||
      paths.some((path) =>
        /^(crates\/perfine-core\/|cmds\/perfine\/|Cargo\.(toml|lock)$)/u.test(
          path,
        ),
      ),
    protocol:
      repositoryShared ||
      shared.protocol ||
      shared.go ||
      paths.some((path) => /^(protos\/|examples\/)/u.test(path)),
  };
}

export function selectRustServers(selected) {
  return [
    {
      service: "delcore-api",
      owner: "delcore_api",
      port: 55434,
      user: "delcore",
      database: "delcore_api_test",
    },
    {
      service: "delitools-api",
      owner: "delitools_api",
      port: 55435,
      user: "delitools",
      database: "delitools_api_test",
    },
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
    {
      service: "lane-api",
      owner: "lane_api",
      port: 55437,
      user: "lane",
      database: "lane_api_test",
    },
    {
      service: "perfine-api",
      owner: "perfine_api",
      port: 55436,
      user: "perfine",
      database: "perfine_api_test",
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
  const selected = {
    ...select(paths, lockfiles),
    ...selectChecks(paths, lockfiles),
  };
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
