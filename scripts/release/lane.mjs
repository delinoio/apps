// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  cpSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";
import {
  platforms,
  platformKey,
  platformPackage,
} from "../../packages/lane/bin/verify.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const json = (path) => JSON.parse(readFileSync(path, "utf8"));
const sha256 = (path) =>
  createHash("sha256").update(readFileSync(path)).digest("hex");
const run = (command, args, options = {}) =>
  execFileSync(command, args, { encoding: "utf8", ...options });
const desktopPlatforms = ["darwin-amd64", "darwin-arm64", "windows-amd64"];
export function releaseIdentity(env = process.env) {
  assert.match(
    env.LANE_RELEASE_TAG ?? "",
    /^lane@v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/,
  );
  assert.match(env.LANE_SOURCE_REVISION ?? "", /^[a-f0-9]{40}$/);
  return { tag: env.LANE_RELEASE_TAG, revision: env.LANE_SOURCE_REVISION };
}
function checksums(directory, files) {
  writeFileSync(
    resolve(directory, "SHA256SUMS"),
    files
      .sort()
      .map((name) => `${sha256(resolve(directory, name))}  ${name}\n`)
      .join(""),
  );
}

function directArchive(directory, artifact, destination) {
  const files = [];
  function visit(relative) {
    for (const name of readdirSync(resolve(directory, relative)).sort()) {
      const path = `${relative}/${name}`;
      const file = resolve(directory, path);
      const stat = lstatSync(file);
      if (stat.isDirectory()) visit(path);
      else {
        assert.ok(stat.isFile(), "direct archives require regular files");
        // cpSync assigns new mtimes. Freeze them before tar reads the files so
        // reassembling identical candidates can reuse immutable published assets.
        utimesSync(file, 0, 0);
        files.push(path);
      }
    }
  }
  visit(artifact);
  const tar = resolve(directory, `${artifact}.tar`);
  const ownership =
    process.platform === "darwin"
      ? ["--uid", "0", "--gid", "0", "--uname", "root", "--gname", "root"]
      : ["--owner=0", "--group=0", "--numeric-owner"];
  // Enumerate only files in sorted order: implicit parent directories are
  // recreated on extraction without capturing their fresh copy timestamps.
  run(
    "tar",
    ["-cf", tar, "--format=ustar", ...ownership, "-C", directory, ...files],
    {
      env: { ...process.env, COPYFILE_DISABLE: "1" },
    },
  );
  // Node gzip fixes its header timestamp; do not depend on the host tar's gzip.
  writeFileSync(destination, gzipSync(readFileSync(tar), { level: 9 }));
  rmSync(tar);
}

// Release candidates are not installed npm dependencies. Verify each unpacked
// platform archive directly, then let runtime installation retain host selection.
export function verifiedPlatformCandidate(
  directory,
  manifest,
  platform,
  identity,
) {
  const info = platformPackage(platform);
  const metadata = json(resolve(directory, "package.json"));
  assert.equal(metadata.name, info.name);
  assert.equal(metadata.version, manifest.version);
  assert.deepEqual(metadata.os, [info.os]);
  assert.deepEqual(metadata.cpu, [info.cpu]);
  const entry = manifest.artifacts[platform];
  const name = platform.startsWith("windows-") ? "lane.exe" : "lane";
  assert.equal(entry.path, name);
  assert.equal(entry.version, manifest.version);
  assert.equal(entry.platform, platform);
  assert.equal(manifest.sourceRevision, identity.revision);
  assert.equal(manifest.launcherRevision, identity.revision);
  assert.deepEqual(json(resolve(directory, "manifest.json")), {
    ...entry,
    sourceRevision: identity.revision,
  });
  const file = resolve(directory, name);
  assert.ok(
    lstatSync(file).isFile() &&
      realpathSync(file).startsWith(`${realpathSync(directory)}${sep}`),
    "candidate executable must be a regular packaged file",
  );
  const bytes = readFileSync(file);
  if (bytes.length !== entry.size || sha256(file) !== entry.sha256)
    throw new Error(`checksum mismatch for ${platform}`);
  for (const notice of [
    "LICENSE",
    "NOTICE",
    "THIRD_PARTY_NOTICES/modules.json",
  ])
    assert.ok(
      existsSync(resolve(directory, notice)),
      `missing platform notice: ${notice}`,
    );
  return file;
}

export function packageDesktop(destination, identity = releaseIdentity()) {
  const platform = platformKey();
  assert.ok(desktopPlatforms.includes(platform));
  const mac = platform.startsWith("darwin-");
  const build = resolve(root, "apps/lane/native/desktop/build");
  const app = resolve(build, mac ? "Lane.app" : "application");
  const resources = resolve(
    app,
    mac ? "Contents/Resources/app" : "resources/app",
  );
  const desktopConfig = json(resolve(resources, "package.json"));
  assert.equal(desktopConfig.name, "lane");
  assert.equal(desktopConfig.scheme, "lane");
  assert.equal(desktopConfig.testing, false);
  assert.equal(desktopConfig.releaseTag, identity.tag);
  assert.equal(desktopConfig.sourceRevision, identity.revision);
  assert.ok(
    existsSync(resolve(resources, "update-install.mjs")),
    "missing update installer",
  );
  const source = resolve(root, "packages/lane/dist/package");
  assert.equal(
    json(resolve(source, "manifest.json")).sourceRevision,
    identity.revision,
  );
  const helper = resolve(resources, "helpers", mac ? "lane" : "lane.exe");
  assert.equal(
    sha256(helper),
    sha256(
      verifiedPlatformCandidate(
        resolve(root, "packages/lane/dist/platforms", `lane-${platform}`),
        json(resolve(source, "manifest.json")),
        platform,
        identity,
      ),
    ),
    "desktop must embed the matching verified CLI",
  );
  for (const path of [
    mac ? "Contents/MacOS/lynxtron" : "lynxtron.exe",
    `${mac ? "Contents/Resources/app" : "resources/app"}/lane_host.node`,
  ])
    assert.ok(existsSync(resolve(app, path)), `missing desktop file: ${path}`);
  for (const name of ["LICENSE", "NOTICE", "THIRD_PARTY_NOTICES"])
    assert.ok(
      existsSync(resolve(resources, "helpers", name)),
      `missing helper notice: ${name}`,
    );
  mkdirSync(destination, { recursive: true });
  const name = `lane-desktop-${platform}-${identity.tag.replace("lane@", "")}.tar.gz`;
  run("tar", [
    "-czf",
    resolve(destination, name),
    "-C",
    build,
    mac ? "Lane.app" : "application",
  ]);
  writeFileSync(
    resolve(destination, `desktop-${platform}.json`),
    JSON.stringify(
      {
        ...identity,
        platform,
        archive: name,
        sha256: sha256(resolve(destination, name)),
        signing: mac ? "ad-hoc" : "unsigned",
        cliVersion: json(resolve(source, "package.json")).version,
      },
      null,
      2,
    ) + "\n",
  );
  console.info(
    JSON.stringify({
      event: "lane_desktop_packaged",
      platform,
      revision: identity.revision,
      archive: name,
    }),
  );
}

// Every host uploads its npm archive rather than loose executable files. Tar
// preserves executable modes and macOS framework symlinks across artifact storage.
export function assemble(inputs, destination, identity = releaseIdentity()) {
  assert.ok(!existsSync(destination), "release destination must be new");
  mkdirSync(destination, { recursive: true });
  const temporary = mkdtempSync(resolve(tmpdir(), "lane-release-"));
  try {
    const stage = resolve(temporary, "package");
    const seen = new Set();
    let combined;
    const files = [];
    const desktops = new Map();
    for (const directory of readdirSync(inputs).sort()) {
      const candidate = resolve(inputs, directory);
      const archives = readdirSync(candidate).filter((name) =>
        /^delinoio-lane-[0-9.]+\.tgz$/.test(name),
      );
      assert.equal(
        archives.length,
        1,
        "each host must supply one npm candidate",
      );
      const extracted = resolve(temporary, directory);
      mkdirSync(extracted);
      run("tar", ["-xzf", resolve(candidate, archives[0]), "-C", extracted]);
      const pkg = resolve(extracted, "package");
      const manifest = json(resolve(pkg, "manifest.json"));
      assert.equal(manifest.sourceRevision, identity.revision);
      assert.equal(manifest.launcherRevision, identity.revision);
      if (!combined) {
        combined = { ...manifest, artifacts: {} };
        cpSync(pkg, stage, { recursive: true });
        rmSync(resolve(stage, "artifacts"), { recursive: true, force: true });
      }
      assert.equal(manifest.version, combined.version);
      assert.equal(
        json(resolve(pkg, "package.json")).version,
        combined.version,
      );
      assert.equal(manifest.schemaVersion, 1);
      assert.deepEqual(
        json(resolve(pkg, "package.json")).optionalDependencies,
        Object.fromEntries(
          platforms.map((key) => [platformPackage(key).name, manifest.version]),
        ),
      );
      for (const platform of Object.keys(manifest.artifacts)) {
        assert.ok(
          platforms.includes(platform) && !seen.has(platform),
          "unexpected or duplicate target",
        );
        seen.add(platform);
        const artifact = `lane-${platform}`;
        const npmArchive = `delinoio-${artifact}-${combined.version}.tgz`;
        const platformStage = resolve(extracted, artifact);
        mkdirSync(platformStage);
        run("tar", [
          "-xzf",
          resolve(candidate, npmArchive),
          "-C",
          platformStage,
        ]);
        const platformRoot = resolve(platformStage, "package");
        verifiedPlatformCandidate(platformRoot, manifest, platform, identity);
        cpSync(
          resolve(candidate, npmArchive),
          resolve(destination, npmArchive),
        );
        files.push(npmArchive);
        const direct = resolve(extracted, "direct");
        cpSync(platformRoot, resolve(direct, artifact), { recursive: true });
        combined.artifacts[platform] = manifest.artifacts[platform];
        const archive = `${artifact}-${combined.version}.tar.gz`;
        directArchive(direct, artifact, resolve(destination, archive));
        files.push(archive);
      }
      for (const name of readdirSync(candidate).filter((name) =>
        /^desktop-.*\.json$/.test(name),
      )) {
        const desktop = json(resolve(candidate, name));
        assert.ok(
          desktopPlatforms.includes(desktop.platform) &&
            !desktops.has(desktop.platform),
        );
        assert.equal(desktop.tag, identity.tag);
        assert.equal(desktop.revision, identity.revision);
        assert.equal(desktop.cliVersion, combined.version);
        assert.equal(
          desktop.archive,
          `lane-desktop-${desktop.platform}-${identity.tag.replace("lane@", "")}.tar.gz`,
        );
        assert.equal(
          sha256(resolve(candidate, desktop.archive)),
          desktop.sha256,
        );
        cpSync(
          resolve(candidate, desktop.archive),
          resolve(destination, desktop.archive),
        );
        files.push(desktop.archive);
        desktops.set(desktop.platform, desktop);
      }
    }
    assert.deepEqual(
      [...seen].sort(),
      [...platforms].sort(),
      "all six CLI targets are required",
    );
    assert.deepEqual(
      [...desktops.keys()].sort(),
      [...desktopPlatforms].sort(),
      "all three desktop targets are required",
    );
    writeFileSync(
      resolve(stage, "manifest.json"),
      JSON.stringify(combined, null, 2) + "\n",
    );
    run(
      "npm",
      [
        "pack",
        "--ignore-scripts",
        "--pack-destination",
        resolve(destination),
        "--quiet",
      ],
      { cwd: stage },
    );
    files.push(`delinoio-lane-${combined.version}.tgz`);
    writeFileSync(
      resolve(destination, "lane-release.json"),
      JSON.stringify(
        {
          ...identity,
          cliVersion: combined.version,
          artifacts: combined.artifacts,
          desktops: [...desktops.values()],
        },
        null,
        2,
      ) + "\n",
    );
    files.push("lane-release.json");
    checksums(destination, files);
    console.info(
      JSON.stringify({
        event: "lane_release_assembled",
        revision: identity.revision,
        cliTargets: seen.size,
        desktopTargets: desktops.size,
      }),
    );
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
}

export function publish(directory, identity = releaseIdentity()) {
  const record = json(resolve(directory, "lane-release.json"));
  assert.equal(record.tag, identity.tag);
  assert.equal(record.revision, identity.revision);
  assert.match(record.cliVersion, /^\d+\.\d+\.\d+$/);
  assert.deepEqual(Object.keys(record.artifacts).sort(), [...platforms].sort());
  assert.deepEqual(
    record.desktops.map((entry) => entry.platform).sort(),
    [...desktopPlatforms].sort(),
  );
  const files = readFileSync(resolve(directory, "SHA256SUMS"), "utf8")
    .trim()
    .split("\n")
    .map((line) => {
      const match = /^([a-f0-9]{64})  ([a-zA-Z0-9.-]+)$/.exec(line);
      assert.ok(match, "invalid release checksum");
      assert.equal(sha256(resolve(directory, match[2])), match[1]);
      return match[2];
    });
  files.push("SHA256SUMS");
  assert.equal(new Set(files).size, files.length);
  assert.deepEqual(
    [...files].sort(),
    [
      ...platforms.map(
        (platform) => `lane-${platform}-${record.cliVersion}.tar.gz`,
      ),
      ...platforms.map(
        (platform) => `delinoio-lane-${platform}-${record.cliVersion}.tgz`,
      ),
      ...desktopPlatforms.map(
        (platform) =>
          `lane-desktop-${platform}-${identity.tag.replace("lane@", "")}.tar.gz`,
      ),
      `delinoio-lane-${record.cliVersion}.tgz`,
      "lane-release.json",
      "SHA256SUMS",
    ].sort(),
    "checksum set must cover every release artifact",
  );
  const repo = "delinoio/apps";
  // The tag endpoint covers published releases. List authenticated releases so
  // partial draft uploads can be resumed before the success tag exists.
  const findRelease = () =>
    JSON.parse(
      run("gh", [
        "api",
        "--paginate",
        "--slurp",
        `repos/${repo}/releases?per_page=100`,
      ]),
    )
      .flat()
      .find((entry) => entry.tag_name === identity.tag);
  let release = findRelease();
  if (!release) {
    run("gh", [
      "release",
      "create",
      identity.tag,
      "--repo",
      repo,
      "--target",
      identity.revision,
      "--draft",
      "--title",
      `Lane ${identity.tag.replace("lane@", "")}`,
      "--notes",
      `Source: ${identity.revision}\nCLI: ${record.cliVersion}; six targets. Desktop: macOS Intel/Apple Silicon (ad-hoc signed), Windows x64 (unsigned portable archive). Native installation, device behavior, signing/notarization and store acceptance are separate. The npm archive is downloadable; npm registry publication is not performed.`,
    ]);
    release = findRelease();
  }
  assert.ok(release, "Draft release was not returned after creation");
  assert.equal(
    release.target_commitish,
    identity.revision,
    "Release source mismatch",
  );
  const temporary = mkdtempSync(resolve(tmpdir(), "lane-published-"));
  try {
    for (const name of files) {
      if (release.assets.some((asset) => asset.name === name)) {
        run("gh", [
          "release",
          "download",
          identity.tag,
          "--repo",
          repo,
          "--pattern",
          name,
          "--dir",
          temporary,
        ]);
        assert.equal(
          sha256(resolve(temporary, name)),
          sha256(resolve(directory, name)),
          `published asset differs: ${name}; never overwrite an immutable release asset`,
        );
        console.info(
          JSON.stringify({
            event: "lane_release_asset_reused",
            tag: identity.tag,
            name,
          }),
        );
      } else {
        run("gh", [
          "release",
          "upload",
          identity.tag,
          "--repo",
          repo,
          resolve(directory, name),
        ]);
        console.info(
          JSON.stringify({
            event: "lane_release_asset_uploaded",
            tag: identity.tag,
            name,
          }),
        );
      }
    }
    console.info(
      JSON.stringify({
        event: "lane_release_assets_staged",
        tag: identity.tag,
        revision: identity.revision,
        assets: files.length,
      }),
    );
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  const [command, input, output] = process.argv.slice(2);
  if (command === "desktop") packageDesktop(resolve(input));
  else if (command === "assemble") assemble(resolve(input), resolve(output));
  else if (command === "publish") publish(resolve(input));
  else throw new Error("Expected desktop, assemble, or publish");
}
