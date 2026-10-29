// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
const gh = (...args) => execFileSync("gh", args, { encoding: "utf8" }).trim();
const {
  RELEASE_TAG: tag,
  RELEASE_REVISION: revision,
  RELEASE_DIRECTORY: directory,
} = process.env;
assert.match(tag ?? "", /^perfine@v\d+\.\d+\.\d+$/);
assert.match(revision ?? "", /^[a-f0-9]{40}$/);
const repo = "delinoio/apps";
const releases = JSON.parse(
  gh("api", "--paginate", "--slurp", `repos/${repo}/releases?per_page=100`),
).flat();
let release = releases.find((r) => r.tag_name === tag);
if (!release) {
  gh(
    "release",
    "create",
    tag,
    "--repo",
    repo,
    "--draft",
    "--target",
    revision,
    "--title",
    `Perfine ${tag}`,
    "--notes-file",
    join(directory, "action-release.json"),
  );
  release = JSON.parse(
    gh("api", "--paginate", "--slurp", `repos/${repo}/releases?per_page=100`),
  )
    .flat()
    .find((r) => r.tag_name === tag);
}
assert.ok(release);
assert.equal(release.target_commitish, revision, "Release source mismatch");
const temporary = mkdtempSync(join(tmpdir(), "release-assets-"));
try {
  for (const name of readdirSync(directory).filter(
    (n) =>
      /\.(tar\.gz|tgz)$/.test(n) ||
      ["SHA256SUMS", "release-set.json", "action-release.json"].includes(n),
  )) {
    if (release.assets.some((a) => a.name === name)) {
      gh(
        "release",
        "download",
        tag,
        "--repo",
        repo,
        "--pattern",
        name,
        "--dir",
        temporary,
      );
      assert.ok(
        readFileSync(join(directory, name)).equals(
          readFileSync(join(temporary, name)),
        ),
        `Published asset differs: ${name}`,
      );
    } else gh("release", "upload", tag, "--repo", repo, join(directory, name));
  }
} finally {
  rmSync(temporary, { recursive: true, force: true });
}
