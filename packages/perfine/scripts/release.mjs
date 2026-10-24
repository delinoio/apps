// SPDX-License-Identifier: Apache-2.0
import { execFileSync } from "node:child_process";
import {
  mkdtempSync,
  readFileSync,
  writeFileSync,
  readdirSync,
  rmSync,
  cpSync,
  appendFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { commit, version, root, sha256 } from "./build.mjs";
export const distribution = "https://github.com/delinoio/perfine-action.git";
const git = (cwd, ...args) =>
  execFileSync("git", args, {
    cwd,
    encoding: "utf8",
    maxBuffer: 128 * 1024 * 1024,
  }).trim();
export function exactTag(tag) {
  if (!tag?.startsWith("v"))
    throw new Error("Exact vX.Y.Z release tag required");
  version(tag.slice(1));
  return tag;
}
export function inventory(directory) {
  const result = {};
  function walk(relative = "") {
    for (const item of readdirSync(join(directory, relative), {
      withFileTypes: true,
    })) {
      if (item.name === "dist" || item.name === ".git")
        throw new Error(
          "Generated dist or Git metadata cannot enter a release",
        );
      const path = relative ? `${relative}/${item.name}` : item.name;
      if (item.isDirectory()) walk(path);
      else if (item.isFile())
        result[path] = sha256(readFileSync(join(directory, path)));
      else throw new Error("Release symlinks are forbidden");
    }
  }
  walk();
  return Object.fromEntries(Object.entries(result).sort());
}
export function verifyTree(cwd, sha, tag, expectedSource) {
  commit(sha);
  exactTag(tag);
  commit(expectedSource);
  const names = git(cwd, "ls-tree", "-r", "--name-only", sha).split("\n");
  const entries = git(cwd, "ls-tree", "-r", sha).split("\n");
  if (
    entries.some((line) => !/^100(644|755) blob /.test(line)) ||
    !entries.some(
      (line) =>
        line.startsWith("100755 blob ") && line.endsWith("\truntime/perfine"),
    )
  )
    throw new Error(
      "Distribution requires regular files and executable runtime",
    );
  const required = [
    "ACTION_NOTICES",
    "LICENSE",
    "NOTICE",
    "README.md",
    "THIRD_PARTY_NOTICES",
    "action.cjs",
    "action.yml",
    "provenance.json",
    "runtime/SHA256SUMS",
    "runtime/perfine",
  ];
  if (JSON.stringify(names.sort()) !== JSON.stringify(required.sort()))
    throw new Error("Unexpected Action distribution layout");
  const bytes = (name) =>
    execFileSync("git", ["show", `${sha}:${name}`], {
      cwd,
      maxBuffer: 128 * 1024 * 1024,
    });
  const provenance = JSON.parse(bytes("provenance.json"));
  if (
    provenance.schemaVersion !== 1 ||
    provenance.sourceRepository !== "delinoio/apps" ||
    provenance.sourceCommit !== expectedSource ||
    provenance.version !== tag.slice(1) ||
    provenance.target !== "x86_64-unknown-linux-gnu"
  )
    throw new Error("Release provenance mismatch");
  const checksum = sha256(bytes("runtime/perfine"));
  if (
    provenance.binarySha256 !== checksum ||
    bytes("runtime/SHA256SUMS").toString() !== `${checksum}  perfine\n`
  )
    throw new Error("Release checksum mismatch");
  if (
    !bytes("action.yml").toString().includes("main: action.cjs") ||
    !bytes("action.cjs").length
  )
    throw new Error("Action entry point missing");
  for (const name of [
    "LICENSE",
    "NOTICE",
    "THIRD_PARTY_NOTICES",
    "ACTION_NOTICES",
  ])
    if (!bytes(name).length) throw new Error("Missing license notices");
  return {
    schemaVersion: 1,
    repository: "delinoio/perfine-action",
    tag,
    actionCommit: sha,
    sourceCommit: expectedSource,
    version: tag.slice(1),
  };
}
export function verifyRemote({ tag, expectedCommit, sourceCommit }) {
  exactTag(tag);
  commit(expectedCommit);
  commit(sourceCommit);
  const directory = mkdtempSync(join(tmpdir(), "perfine-verify-"));
  try {
    git(directory, "init", "--bare");
    git(
      directory,
      "fetch",
      "--no-tags",
      "--depth=1",
      distribution,
      `refs/tags/${tag}`,
    );
    const actual = git(directory, "rev-parse", "FETCH_HEAD^{commit}");
    if (actual !== expectedCommit)
      throw new Error(
        "Distribution tag does not resolve to the exact expected commit",
      );
    return verifyTree(directory, actual, tag, sourceCommit);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}
export function source(tag) {
  exactTag(tag);
  const sourceTag = `perfine@${tag}`;
  const sha = commit(git(root, "rev-parse", `refs/tags/${sourceTag}^{commit}`));
  git(root, "merge-base", "--is-ancestor", sha, "origin/main");
  const expected = tag.slice(1);
  for (const path of [
    "crates/perfine-core/Cargo.toml",
    "cmds/perfine/Cargo.toml",
  ]) {
    const toml = git(root, "show", `${sha}:${path}`);
    if (toml.match(/^version\s*=\s*"([^"]+)"/m)?.[1] !== expected)
      throw new Error("Rust release versions must agree");
  }
  for (const path of [
    "packages/perfine/package.json",
    "actions/perfine/package.json",
  ])
    if (JSON.parse(git(root, "show", `${sha}:${path}`)).version !== expected)
      throw new Error("Distribution versions must agree");
  return sha;
}
function publishAction(tag, sourceCommit, directory) {
  const staged = resolve(directory, "action");
  const expected = inventory(staged);
  const temporary = mkdtempSync(join(tmpdir(), "perfine-publish-"));
  try {
    git(temporary, "init");
    git(temporary, "remote", "add", "origin", distribution);
    const existing = git(
      temporary,
      "ls-remote",
      "--tags",
      "origin",
      `refs/tags/${tag}`,
      `refs/tags/${tag}^{}`,
    );
    if (existing) {
      git(temporary, "fetch", "--no-tags", "origin", `refs/tags/${tag}`);
      const sha = commit(git(temporary, "rev-parse", "FETCH_HEAD^{commit}"));
      verifyTree(temporary, sha, tag, sourceCommit);
      for (const [name, hash] of Object.entries(expected))
        if (
          sha256(
            execFileSync("git", ["show", `${sha}:${name}`], {
              cwd: temporary,
              maxBuffer: 128 * 1024 * 1024,
            }),
          ) !== hash
        )
          throw new Error(
            "Existing immutable tag has different content; do not replace it",
          );
      return verifyRemote({ tag, expectedCommit: sha, sourceCommit });
    }
    cpSync(staged, temporary, { recursive: true });
    git(temporary, "add", ".");
    git(
      temporary,
      "-c",
      "user.name=Perfine release",
      "-c",
      "user.email=release@delino.io",
      "-c",
      "commit.gpgsign=false",
      "commit",
      "-m",
      `release: perfine ${tag} from ${sourceCommit}`,
    );
    const sha = commit(git(temporary, "rev-parse", "HEAD"));
    verifyTree(temporary, sha, tag, sourceCommit);
    git(temporary, "tag", tag, sha);
    // Only a new exact tag is pushed. No force, branch update, or moving major tag.
    git(
      temporary,
      "push",
      "--atomic",
      "origin",
      `refs/tags/${tag}:refs/tags/${tag}`,
    );
    return verifyRemote({ tag, expectedCommit: sha, sourceCommit });
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
}
export function main(env = process.env, command = process.argv[2]) {
  const tag = exactTag(env.PERFINE_RELEASE_TAG);
  if (command === "source") {
    const sha = source(tag);
    if (env.GITHUB_OUTPUT)
      appendFileSync(
        env.GITHUB_OUTPUT,
        `revision=${sha}\nversion=${tag.slice(1)}\n`,
      );
    return { sourceCommit: sha };
  }
  if (command === "verify")
    return verifyRemote({
      tag,
      expectedCommit: env.PERFINE_ACTION_COMMIT,
      sourceCommit: env.PERFINE_SOURCE_COMMIT,
    });
  if (command !== "publish-action" || env.PERFINE_APPROVE_RELEASE !== "true")
    throw new Error("Explicit approved release required");
  const sourceCommit = commit(env.PERFINE_SOURCE_COMMIT);
  const record = publishAction(
    tag,
    sourceCommit,
    env.PERFINE_RELEASE_DIRECTORY,
  );
  writeFileSync(
    join(env.PERFINE_RELEASE_DIRECTORY, "action-release.json"),
    `${JSON.stringify(record, null, 2)}\n`,
  );
  if (env.GITHUB_OUTPUT)
    appendFileSync(env.GITHUB_OUTPUT, `action-commit=${record.actionCommit}\n`);
  return record;
}
if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  try {
    console.log(JSON.stringify(main()));
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
