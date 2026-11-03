// SPDX-License-Identifier: Apache-2.0
// Packages the AnyDash CLI for the selected platforms into packages/anydash/dist:
// direct archives, platform npm packages, the launcher package and SHA256SUMS.
// `--no-compile` packages executables that a release job already built and
// signed under `<target-dir>/<triple>/release/`.
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { parseArgs } from "node:util";
import { executableName, platforms } from "../bin/verify.mjs";
import { collectNotices } from "./notices.mjs";
import {
  packArtifact,
  packLauncher,
  packageRoot,
  readPackage,
  repositoryRoot,
  stageArtifact,
  writeChecksums,
} from "./package.mjs";
import { targets } from "./targets.mjs";

const { values } = parseArgs({
  options: {
    platform: { type: "string", multiple: true },
    "no-compile": { type: "boolean", default: false },
    "target-dir": { type: "string" },
  },
});
const selected = values.platform ?? platforms;
if (
  !selected.length ||
  new Set(selected).size !== selected.length ||
  selected.some((key) => !platforms.includes(key))
)
  throw new Error("select distinct supported platforms");
const targetDirectory = resolve(
  repositoryRoot,
  values["target-dir"] ?? "target",
);
const git = (...args) =>
  execFileSync("git", args, { cwd: repositoryRoot, encoding: "utf8" }).trim();
const inputs = [
  "cmds/anydash",
  "Cargo.toml",
  "Cargo.lock",
  ".cargo",
  "rust-toolchain",
  "protos/anydash",
  "packages/anydash",
  "LICENSE",
  "NOTICE",
];
const sourceState = () =>
  git("status", "--porcelain", "--untracked-files=all", "--", ...inputs);
if (sourceState())
  throw new Error("CLI and launcher inputs must be committed before packaging");
const revision = git("rev-parse", "HEAD");
const pkg = readPackage();
const cargo = readFileSync(
  resolve(repositoryRoot, "cmds/anydash/Cargo.toml"),
  "utf8",
);
if (
  !new RegExp(`^version = "${pkg.version.replaceAll(".", "\\.")}"$`, "m").test(
    cargo,
  )
)
  throw new Error("Rust CLI and npm package versions must match");

const dist = resolve(packageRoot, "dist");
rmSync(dist, { recursive: true, force: true });
mkdirSync(dist, { recursive: true });
const notices = resolve(dist, "notices");
const modules = collectNotices(repositoryRoot, notices, selected);
console.info(
  JSON.stringify({
    event: "anydash_notices_collected",
    modules: modules.length,
  }),
);
const manifest = {
  schemaVersion: 1,
  version: pkg.version,
  sourceRevision: revision,
  artifacts: {},
};
const files = [];
for (const key of selected) {
  const target = targets[key];
  if (!values["no-compile"])
    execFileSync(
      "cargo",
      [
        "build",
        "--locked",
        "--release",
        "-p",
        "anydash",
        "--target",
        target,
        "--target-dir",
        targetDirectory,
      ],
      { cwd: repositoryRoot, stdio: "inherit" },
    );
  const directory = resolve(dist, "artifacts", `anydash-${key}`);
  manifest.artifacts[key] = stageArtifact({
    key,
    version: pkg.version,
    revision,
    executable: resolve(
      targetDirectory,
      target,
      "release",
      executableName(key),
    ),
    notices,
    directory,
  });
  files.push(
    ...packArtifact({
      key,
      version: pkg.version,
      directory,
      destination: dist,
      stage: resolve(dist, "platforms", `anydash-${key}`),
    }),
  );
  console.info(
    JSON.stringify({
      event: "anydash_cli_packaged",
      platform: key,
      sha256: manifest.artifacts[key].sha256,
    }),
  );
}
if (sourceState() || git("rev-parse", "HEAD") !== revision)
  throw new Error("CLI source changed during packaging; discard and rebuild");
files.push(
  packLauncher({
    manifest,
    destination: dist,
    stage: resolve(dist, "package"),
  }),
);
writeChecksums(dist, files);
writeFileSync(
  resolve(dist, "build.json"),
  `${JSON.stringify({ ...manifest, files: files.sort() }, null, 2)}\n`,
);
rmSync(notices, { recursive: true, force: true });
