// SPDX-License-Identifier: Apache-2.0
import { execFileSync } from "node:child_process";
import { appendFileSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const Project = Object.freeze({
  Devbase: "devbase",
  DelinoSite: "delino-site",
  Delitools: "delitools",
  Fixkey: "fixkey",
});
export const Bump = Object.freeze({
  Patch: "patch",
  Minor: "minor",
  Major: "major",
});
const nativePackages = Object.freeze({
  delitools: "delitools-shell",
  fixkey: "fixkey",
});
const stable = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/u;
function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}
function requireProject(project) {
  requireValue(
    Object.values(Project).includes(project),
    "Unknown release project",
  );
}
export function bumpVersion(version, bump) {
  requireValue(
    stable.test(version),
    "Expected a stable MAJOR.MINOR.PATCH version",
  );
  requireValue(Object.values(Bump).includes(bump), "Unknown version bump");
  const parts = version.split(".").map(BigInt);
  const index = { major: 0, minor: 1, patch: 2 }[bump];
  parts[index] += 1n;
  for (let i = index + 1; i < 3; i++) parts[i] = 0n;
  requireValue(
    parts.every((part) => part <= 2147483647n),
    "Version component overflow",
  );
  return parts.join(".");
}
export function releaseTag(project, version) {
  requireProject(project);
  requireValue(stable.test(version), "Invalid release version");
  // FixKey's updater already consumes this tag namespace. Keep it stable.
  return project === Project.Fixkey
    ? `fixkey-v${version}`
    : `${project}@v${version}`;
}
export function versionChanges(project, bump, read) {
  requireProject(project);
  const packageFile = `apps/${project}/package.json`;
  const source = read(packageFile);
  const manifest = JSON.parse(source);
  requireValue(
    manifest.name === (project === Project.Fixkey ? "fixkey-desktop" : project),
    "Package identity mismatch",
  );
  const previous = manifest.version;
  const version = bumpVersion(previous, bump);
  const changes = {};
  const replaceJson = (file) => {
    const content = read(file);
    requireValue(
      JSON.parse(content).version === previous,
      "Source versions disagree",
    );
    const matches = [...content.matchAll(/^  "version": "([^"]+)"/gmu)];
    requireValue(matches.length === 1, "Ambiguous JSON version field");
    changes[file] = content.replace(
      /^  "version": "[^"]+"/mu,
      `  "version": "${version}"`,
    );
  };
  replaceJson(packageFile);
  const native = nativePackages[project];
  if (native) {
    replaceJson(`apps/${project}/src-tauri/tauri.conf.json`);
    const file = `apps/${project}/src-tauri/Cargo.toml`;
    const content = read(file);
    const section = content.match(
      /^\[package\]\n[\s\S]*?(?=^\[|$(?![\s\S]))/mu,
    )?.[0];
    requireValue(
      section?.includes(`name = "${native}"\n`) &&
        section.includes(`version = "${previous}"\n`),
      "Native package version mismatch",
    );
    changes[file] = content.replace(
      section,
      section.replace(`version = "${previous}"`, `version = "${version}"`),
    );
    const lock = read("Cargo.lock");
    const entries = [
      ...lock.matchAll(/^\[\[package\]\]\n[\s\S]*?(?=^\[\[|$(?![\s\S]))/gmu),
    ].filter(([entry]) => entry.includes(`\nname = "${native}"\n`));
    requireValue(
      entries.length === 1 &&
        !/^source = /mu.test(entries[0][0]) &&
        entries[0][0].includes(`version = "${previous}"\n`),
      "Workspace lock version mismatch",
    );
    requireValue(
      !lock.includes(` "${native} ${previous}`),
      "Version-qualified workspace dependency requires an explicit release contract",
    );
    changes["Cargo.lock"] = lock.replace(
      entries[0][0],
      entries[0][0].replace(
        `version = "${previous}"`,
        `version = "${version}"`,
      ),
    );
  }
  return {
    project,
    previous,
    version,
    tag: releaseTag(project, version),
    changes,
  };
}
const git = (directory, ...args) =>
  execFileSync("git", args, { cwd: directory, encoding: "utf8" }).trim();

export function prepareRelease({ directory, project, bump, runId }) {
  requireProject(project);
  requireValue(Object.values(Bump).includes(bump), "Unknown version bump");
  requireValue(/^[1-9]\d*$/u.test(runId), "Invalid workflow run ID");
  requireValue(
    !git(directory, "status", "--porcelain"),
    "Release requires a clean checkout",
  );
  git(directory, "fetch", "origin", "main", "--tags");
  const marker = `Release-Run: ${runId}`;
  const revisions = git(
    directory,
    "log",
    "origin/main",
    "--format=%H",
    "--extended-regexp",
    `--grep=^${marker}$`,
  )
    .split("\n")
    .filter(Boolean);
  requireValue(revisions.length <= 1, "Ambiguous release run history");
  if (revisions.length) {
    const revision = revisions[0];
    const message = git(directory, "show", "-s", "--format=%B", revision);
    const match = message.match(
      new RegExp(`^chore\\(${project}\\): release ([0-9.]+)$`, "mu"),
    );
    requireValue(
      match &&
        message.split("\n").includes(marker) &&
        message.split("\n").includes(`Release-Bump: ${bump}`),
      "Release run inputs changed",
    );
    const plan = versionChanges(
      project,
      bump,
      (file) => git(directory, "show", `${revision}^:${file}`) + "\n",
    );
    requireValue(plan.version === match[1], "Release run version mismatch");
    const changed = git(
      directory,
      "diff-tree",
      "--no-commit-id",
      "--name-only",
      "-r",
      revision,
    )
      .split("\n")
      .sort();
    requireValue(
      JSON.stringify(changed) ===
        JSON.stringify(Object.keys(plan.changes).sort()),
      "Release commit paths mismatch",
    );
    for (const [file, content] of Object.entries(plan.changes)) {
      requireValue(
        git(directory, "show", `${revision}:${file}`) === content.trim(),
        "Release commit content mismatch",
      );
    }
    requireValue(
      git(directory, "rev-parse", `refs/tags/${plan.tag}^{commit}`) ===
        revision,
      "Existing release tag does not match the recorded run",
    );
    return {
      project,
      previous: plan.previous,
      version: plan.version,
      tag: plan.tag,
      revision,
      resumed: true,
    };
  }
  requireValue(
    git(directory, "rev-parse", "HEAD") ===
      git(directory, "rev-parse", "origin/main"),
    "Main advanced; start a new release run",
  );
  const plan = versionChanges(project, bump, (file) =>
    readFileSync(resolve(directory, file), "utf8"),
  );
  requireValue(
    !git(directory, "tag", "--list", plan.tag),
    "Release tag already exists; never move a release tag",
  );
  for (const [file, content] of Object.entries(plan.changes))
    writeFileSync(resolve(directory, file), content);
  git(directory, "add", "--", ...Object.keys(plan.changes));
  git(
    directory,
    "commit",
    "-m",
    `chore(${project}): release ${plan.version}`,
    "-m",
    `${marker}\nRelease-Bump: ${bump}`,
  );
  const revision = git(directory, "rev-parse", "HEAD");
  git(directory, "tag", plan.tag, revision);
  // Atomic push prevents a rejected main update from leaving an orphan release
  // tag. Keep this requirement while source commits and tags share one release.
  git(
    directory,
    "push",
    "--atomic",
    "origin",
    "HEAD:refs/heads/main",
    `refs/tags/${plan.tag}:refs/tags/${plan.tag}`,
  );
  return {
    project,
    previous: plan.previous,
    version: plan.version,
    tag: plan.tag,
    revision,
    resumed: false,
  };
}

export function validateWebSource(directory, project, tag) {
  requireValue(
    [Project.Devbase, Project.DelinoSite, Project.Delitools].includes(project),
    "Unknown web release project",
  );
  requireValue(
    typeof tag === "string" &&
      tag.startsWith(`${project}@v`) &&
      stable.test(tag.slice(project.length + 2)),
    "Invalid web release tag",
  );
  git(
    directory,
    "fetch",
    "origin",
    "main",
    `refs/tags/${tag}:refs/tags/${tag}`,
  );
  const revision = git(directory, "rev-parse", `${tag}^{commit}`);
  git(directory, "merge-base", "--is-ancestor", revision, "origin/main");
  const manifest = JSON.parse(
    git(directory, "show", `${revision}:apps/${project}/package.json`),
  );
  requireValue(
    tag === releaseTag(project, manifest.version),
    "Tag and source version disagree",
  );
  return { project, version: manifest.version, tag, revision };
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  const directory = process.cwd();
  requireValue(
    process.env.GITHUB_REPOSITORY === "delinoio/apps",
    "Release is restricted to delinoio/apps",
  );
  const command = process.argv[2];
  let result;
  if (command === "prepare") {
    requireValue(
      process.env.GITHUB_REF === "refs/heads/main",
      "Release preparation is main-only",
    );
    result = prepareRelease({
      directory,
      project: process.env.RELEASE_PROJECT,
      bump: process.env.RELEASE_BUMP,
      runId: process.env.GITHUB_RUN_ID,
    });
  } else {
    requireValue(command === "validate-web", "Unknown release command");
    result = validateWebSource(
      directory,
      process.env.RELEASE_PROJECT,
      process.env.RELEASE_TAG,
    );
  }
  console.log(JSON.stringify({ event: "release_source", ...result }));
  if (process.env.GITHUB_OUTPUT)
    appendFileSync(
      process.env.GITHUB_OUTPUT,
      Object.entries(result)
        .map(([key, value]) => `${key}=${value}\n`)
        .join(""),
    );
  if (process.env.GITHUB_STEP_SUMMARY)
    appendFileSync(
      process.env.GITHUB_STEP_SUMMARY,
      `Project: ${result.project}\n\nVersion: ${result.version}\n\nTag: ${result.tag}\n\nSource revision: ${result.revision}\n\nSource preparation does not establish deployment, native or store acceptance.\n`,
    );
}
