// SPDX-License-Identifier: Apache-2.0
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  cpSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";
import { parseArgs } from "node:util";
import { platforms } from "../bin/verify.mjs";
import { targets } from "./targets.mjs";
import { collectNotices } from "./notices.mjs";
import { npm } from "./npm.mjs";
const root = fileURLToPath(new URL("../../../", import.meta.url));
const { values } = parseArgs({
  options: {
    "source-root": { type: "string" },
    platform: { type: "string", multiple: true },
  },
});
const selectedPlatforms = values.platform ?? platforms;
if (
  !selectedPlatforms.length ||
  new Set(selectedPlatforms).size !== selectedPlatforms.length ||
  selectedPlatforms.some((key) => !platforms.includes(key))
)
  throw new Error("select distinct supported platforms");
const cliRoot = values["source-root"] ? resolve(values["source-root"]) : root;
const sourceState = () =>
  execFileSync(
    "git",
    [
      "status",
      "--porcelain",
      "--untracked-files=all",
      "--",
      "cmds/lane",
      "Cargo.toml",
      "Cargo.lock",
      ".cargo",
      "apps/lane/native/store/schema.sql",
      "apps/lane/native/store/registry.sql",
      "protos",
    ],
    { cwd: cliRoot, encoding: "utf8" },
  );
if (sourceState())
  throw new Error("CLI build inputs must be committed before packaging");
const cliRevision = execFileSync("git", ["rev-parse", "HEAD"], {
  cwd: cliRoot,
  encoding: "utf8",
}).trim();
const launcherState = () =>
  execFileSync(
    "git",
    [
      "status",
      "--porcelain",
      "--untracked-files=all",
      "--",
      "packages/lane",
      "LICENSE",
      "NOTICE",
    ],
    { cwd: root, encoding: "utf8" },
  );
if (launcherState())
  throw new Error("Launcher build inputs must be committed before packaging");
const launcherRevision = execFileSync("git", ["rev-parse", "HEAD"], {
  cwd: root,
  encoding: "utf8",
}).trim();
const source = resolve(root, "packages/lane");
const dist = resolve(source, "dist");
const pkg = JSON.parse(readFileSync(resolve(source, "package.json")));
if (!/^\d+\.\d+\.\d+$/.test(pkg.version))
  throw new Error("invalid release version");
rmSync(dist, { recursive: true, force: true });
mkdirSync(dist, { recursive: true });
const stage = resolve(dist, "package");
mkdirSync(stage);
for (const name of ["bin", "README.md", "SKILL.md", "guides"])
  cpSync(resolve(source, name), resolve(stage, name), { recursive: true });
for (const name of ["LICENSE", "NOTICE"])
  cpSync(resolve(root, name), resolve(stage, name));
// Require reviewed dependency notices rather than silently ship a binary without them.
collectNotices(
  cliRoot,
  resolve(stage, "THIRD_PARTY_NOTICES"),
  selectedPlatforms,
);
const { scripts, ...published } = pkg;
writeFileSync(
  resolve(stage, "package.json"),
  `${JSON.stringify(published, null, 2)}\n`,
);
const cargoManifest = readFileSync(
  resolve(cliRoot, "cmds/lane/Cargo.toml"),
  "utf8",
);
if (!cargoManifest.includes(`version = "${pkg.version}"`))
  throw new Error("Rust CLI and npm package versions must match");
const manifest = {
  schemaVersion: 1,
  version: pkg.version,
  sourceRevision: cliRevision,
  launcherRevision,
  artifacts: {},
};
for (const key of selectedPlatforms) {
  const [os, arch] = key.split("-");
  const directory = `lane-${key}`;
  const name = os === "windows" ? "lane.exe" : "lane";
  const artifact = resolve(dist, "artifacts", directory);
  mkdirSync(artifact, { recursive: true });
  const target = targets[key];
  const cargoTarget = resolve(dist, "cargo");
  execFileSync(
    "cargo",
    [
      "build",
      "--locked",
      "--release",
      "-p",
      "lane",
      "--target",
      target,
      "--target-dir",
      cargoTarget,
    ],
    {
      cwd: cliRoot,
      stdio: "inherit",
    },
  );
  cpSync(
    resolve(cargoTarget, target, "release", name),
    resolve(artifact, name),
  );
  const bytes = readFileSync(resolve(artifact, name));
  const entry = {
    platform: key,
    version: pkg.version,
    path: `artifacts/${directory}/${name}`,
    size: bytes.length,
    sha256: createHash("sha256").update(bytes).digest("hex"),
  };
  manifest.artifacts[key] = entry;
  for (const notice of ["LICENSE", "NOTICE", "THIRD_PARTY_NOTICES"])
    cpSync(resolve(stage, notice), resolve(artifact, notice), {
      recursive: true,
    });
  writeFileSync(
    resolve(artifact, "manifest.json"),
    `${JSON.stringify({ ...entry, path: name, sourceRevision: manifest.sourceRevision }, null, 2)}\n`,
  );
  writeFileSync(resolve(artifact, "SHA256SUMS"), `${entry.sha256}  ${name}\n`);
  cpSync(artifact, resolve(stage, "artifacts", directory), { recursive: true });
  execFileSync("tar", [
    "-czf",
    resolve(dist, `${directory}-${pkg.version}.tar.gz`),
    "-C",
    resolve(dist, "artifacts"),
    directory,
  ]);
}
writeFileSync(
  resolve(stage, "manifest.json"),
  `${JSON.stringify(manifest, null, 2)}\n`,
);
if (
  sourceState() ||
  launcherState() ||
  execFileSync("git", ["rev-parse", "HEAD"], {
    cwd: root,
    encoding: "utf8",
  }).trim() !== launcherRevision ||
  execFileSync("git", ["rev-parse", "HEAD"], {
    cwd: cliRoot,
    encoding: "utf8",
  }).trim() !== cliRevision
)
  throw new Error(
    "CLI source changed during packaging; discard artifacts and rebuild",
  );
npm(["pack", "--ignore-scripts", "--pack-destination", dist, "--quiet"], {
  cwd: stage,
  stdio: "inherit",
});
const files = selectedPlatforms
  .map((key) => `lane-${key}-${pkg.version}.tar.gz`)
  .concat(`delinoio-lane-${pkg.version}.tgz`);
writeFileSync(
  resolve(dist, "SHA256SUMS"),
  files
    .map(
      (name) =>
        `${createHash("sha256")
          .update(readFileSync(resolve(dist, name)))
          .digest("hex")}  ${name}\n`,
    )
    .join(""),
);
