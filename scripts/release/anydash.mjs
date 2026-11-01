// SPDX-License-Identifier: Apache-2.0
// AnyDash distribution steps for the main-only unified release: release
// identity, macOS signing order and notarization, Windows signing lists, the
// desktop update archives, release-set assembly with the signed checksum list,
// and immutable GitHub Release and npm publication.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import {
  createHash,
  createPrivateKey,
  createPublicKey,
  sign,
} from "node:crypto";
import {
  appendFileSync,
  cpSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  readSync,
  closeSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { verifySums } from "../../apps/anydash/native/desktop/updater.mjs";
import {
  executableName,
  platformPackage,
  platforms,
} from "../../packages/anydash/bin/verify.mjs";
import {
  archiveName,
  launcherArchiveName,
  packLauncher,
  platformArchiveName,
  sha256File,
  verifyChecksums,
  writeChecksums,
} from "../../packages/anydash/scripts/package.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const json = (path) => JSON.parse(readFileSync(path, "utf8"));
const writeJson = (path, value) =>
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`);
const run = (command, args, options = {}) =>
  execFileSync(command, args, { encoding: "utf8", ...options });
const log = (event, fields = {}) =>
  console.info(JSON.stringify({ event, ...fields }));

/** Desktop update feed platforms; keys match the app updater's platformKey. */
export const DesktopPlatform = Object.freeze({
  DarwinArm64: "darwin-arm64",
  DarwinX64: "darwin-x64",
  WindowsX64: "windows-x64",
});
export const desktopPlatforms = Object.freeze(Object.values(DesktopPlatform));
/** Windows arm64 is cross-built on the Windows x64 runner and not executed. */
export const crossBuiltPlatforms = Object.freeze(["windows-arm64"]);

export const desktopArchiveName = (key, version) =>
  `anydash-desktop-${key}-${version}.tar.gz`;
export const installerName = (key, version) =>
  key.startsWith("darwin-")
    ? `AnyDash-${version}-macos-${key.slice("darwin-".length)}.dmg`
    : `AnyDash-Setup-${version}-${key}.exe`;

export function releaseIdentity(env = process.env) {
  const match = /^anydash@v((0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*))$/.exec(
    env.ANYDASH_RELEASE_TAG ?? "",
  );
  assert.ok(match, "ANYDASH_RELEASE_TAG must be anydash@vMAJOR.MINOR.PATCH");
  assert.match(
    env.ANYDASH_SOURCE_REVISION ?? "",
    /^[a-f0-9]{40}$/,
    "ANYDASH_SOURCE_REVISION must be a full commit SHA",
  );
  return {
    tag: env.ANYDASH_RELEASE_TAG,
    version: match[1],
    revision: env.ANYDASH_SOURCE_REVISION,
  };
}

/**
 * The prepared release commit mirrors the distribution version into the npm
 * launcher, its exact optional dependencies and the Rust CLI.
 */
export function checkSource(
  identity,
  read = (file) => readFileSync(resolve(root, file), "utf8"),
) {
  const pkg = JSON.parse(read("packages/anydash/package.json"));
  assert.equal(pkg.version, identity.version, "npm launcher version mismatch");
  assert.deepEqual(
    pkg.optionalDependencies,
    Object.fromEntries(
      platforms.map((key) => [platformPackage(key).name, identity.version]),
    ),
    "optional platform dependencies must match the release version",
  );
  const cargo = read("cmds/anydash/Cargo.toml");
  assert.match(
    cargo,
    new RegExp(`^version = "${identity.version.replaceAll(".", "\\.")}"$`, "m"),
    "Rust CLI version mismatch",
  );
  return identity;
}

const MachO = new Set([
  0xfeedface, 0xfeedfacf, 0xcefaedfe, 0xcffaedfe, 0xcafebabe, 0xbebafeca,
]);
function isMachO(path) {
  const descriptor = openSync(path, "r");
  try {
    const header = Buffer.alloc(4);
    if (readSync(descriptor, header, 0, 4, 0) !== 4) return false;
    return MachO.has(header.readUInt32BE(0));
  } finally {
    closeSync(descriptor);
  }
}
const bundle = /\.(app|appex|framework|xpc|bundle)$/;

/**
 * Inside-out signing order for a macOS app: every nested Mach-O file and code
 * bundle, deepest first, then the app itself. `keep` lists relative paths that
 * already carry their final signature (the signed collector and the widget
 * host); they and their contents are sealed by the outer signature unchanged.
 */
export function signingOrder(app, keep = []) {
  const kept = keep.map((path) => resolve(app, path));
  const result = [];
  const visit = (directory) => {
    for (const name of readdirSync(directory).sort()) {
      const path = join(directory, name);
      if (
        kept.some(
          (entry) => path === entry || path.startsWith(`${entry}${sep}`),
        )
      )
        continue;
      const stat = lstatSync(path);
      if (stat.isSymbolicLink()) continue;
      if (stat.isDirectory()) {
        visit(path);
        if (bundle.test(name) && !/\.bundle$/.test(name))
          result.push({ path, bundle: true });
        else if (/\.bundle$/.test(name) && containsCode(path))
          result.push({ path, bundle: true });
      } else if (stat.isFile() && isMachO(path))
        result.push({ path, bundle: false });
    }
  };
  const containsCode = (directory) =>
    result.some((entry) => entry.path.startsWith(`${directory}${sep}`));
  visit(app);
  const depth = (path) => relative(app, path).split(sep).length;
  result.sort(
    (a, b) => depth(b.path) - depth(a.path) || a.path.localeCompare(b.path),
  );
  result.push({ path: app, bundle: true });
  return result;
}

/** Signs the app inside-out with the hardened runtime and a secure timestamp. */
export function signMacApp(
  app,
  { identity, entitlements, keychain, keep = [], exec = run },
) {
  assert.ok(identity, "ANYDASH_MACOS_SIGNING_IDENTITY is required");
  for (const entry of signingOrder(app, keep)) {
    const args = [
      "--force",
      "--sign",
      identity,
      "--options",
      "runtime",
      "--timestamp",
    ];
    if (keychain) args.push("--keychain", keychain);
    // App bundles host the JavaScript runtime and need its JIT entitlements;
    // libraries and helper executables are signed without entitlements.
    if (entry.bundle && entry.path.endsWith(".app"))
      args.push("--entitlements", entitlements);
    exec("codesign", [...args, entry.path]);
  }
  exec("codesign", ["--verify", "--deep", "--strict", "--verbose=2", app]);
}

export function signMacFile(file, { identity, keychain, exec = run }) {
  assert.ok(identity, "ANYDASH_MACOS_SIGNING_IDENTITY is required");
  const args = [
    "--force",
    "--sign",
    identity,
    "--options",
    "runtime",
    "--timestamp",
  ];
  if (keychain) args.push("--keychain", keychain);
  exec("codesign", [...args, file]);
  exec("codesign", ["--verify", "--strict", "--verbose=2", file]);
}

/** Submits to Apple notarization and staples bundles and disk images. */
export function notarize(path, env = process.env, exec = run) {
  for (const name of [
    "APPLE_API_KEY_PATH",
    "APPLE_API_KEY",
    "APPLE_API_ISSUER",
  ])
    assert.ok(env[name], `${name} is required for notarization`);
  const temporary = mkdtempSync(join(tmpdir(), "anydash-notarize-"));
  try {
    let submission = path;
    if (!path.endsWith(".dmg")) {
      submission = join(temporary, `${basename(path)}.zip`);
      exec("ditto", ["-c", "-k", "--keepParent", path, submission]);
    }
    const result = JSON.parse(
      exec("xcrun", [
        "notarytool",
        "submit",
        submission,
        "--key",
        env.APPLE_API_KEY_PATH,
        "--key-id",
        env.APPLE_API_KEY,
        "--issuer",
        env.APPLE_API_ISSUER,
        "--wait",
        "--output-format",
        "json",
      ]),
    );
    log("anydash_notarization", {
      target: basename(path),
      status: result.status,
      id: result.id,
    });
    assert.equal(
      result.status,
      "Accepted",
      `Notarization ${result.status} for ${basename(path)}`,
    );
    // A standalone executable cannot hold a ticket; Gatekeeper checks online.
    if (path.endsWith(".app") || path.endsWith(".dmg")) {
      exec("xcrun", ["stapler", "staple", path]);
      exec("xcrun", ["stapler", "validate", path]);
    }
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
}

/** Lists the Windows files the release signs, excluding already-signed paths. */
export function windowsSigningFiles(directory, keep = []) {
  const kept = keep.map((path) => resolve(directory, path));
  const files = [];
  const visit = (path) => {
    for (const name of readdirSync(path).sort()) {
      const child = join(path, name);
      if (kept.includes(child)) continue;
      const stat = lstatSync(child);
      if (stat.isDirectory()) visit(child);
      else if (stat.isFile() && /\.(exe|dll|node)$/i.test(name))
        files.push(child);
    }
  };
  visit(directory);
  return files;
}

function desktopLayout(platform) {
  const mac = platform.startsWith("darwin-");
  const build = resolve(root, "apps/anydash/native/desktop/build");
  const app = resolve(build, mac ? "AnyDash.app" : "application");
  const resources = resolve(
    app,
    mac ? "Contents/Resources/app" : "resources/app",
  );
  return {
    mac,
    build,
    app,
    resources,
    entry: mac ? "AnyDash.app" : "application",
    collector: resolve(resources, "bin", mac ? "anydash" : "anydash.exe"),
    widget: mac
      ? resolve(app, "Contents/Helpers/AnyDash Widgets.app")
      : resolve(resources, "widgets/AnyDash.Widgets.msix"),
  };
}

/**
 * Packs the signed desktop app as the update archive, after checking that it
 * is a release build of this version that embeds the verified collector and
 * the widget host. The installer must already be in `destination`.
 */
export function packageDesktop(
  platform,
  destination,
  identity = releaseIdentity(),
) {
  assert.ok(
    desktopPlatforms.includes(platform),
    `no desktop app for ${platform}`,
  );
  const layout = desktopLayout(platform);
  const config = json(resolve(layout.resources, "package.json"));
  assert.equal(config.name, "anydash");
  assert.equal(config.version, identity.version, "desktop version mismatch");
  assert.equal(config.releaseBuild, true, "desktop must be a release build");
  assert.equal(config.testing, false);
  assert.ok(config.updatePublicKey, "desktop must embed the update key");
  assert.ok(
    layout.mac ? config.teamId : config.publisher,
    "desktop must embed the signer identity",
  );
  const cli = resolve(
    root,
    "packages/anydash/dist/artifacts",
    `anydash-${platform}`,
    executableName(platform),
  );
  const build = json(resolve(root, "packages/anydash/dist/build.json"));
  assert.equal(build.sourceRevision, identity.revision);
  assert.equal(build.version, identity.version);
  assert.equal(
    sha256File(layout.collector),
    build.artifacts[platform]?.sha256,
    "desktop must embed the matching signed CLI",
  );
  assert.equal(sha256File(layout.collector), sha256File(cli));
  assert.ok(existsSync(layout.widget), "desktop must embed the widget host");
  const archive = desktopArchiveName(platform, identity.version);
  const installer = installerName(platform, identity.version);
  assert.ok(
    existsSync(resolve(destination, installer)),
    `missing installer ${installer}`,
  );
  mkdirSync(destination, { recursive: true });
  // One top-level entry, as the updater expects. COPYFILE_DISABLE keeps
  // AppleDouble files out; the relative name keeps GNU tar on Windows local.
  run("tar", ["-czf", archive, "-C", layout.build, layout.entry], {
    cwd: destination,
    env: { ...process.env, COPYFILE_DISABLE: "1" },
  });
  const record = {
    ...identity,
    platform,
    archive,
    sha256: sha256File(resolve(destination, archive)),
    installer,
    installerSha256: sha256File(resolve(destination, installer)),
    collectorSha256: sha256File(layout.collector),
    signing: layout.mac ? "developer-id-notarized" : "authenticode",
  };
  writeJson(resolve(destination, `desktop-${platform}.json`), record);
  log("anydash_desktop_packaged", { platform, archive, installer });
  return record;
}

/** Copies this host's CLI packaging output into a release candidate directory. */
export function collectCandidate(destination, identity = releaseIdentity()) {
  const dist = resolve(root, "packages/anydash/dist");
  const build = json(resolve(dist, "build.json"));
  assert.equal(
    build.sourceRevision,
    identity.revision,
    "CLI source revision mismatch",
  );
  assert.equal(build.version, identity.version, "CLI version mismatch");
  mkdirSync(destination, { recursive: true });
  const sums = verifyChecksums(dist);
  for (const key of Object.keys(build.artifacts)) {
    for (const name of [
      archiveName(key, identity.version),
      platformArchiveName(key, identity.version),
    ]) {
      assert.ok(sums.has(name), `unverified candidate ${name}`);
      cpSync(resolve(dist, name), resolve(destination, name));
    }
    const smoke = resolve(dist, `smoke-${key}.json`);
    if (existsSync(smoke))
      cpSync(smoke, resolve(destination, `smoke-${key}.json`));
  }
  writeJson(
    resolve(
      destination,
      `cli-${Object.keys(build.artifacts).sort().join("+")}.json`,
    ),
    build,
  );
  log("anydash_candidate_collected", {
    platforms: Object.keys(build.artifacts),
  });
}

function extract(archive, directory) {
  mkdirSync(directory, { recursive: true });
  // A relative archive name keeps GNU tar on Windows from reading `C:` as a host.
  run("tar", ["-xzf", basename(archive), "-C", directory], {
    cwd: dirname(archive),
  });
  return directory;
}

/** The files a complete AnyDash release publishes, besides SHA256SUMS(.sig). */
export function releaseFiles(version) {
  return [
    ...platforms.flatMap((key) => [
      archiveName(key, version),
      platformArchiveName(key, version),
    ]),
    launcherArchiveName(version),
    ...desktopPlatforms.flatMap((key) => [
      desktopArchiveName(key, version),
      installerName(key, version),
    ]),
    "anydash-release.json",
  ].sort();
}

/**
 * Combines the host candidates into the release set: all six CLI targets with
 * matching executables and native smoke records, three desktop packages that
 * embed those executables, the launcher with the combined manifest and one
 * SHA256SUMS over every published file.
 */
export function assemble(
  inputs,
  destination,
  identity = releaseIdentity(),
  { pkg } = {},
) {
  assert.ok(!existsSync(destination), "release destination must be new");
  mkdirSync(destination, { recursive: true });
  const temporary = mkdtempSync(join(tmpdir(), "anydash-assemble-"));
  try {
    const artifacts = {};
    const smoke = {};
    const desktops = {};
    for (const host of readdirSync(inputs).sort()) {
      const directory = resolve(inputs, host);
      for (const name of readdirSync(directory).sort()) {
        const path = resolve(directory, name);
        if (/^cli-.*\.json$/.test(name)) {
          const build = json(path);
          assert.equal(
            build.sourceRevision,
            identity.revision,
            "mixed source revisions",
          );
          assert.equal(build.version, identity.version, "mixed CLI versions");
          for (const [key, entry] of Object.entries(build.artifacts)) {
            assert.ok(
              platforms.includes(key) && !artifacts[key],
              `unexpected or duplicate target ${key}`,
            );
            // Both archives must contain exactly the executable the host recorded.
            const direct = extract(
              resolve(directory, archiveName(key, identity.version)),
              resolve(temporary, `direct-${key}`),
            );
            const npmPackage = extract(
              resolve(directory, platformArchiveName(key, identity.version)),
              resolve(temporary, `npm-${key}`),
            );
            const name = executableName(key);
            for (const file of [
              resolve(direct, `anydash-${key}`, name),
              resolve(npmPackage, "package", name),
            ])
              assert.equal(
                sha256File(file),
                entry.sha256,
                `executable mismatch for ${key}`,
              );
            const metadata = json(resolve(npmPackage, "package/package.json"));
            const info = platformPackage(key);
            assert.equal(metadata.name, info.name);
            assert.equal(metadata.version, identity.version);
            assert.deepEqual(metadata.os, [info.os]);
            assert.deepEqual(metadata.cpu, [info.cpu]);
            for (const file of [
              archiveName(key, identity.version),
              platformArchiveName(key, identity.version),
            ])
              cpSync(resolve(directory, file), resolve(destination, file));
            artifacts[key] = entry;
          }
        } else if (/^smoke-.*\.json$/.test(name)) {
          const record = json(path);
          assert.equal(record.status, "passed");
          assert.equal(record.sourceRevision, identity.revision);
          assert.equal(record.version, identity.version);
          smoke[record.platform] = record;
        } else if (/^desktop-.*\.json$/.test(name)) {
          const record = json(path);
          assert.ok(
            desktopPlatforms.includes(record.platform) &&
              !desktops[record.platform],
            "unexpected or duplicate desktop",
          );
          assert.equal(record.tag, identity.tag);
          assert.equal(record.revision, identity.revision);
          assert.equal(
            record.archive,
            desktopArchiveName(record.platform, identity.version),
          );
          assert.equal(
            record.installer,
            installerName(record.platform, identity.version),
          );
          assert.equal(
            sha256File(resolve(directory, record.archive)),
            record.sha256,
          );
          assert.equal(
            sha256File(resolve(directory, record.installer)),
            record.installerSha256,
          );
          for (const file of [record.archive, record.installer])
            cpSync(resolve(directory, file), resolve(destination, file));
          desktops[record.platform] = record;
        }
      }
    }
    assert.deepEqual(
      Object.keys(artifacts).sort(),
      [...platforms].sort(),
      "all six CLI targets are required",
    );
    assert.deepEqual(
      Object.keys(desktops).sort(),
      [...desktopPlatforms].sort(),
      "all three desktop packages are required",
    );
    for (const key of platforms.filter(
      (key) => !crossBuiltPlatforms.includes(key),
    )) {
      assert.ok(smoke[key], `native smoke evidence is required for ${key}`);
      assert.equal(
        smoke[key].archives[archiveName(key, identity.version)],
        sha256File(resolve(destination, archiveName(key, identity.version))),
        `archive changed after smoke for ${key}`,
      );
    }
    for (const record of Object.values(desktops))
      assert.equal(
        record.collectorSha256,
        artifacts[record.platform].sha256,
        `desktop ${record.platform} must embed the released CLI`,
      );
    const manifest = {
      schemaVersion: 1,
      version: identity.version,
      sourceRevision: identity.revision,
      artifacts,
    };
    packLauncher({
      manifest,
      destination,
      stage: resolve(temporary, "launcher"),
      ...(pkg ? { pkg } : {}),
    });
    writeJson(resolve(destination, "anydash-release.json"), {
      schemaVersion: 1,
      ...identity,
      artifacts,
      desktops: desktopPlatforms.map((key) => desktops[key]),
      smoke: Object.keys(smoke).sort(),
      crossBuilt: [...crossBuiltPlatforms],
    });
    writeChecksums(destination, releaseFiles(identity.version));
    log("anydash_release_assembled", {
      tag: identity.tag,
      cliTargets: platforms.length,
      desktops: desktopPlatforms.length,
    });
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
}

/** Ed25519 signature over SHA256SUMS for the desktop update feed. */
export function signChecksums(directory, { privateKey, publicKey }) {
  assert.ok(privateKey, "ANYDASH_UPDATE_PRIVATE_KEY is required");
  assert.ok(publicKey, "ANYDASH_UPDATE_PUBLIC_KEY is required");
  const key = createPrivateKey({
    key: Buffer.from(privateKey, "base64"),
    format: "der",
    type: "pkcs8",
  });
  assert.equal(
    key.asymmetricKeyType,
    "ed25519",
    "the update key must be Ed25519",
  );
  assert.equal(
    createPublicKey(key)
      .export({ format: "der", type: "spki" })
      .toString("base64"),
    publicKey,
    "the update private key does not match ANYDASH_UPDATE_PUBLIC_KEY",
  );
  const sums = readFileSync(resolve(directory, "SHA256SUMS"));
  const signature = sign(null, sums, key).toString("base64");
  writeFileSync(resolve(directory, "SHA256SUMS.sig"), `${signature}\n`);
  // Check with the desktop updater's own verification code.
  assert.ok(
    verifySums(
      sums,
      readFileSync(resolve(directory, "SHA256SUMS.sig")),
      publicKey,
    ),
  );
  log("anydash_checksums_signed", {
    files: sums.toString("utf8").trim().split("\n").length,
  });
}

/** Verifies a complete, signed release set before any publication. */
export function verifyRelease(
  directory,
  publicKey,
  identity = releaseIdentity(),
) {
  const record = json(resolve(directory, "anydash-release.json"));
  assert.equal(record.tag, identity.tag);
  assert.equal(record.revision, identity.revision);
  assert.equal(record.version, identity.version);
  const sums = verifyChecksums(directory);
  assert.deepEqual(
    [...sums.keys()].sort(),
    releaseFiles(identity.version),
    "SHA256SUMS must cover every release file",
  );
  assert.ok(publicKey, "ANYDASH_UPDATE_PUBLIC_KEY is required");
  assert.ok(
    verifySums(
      readFileSync(resolve(directory, "SHA256SUMS")),
      readFileSync(resolve(directory, "SHA256SUMS.sig")),
      publicKey,
    ),
    "SHA256SUMS.sig does not verify",
  );
  const launcher = extract(
    resolve(directory, launcherArchiveName(identity.version)),
    mkdtempSync(join(tmpdir(), "anydash-verify-")),
  );
  try {
    const manifest = json(resolve(launcher, "package/manifest.json"));
    assert.deepEqual(
      manifest.artifacts,
      record.artifacts,
      "launcher manifest mismatch",
    );
    assert.equal(manifest.sourceRevision, identity.revision);
  } finally {
    rmSync(launcher, { recursive: true, force: true });
  }
  return {
    ...record,
    files: [...releaseFiles(identity.version), "SHA256SUMS", "SHA256SUMS.sig"],
  };
}

const repo = "delinoio/apps";
const gh = (...args) => run("gh", args);
const ghJson = (path) =>
  JSON.parse(gh("api", "--paginate", "--slurp", path)).flat();

/** Workflow artifact holding the assembled, signed release set of a revision. */
export const releaseArtifactName = (revision) => `anydash-release-${revision}`;

/**
 * Finds the release set an earlier dispatch of the same publication identity
 * assembled. macOS and Windows signatures carry timestamps and the desktop
 * archives are packaged again, so a rebuild never matches assets that were
 * already uploaded; a repeated dispatch must publish the original set instead.
 * Only unexpired artifacts from unified release runs on main qualify, and the
 * earliest one wins because it is the set that publication may have started.
 * The publish job still verifies the record, checksums and update-key
 * signature before it uses the set. Returns the run ID, or undefined to build.
 */
export function retainedRelease(
  identity = releaseIdentity(),
  { currentRunId = process.env.GITHUB_RUN_ID, api = ghJson } = {},
) {
  const name = releaseArtifactName(identity.revision);
  const candidates = api(
    `repos/${repo}/actions/artifacts?name=${encodeURIComponent(name)}&per_page=100`,
  )
    .flatMap((page) => page.artifacts ?? [])
    .filter(
      (artifact) =>
        artifact.name === name &&
        !artifact.expired &&
        String(artifact.workflow_run?.id) !== String(currentRunId) &&
        artifact.workflow_run?.head_branch === "main" &&
        artifact.workflow_run?.head_repository_id ===
          artifact.workflow_run?.repository_id,
    )
    .sort((a, b) => Date.parse(a.created_at) - Date.parse(b.created_at));
  for (const artifact of candidates) {
    const [workflowRun] = api(
      `repos/${repo}/actions/runs/${artifact.workflow_run.id}`,
    );
    if (
      workflowRun?.path === ".github/workflows/release.yml" &&
      workflowRun.event === "repository_dispatch" &&
      workflowRun.head_branch === "main"
    ) {
      log("anydash_release_retained", {
        tag: identity.tag,
        revision: identity.revision,
        runId: workflowRun.id,
        artifactId: artifact.id,
      });
      return String(workflowRun.id);
    }
  }
  log("anydash_release_build", {
    tag: identity.tag,
    revision: identity.revision,
    candidates: candidates.length,
  });
  return undefined;
}

/** Uploads to a draft GitHub Release; existing assets must match byte for byte. */
export function publishRelease(
  directory,
  publicKey,
  identity = releaseIdentity(),
) {
  const { files } = verifyRelease(directory, publicKey, identity);
  const find = () =>
    JSON.parse(
      gh("api", "--paginate", "--slurp", `repos/${repo}/releases?per_page=100`),
    )
      .flat()
      .find((entry) => entry.tag_name === identity.tag);
  let release = find();
  if (!release) {
    gh(
      "release",
      "create",
      identity.tag,
      "--repo",
      repo,
      "--target",
      identity.revision,
      "--draft",
      "--title",
      `AnyDash ${identity.version}`,
      "--notes",
      `AnyDash ${identity.version}. Install the CLI with https://anydash.delino.io/install.sh, https://anydash.delino.io/install.ps1 or npm install --global @delinoio/anydash. Desktop installers for macOS and Windows are attached. Verify downloads with SHA256SUMS.`,
    );
    release = find();
  }
  assert.ok(release, "draft release was not returned after creation");
  assert.equal(
    release.target_commitish,
    identity.revision,
    "release source mismatch",
  );
  const temporary = mkdtempSync(join(tmpdir(), "anydash-published-"));
  try {
    for (const name of files) {
      if (release.assets.some((asset) => asset.name === name)) {
        gh(
          "release",
          "download",
          identity.tag,
          "--repo",
          repo,
          "--pattern",
          name,
          "--dir",
          temporary,
        );
        assert.equal(
          sha256File(resolve(temporary, name)),
          sha256File(resolve(directory, name)),
          `published asset differs: ${name}; never overwrite an immutable release asset`,
        );
        log("anydash_release_asset_reused", { tag: identity.tag, name });
      } else {
        gh(
          "release",
          "upload",
          identity.tag,
          "--repo",
          repo,
          resolve(directory, name),
        );
        log("anydash_release_asset_uploaded", { tag: identity.tag, name });
      }
    }
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
}

const integrity = (path) =>
  `sha512-${createHash("sha512").update(readFileSync(path)).digest("base64")}`;

/**
 * Publishes the platform packages, then the launcher. An existing version is
 * accepted only when the registry holds the same bytes, so a retry completes a
 * partial publication without replacing anything.
 */
export function publishNpm(
  directory,
  publicKey,
  identity = releaseIdentity(),
  exec = spawnSync,
) {
  verifyRelease(directory, publicKey, identity);
  const npm = (...args) => {
    const result = exec("npm", args, {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    });
    if (result.error) throw result.error;
    return result;
  };
  const whoami = npm("whoami");
  assert.equal(whoami.status, 0, "npm authentication failed");
  const packages = [
    ...platforms.map((key) => ({
      name: platformPackage(key).name,
      archive: platformArchiveName(key, identity.version),
    })),
    {
      name: "@delinoio/anydash",
      archive: launcherArchiveName(identity.version),
    },
  ];
  for (const { name, archive } of packages) {
    const expected = integrity(resolve(directory, archive));
    const existing = npm(
      "view",
      `${name}@${identity.version}`,
      "dist.integrity",
    );
    if (existing.status === 0 && existing.stdout.trim()) {
      assert.equal(
        existing.stdout.trim(),
        expected,
        `${name}@${identity.version} exists with different bytes; reconcile manually`,
      );
      log("anydash_npm_reused", { name, version: identity.version });
      continue;
    }
    const published = npm(
      "publish",
      resolve(directory, archive),
      "--access",
      "public",
      "--provenance",
      "--ignore-scripts",
    );
    assert.equal(published.status, 0, `npm publish failed for ${name}`);
    const confirmed = npm(
      "view",
      `${name}@${identity.version}`,
      "dist.integrity",
    );
    assert.equal(
      confirmed.stdout.trim(),
      expected,
      `published npm bytes are not confirmed for ${name}`,
    );
    log("anydash_npm_published", { name, version: identity.version });
  }
}

function output(name, values) {
  const delimiter = `ANYDASH_${Date.now()}`;
  appendFileSync(
    process.env.GITHUB_OUTPUT,
    `${name}<<${delimiter}\n${values.join("\n")}\n${delimiter}\n`,
  );
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  const [command, ...args] = process.argv.slice(2);
  const env = process.env;
  const signer = {
    identity: env.ANYDASH_MACOS_SIGNING_IDENTITY,
    keychain: env.ANYDASH_KEYCHAIN,
    entitlements: resolve(
      root,
      "apps/anydash/native/desktop/release.entitlements",
    ),
  };
  if (command === "retained") {
    const runId = retainedRelease(releaseIdentity()) ?? "";
    if (env.GITHUB_OUTPUT)
      appendFileSync(env.GITHUB_OUTPUT, `run-id=${runId}\n`);
  } else if (command === "source") {
    const identity = checkSource(releaseIdentity());
    if (env.GITHUB_OUTPUT)
      appendFileSync(env.GITHUB_OUTPUT, `version=${identity.version}\n`);
    log("anydash_release_source", identity);
  } else if (command === "sign-macos-files") {
    for (const file of args) signMacFile(resolve(file), signer);
  } else if (command === "sign-macos-app") {
    signMacApp(resolve(args[0]), { ...signer, keep: args.slice(1) });
  } else if (command === "notarize") {
    for (const path of args) notarize(resolve(path));
  } else if (command === "windows-signing-files") {
    output("files", windowsSigningFiles(resolve(args[0]), args.slice(1)));
  } else if (command === "candidate") collectCandidate(resolve(args[0]));
  else if (command === "desktop") packageDesktop(args[0], resolve(args[1]));
  else if (command === "assemble") assemble(resolve(args[0]), resolve(args[1]));
  else if (command === "sign-checksums")
    signChecksums(resolve(args[0]), {
      privateKey: env.ANYDASH_UPDATE_PRIVATE_KEY,
      publicKey: env.ANYDASH_UPDATE_PUBLIC_KEY,
    });
  else if (command === "verify")
    verifyRelease(resolve(args[0]), env.ANYDASH_UPDATE_PUBLIC_KEY);
  else if (command === "publish")
    publishRelease(resolve(args[0]), env.ANYDASH_UPDATE_PUBLIC_KEY);
  else if (command === "publish-npm") {
    assert.equal(
      env.ANYDASH_APPROVE_RELEASE,
      "true",
      "explicit approved release required",
    );
    publishNpm(resolve(args[0]), env.ANYDASH_UPDATE_PUBLIC_KEY);
  } else
    throw new Error(
      "Expected source, retained, sign-macos-files, sign-macos-app, notarize, windows-signing-files, candidate, desktop, assemble, sign-checksums, verify, publish or publish-npm",
    );
}
