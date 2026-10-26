// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  appendFileSync,
  mkdirSync,
  readFileSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { bumpVersion, versionChanges, Project } from "./project.mjs";

const manifestPath = ".release/manifest.json";
export const servers = Object.freeze([
  "delcore-api",
  "delitools-api",
  "devbase-api",
  "git-tracker-api",
  "lane-api",
  "perfine-api",
]);
export const apps = Object.freeze({
  "delino-site": [
    "delino-site-production",
    "CLOUDFLARE_PAGES_PROJECT_SITE",
    [],
  ],
  "delino-docs": [
    "delino-site-production",
    "CLOUDFLARE_PAGES_PROJECT_DOCS",
    [],
  ],
  devbase: ["devbase-production", "CLOUDFLARE_PAGES_PROJECT", ["devbase-api"]],
  "git-tracker": [
    "delino-site-production",
    "CLOUDFLARE_PAGES_PROJECT_GIT_TRACKER",
    ["git-tracker-api"],
  ],
  lane: [
    "delino-site-production",
    "CLOUDFLARE_PAGES_PROJECT_LANE",
    ["lane-api"],
  ],
  // The scaffold has no API calls today. Keep its independently released API
  // in the publication closure so future client changes cannot precede it.
  delitools: [
    "delitools-production",
    "CLOUDFLARE_PAGES_PROJECT",
    ["delitools-api"],
  ],
});
const run = (cwd, command, args) =>
  execFileSync(command, args, { cwd, encoding: "utf8" }).trim();
const git = (cwd, ...args) => run(cwd, "git", args);

export function planRelease(records, inventory, read) {
  const bumps = {};
  const rank = ["none", "patch", "minor", "major"];
  for (const record of records) {
    assert.match(record.id, /^[a-z0-9]+(?:-[a-z0-9]+)*$/);
    for (const [name, bump] of Object.entries(record.packages)) {
      assert.ok(inventory[name], `Unknown package ${name}`);
      assert.ok(rank.includes(bump), `Unknown bump ${bump}`);
      if (!(name in bumps) || rank.indexOf(bump) > rank.indexOf(bumps[name]))
        bumps[name] = bump;
    }
  }
  const changes = {};
  const selectedApps = new Set();
  const selectedServers = new Set();
  const versions = {};
  for (const [name, bump] of Object.entries(bumps).sort()) {
    const file = inventory[name];
    const app = Object.keys(apps).find(
      (id) => file === `apps/${id}/package.json`,
    );
    const server = servers.find((id) => file === `servers/${id}/package.json`);
    // Unsupported distribution records must survive until their owning release
    // gate can consume them. Never silently discard a requested publication.
    assert.ok(
      app || server || bump === "none",
      `No release target for ${name}; keep its changeset for the owning distribution workflow`,
    );
    if (app) selectedApps.add(app);
    if (server) selectedServers.add(server);
    if (bump === "none") continue;
    const source = read(file);
    const pkg = JSON.parse(source);
    assert.equal(pkg.name, name);
    const version = bumpVersion(pkg.version, bump);
    versions[name] = { previous: pkg.version, version, bump };
    if (app === "delitools") {
      Object.assign(
        changes,
        versionChanges(Project.Delitools, bump, read).changes,
      );
    } else {
      assert.equal(
        [...source.matchAll(/^  "version": "[^"]+"/gm)].length,
        1,
        `Ambiguous version in ${file}`,
      );
      changes[file] = source.replace(
        /^  "version": "[^"]+"/m,
        `  "version": "${version}"`,
      );
    }
  }
  for (const app of selectedApps)
    for (const server of apps[app][2]) selectedServers.add(server);
  // Every consumer API uses Delcore. Only dependencies are expanded: a server
  // change never selects unrelated frontends or bumps a dependency's version.
  if ([...selectedServers].some((id) => id !== "delcore-api"))
    selectedServers.add("delcore-api");
  return {
    records: records.map(({ id, packages, public: note }) => ({
      id,
      packages,
      ...(note ? { public: note } : {}),
    })),
    versions,
    core: selectedServers.has("delcore-api"),
    servers: servers.filter(
      (id) => id !== "delcore-api" && selectedServers.has(id),
    ),
    web: [...selectedApps].sort().map((app) => ({
      app,
      environment: apps[app][0],
      pages_project_variable: apps[app][1],
      concurrency_group: `release-${app}`,
    })),
    changes,
  };
}

export function prepareChangesets({ directory, runId, load }) {
  assert.match(runId, /^[1-9]\d*$/);
  assert.equal(
    git(directory, "status", "--porcelain"),
    "",
    "Release requires a clean checkout",
  );
  git(directory, "fetch", "origin", "main");
  const marker = `Changeset-Release-Run: ${runId}`;
  const previous = git(
    directory,
    "log",
    "origin/main",
    "--format=%H",
    "--fixed-strings",
    `--grep=${marker}`,
  )
    .split("\n")
    .filter(Boolean)
    .filter((sha) =>
      git(directory, "show", "-s", "--format=%B", sha)
        .split("\n")
        .includes(marker),
    );
  assert.ok(previous.length <= 1, "Ambiguous release history");
  if (previous.length) {
    const revision = previous[0];
    const manifest = JSON.parse(
      git(directory, "show", `${revision}:${manifestPath}`),
    );
    assert.equal(manifest.runId, runId);
    assert.equal(manifest.source, git(directory, "rev-parse", `${revision}^`));
    return { ...manifest, revision };
  }
  const source = git(directory, "rev-parse", "HEAD");
  assert.equal(
    source,
    git(directory, "rev-parse", "origin/main"),
    "Main advanced; start a new release run",
  );
  const { records, inventory } = load();
  if (!records.length)
    return { revision: source, core: false, servers: [], web: [], empty: true };
  const { changes, ...plan } = planRelease(records, inventory, (path) =>
    readFileSync(resolve(directory, path), "utf8"),
  );
  const manifest = { schema: 1, runId, source, ...plan };
  for (const [path, content] of Object.entries(changes))
    writeFileSync(resolve(directory, path), content);
  for (const { id } of records)
    unlinkSync(resolve(directory, `.changeset/${id}.md`));
  mkdirSync(resolve(directory, dirname(manifestPath)), { recursive: true });
  writeFileSync(
    resolve(directory, manifestPath),
    `${JSON.stringify(manifest, null, 2)}\n`,
  );
  git(
    directory,
    "add",
    "--",
    manifestPath,
    ...Object.keys(changes),
    ...records.map(({ id }) => `.changeset/${id}.md`),
  );
  git(
    directory,
    "commit",
    "-m",
    "chore(release): consume pending changesets",
    "-m",
    marker,
  );
  const revision = git(directory, "rev-parse", "HEAD");
  // A normal fast-forward push rejects a concurrent main update. Do not rebase
  // a prepared release: its plan must describe the exact committed source.
  git(directory, "push", "origin", "HEAD:refs/heads/main");
  return { ...manifest, revision };
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  assert.equal(process.env.GITHUB_REPOSITORY, "delinoio/apps");
  assert.equal(process.env.GITHUB_REF, "refs/heads/main");
  const directory = process.cwd();
  const result = prepareChangesets({
    directory,
    runId: process.env.GITHUB_RUN_ID,
    load: () => ({
      records:
        JSON.parse(
          run(directory, "go", ["run", "./tools/changeset", "status"]),
        ) ?? [],
      inventory: JSON.parse(
        run(directory, "go", ["run", "./tools/changeset", "packages"]),
      ),
    }),
  });
  const outputs = {
    revision: result.revision,
    core: result.core,
    servers: JSON.stringify(result.servers),
    web: JSON.stringify(result.web),
  };
  console.log(JSON.stringify({ event: "changeset_release", ...outputs }));
  if (process.env.GITHUB_OUTPUT)
    appendFileSync(
      process.env.GITHUB_OUTPUT,
      Object.entries(outputs)
        .map(([key, value]) => `${key}=${value}\n`)
        .join(""),
    );
  if (process.env.GITHUB_STEP_SUMMARY)
    appendFileSync(
      process.env.GITHUB_STEP_SUMMARY,
      `Source revision: ${result.revision}\n\n\`\`\`json\n${JSON.stringify(result, null, 2)}\n\`\`\`\n\nImage publication does not establish runtime deployment or migration acceptance.\n`,
    );
}
