// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { createHash, generateKeyPairSync } from "node:crypto";
import { execFileSync } from "node:child_process";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, relative } from "node:path";
import test from "node:test";
import {
  executableName,
  platformPackage,
  platforms,
} from "../../packages/anydash/bin/verify.mjs";
import {
  archiveName,
  launcherArchiveName,
  packArtifact,
  sha256File,
  stageArtifact,
} from "../../packages/anydash/scripts/package.mjs";
import {
  assemble,
  checkSource,
  crossBuiltPlatforms,
  desktopArchiveName,
  desktopPlatforms,
  installerName,
  publishNpm,
  releaseFiles,
  releaseIdentity,
  signChecksums,
  signMacApp,
  signingOrder,
  verifyRelease,
  windowsSigningFiles,
} from "./anydash.mjs";

const identity = {
  tag: "anydash@v1.4.0",
  version: "1.4.0",
  revision: "c".repeat(40),
};
const env = {
  ANYDASH_RELEASE_TAG: identity.tag,
  ANYDASH_SOURCE_REVISION: identity.revision,
};
// The launcher package as the prepared release commit carries it.
const pkg = {
  ...JSON.parse(
    readFileSync(
      new URL("../../packages/anydash/package.json", import.meta.url),
      "utf8",
    ),
  ),
  version: identity.version,
  optionalDependencies: Object.fromEntries(
    platforms.map((key) => [platformPackage(key).name, identity.version]),
  ),
};
const temporary = (t, prefix) => {
  const path = mkdtempSync(join(tmpdir(), prefix));
  t.after(() => rmSync(path, { recursive: true, force: true }));
  return path;
};

test("release identity requires an anydash tag and a full revision", () => {
  assert.deepEqual(releaseIdentity(env), identity);
  for (const bad of [
    { ...env, ANYDASH_RELEASE_TAG: "lane@v1.4.0" },
    { ...env, ANYDASH_RELEASE_TAG: "anydash@v1.4" },
    { ...env, ANYDASH_RELEASE_TAG: "anydash@v01.4.0" },
    { ...env, ANYDASH_SOURCE_REVISION: "main" },
  ])
    assert.throws(() => releaseIdentity(bad));
});

test("the release source mirrors the distribution version everywhere", () => {
  const files = {
    "packages/anydash/package.json": JSON.stringify({
      version: "1.4.0",
      optionalDependencies: Object.fromEntries(
        platforms.map((key) => [platformPackage(key).name, "1.4.0"]),
      ),
    }),
    "cmds/anydash/Cargo.toml":
      '[package]\nname = "anydash"\nversion = "1.4.0"\n',
  };
  assert.deepEqual(
    checkSource(identity, (file) => files[file]),
    identity,
  );
  assert.throws(
    () =>
      checkSource(identity, (file) =>
        file.endsWith("Cargo.toml")
          ? files[file].replace("1.4.0", "1.3.0")
          : files[file],
      ),
    /Rust CLI version/,
  );
  const stale = JSON.parse(files["packages/anydash/package.json"]);
  stale.optionalDependencies[platformPackage("linux-x64").name] = "1.3.0";
  assert.throws(
    () =>
      checkSource(identity, (file) =>
        file.endsWith("package.json") ? JSON.stringify(stale) : files[file],
      ),
    /optional platform dependencies/,
  );
});

const macho = Buffer.from([0xcf, 0xfa, 0xed, 0xfe, 0, 0, 0, 0]);
function macApp(root) {
  const app = join(root, "AnyDash.app");
  const write = (path, bytes = macho) => {
    mkdirSync(join(app, path, ".."), { recursive: true });
    writeFileSync(join(app, path), bytes);
  };
  write("Contents/MacOS/lynxtron");
  write("Contents/Info.plist", "plist");
  write(
    "Contents/Frameworks/Lynxtron Framework.framework/Versions/1.0/Lynxtron Framework",
  );
  write(
    "Contents/Frameworks/Lynxtron Framework.framework/Versions/1.0/Libraries/libnode.dylib",
  );
  write(
    "Contents/Frameworks/Lynxtron Framework.framework/Versions/1.0/Resources/icudtl.dat",
    "data",
  );
  symlinkSync(
    "1.0",
    join(
      app,
      "Contents/Frameworks/Lynxtron Framework.framework/Versions/Current",
    ),
  );
  symlinkSync(
    "Versions/Current/Lynxtron Framework",
    join(
      app,
      "Contents/Frameworks/Lynxtron Framework.framework/Lynxtron Framework",
    ),
  );
  write("Contents/Resources/LynxResources.bundle/Contents/Info.plist", "plist");
  write("Contents/Resources/app/bin/anydash");
  write("Contents/Resources/app/main.mjs", "js");
  write("Contents/Helpers/AnyDash Widgets.app/Contents/MacOS/AnyDash Widgets");
  return app;
}

test("macOS signing runs inside-out and keeps already signed code", (t) => {
  const app = macApp(temporary(t, "anydash-sign-"));
  const keep = [
    "Contents/Resources/app/bin/anydash",
    "Contents/Helpers/AnyDash Widgets.app",
  ];
  const order = signingOrder(app, keep).map(({ path, bundle }) => [
    relative(app, path) || ".",
    bundle,
  ]);
  assert.deepEqual(order, [
    [
      "Contents/Frameworks/Lynxtron Framework.framework/Versions/1.0/Libraries/libnode.dylib",
      false,
    ],
    [
      "Contents/Frameworks/Lynxtron Framework.framework/Versions/1.0/Lynxtron Framework",
      false,
    ],
    ["Contents/Frameworks/Lynxtron Framework.framework", true],
    ["Contents/MacOS/lynxtron", false],
    [".", true],
  ]);
  const calls = [];
  signMacApp(app, {
    identity: "Developer ID Application: Delino (ABCDE12345)",
    entitlements: "/entitlements.plist",
    keychain: "/tmp/release.keychain-db",
    keep,
    exec: (command, args) => calls.push([command, ...args]),
  });
  assert.equal(calls.length, order.length + 1);
  for (const call of calls.slice(0, -1)) {
    assert.deepEqual(call.slice(0, 9), [
      "codesign",
      "--force",
      "--sign",
      "Developer ID Application: Delino (ABCDE12345)",
      "--options",
      "runtime",
      "--timestamp",
      "--keychain",
      "/tmp/release.keychain-db",
    ]);
    assert.equal(call.includes("--entitlements"), call.at(-1) === app);
  }
  assert.deepEqual(calls.at(-1), [
    "codesign",
    "--verify",
    "--deep",
    "--strict",
    "--verbose=2",
    app,
  ]);
  assert.throws(
    () => signMacApp(app, { entitlements: "x", exec: () => {} }),
    /SIGNING_IDENTITY/,
  );
});

test("Windows signing lists executables and libraries but not the signed collector", (t) => {
  const root = temporary(t, "anydash-win-");
  for (const path of [
    "lynxtron.exe",
    "lynxtron.dll",
    "resources/app/bin/anydash.exe",
    "resources/app/main.mjs",
    "resources/app/widgets/AnyDash.Widgets.msix",
  ]) {
    mkdirSync(join(root, path, ".."), { recursive: true });
    writeFileSync(join(root, path), "x");
  }
  assert.deepEqual(
    windowsSigningFiles(root, ["resources/app/bin/anydash.exe"]).map((path) =>
      relative(root, path),
    ),
    ["lynxtron.dll", "lynxtron.exe"],
  );
});

/** Synthetic host candidates in the layout the release workflow uploads. */
function candidates(root, { mutate } = {}) {
  const notices = join(root, "notices");
  mkdirSync(notices, { recursive: true });
  writeFileSync(join(notices, "modules.json"), "[]\n");
  const hosts = {
    "linux-x64": ["linux-x64"],
    "linux-arm64": ["linux-arm64"],
    "darwin-arm64": ["darwin-arm64"],
    "darwin-x64": ["darwin-x64"],
    "windows-x64": ["windows-x64", "windows-arm64"],
  };
  const inputs = join(root, "hosts");
  for (const [host, keys] of Object.entries(hosts)) {
    const directory = join(inputs, `anydash-native-${host}`);
    mkdirSync(directory, { recursive: true });
    const build = {
      schemaVersion: 1,
      version: identity.version,
      sourceRevision: identity.revision,
      artifacts: {},
    };
    for (const key of keys) {
      const executable = join(root, "bin", key, executableName(key));
      mkdirSync(join(executable, ".."), { recursive: true });
      writeFileSync(executable, `signed ${key}`);
      const artifact = join(root, "artifacts", key);
      build.artifacts[key] = stageArtifact({
        key,
        version: identity.version,
        revision: identity.revision,
        executable,
        notices,
        directory: artifact,
      });
      packArtifact({
        key,
        version: identity.version,
        directory: artifact,
        destination: directory,
        stage: join(root, "stage", key),
      });
      if (!crossBuiltPlatforms.includes(key))
        writeFileSync(
          join(directory, `smoke-${key}.json`),
          JSON.stringify({
            status: "passed",
            platform: key,
            version: identity.version,
            sourceRevision: identity.revision,
            archives: {
              [archiveName(key, identity.version)]: sha256File(
                join(directory, archiveName(key, identity.version)),
              ),
            },
          }),
        );
      if (desktopPlatforms.includes(key)) {
        const archive = desktopArchiveName(key, identity.version);
        const installer = installerName(key, identity.version);
        writeFileSync(join(directory, archive), `desktop ${key}`);
        writeFileSync(join(directory, installer), `installer ${key}`);
        writeFileSync(
          join(directory, `desktop-${key}.json`),
          JSON.stringify({
            ...identity,
            platform: key,
            archive,
            sha256: sha256File(join(directory, archive)),
            installer,
            installerSha256: sha256File(join(directory, installer)),
            collectorSha256: build.artifacts[key].sha256,
          }),
        );
      }
    }
    writeFileSync(
      join(directory, `cli-${keys.join("+")}.json`),
      JSON.stringify(build),
    );
    mutate?.(host, directory);
  }
  return inputs;
}

test(
  "assembly requires every target and signs one checksum list for the update feed",
  { skip: process.platform === "win32", timeout: 120000 },
  (t) => {
    const root = temporary(t, "anydash-assemble-");
    const inputs = candidates(root);
    const release = join(root, "release");
    assemble(inputs, release, identity, { pkg });
    const sums = readFileSync(join(release, "SHA256SUMS"), "utf8");
    assert.deepEqual(
      sums
        .trim()
        .split("\n")
        .map((line) => line.split("  ")[1]),
      releaseFiles(identity.version),
    );
    assert.equal(releaseFiles(identity.version).length, 6 * 2 + 1 + 3 * 2 + 1);
    const { privateKey, publicKey } = generateKeyPairSync("ed25519");
    const keys = {
      privateKey: privateKey
        .export({ format: "der", type: "pkcs8" })
        .toString("base64"),
      publicKey: publicKey
        .export({ format: "der", type: "spki" })
        .toString("base64"),
    };
    signChecksums(release, keys);
    const record = verifyRelease(release, keys.publicKey, identity);
    assert.deepEqual(
      Object.keys(record.artifacts).sort(),
      [...platforms].sort(),
    );
    assert.ok(record.files.includes("SHA256SUMS.sig"));
    const other = generateKeyPairSync("ed25519")
      .publicKey.export({ format: "der", type: "spki" })
      .toString("base64");
    assert.throws(
      () => signChecksums(release, { ...keys, publicKey: other }),
      /does not match/,
    );
    assert.throws(
      () => verifyRelease(release, other, identity),
      /does not verify/,
    );
    assert.throws(() =>
      verifyRelease(release, keys.publicKey, {
        ...identity,
        tag: "anydash@v1.4.1",
        version: "1.4.1",
      }),
    );
    writeFileSync(
      join(release, desktopArchiveName("darwin-arm64", identity.version)),
      "tampered",
    );
    assert.throws(
      () => verifyRelease(release, keys.publicKey, identity),
      /checksum mismatch/,
    );

    // The launcher carries the combined manifest and no executable.
    const launcher = join(root, "launcher");
    mkdirSync(launcher);
    execFileSync("tar", [
      "-xzf",
      join(release, launcherArchiveName(identity.version)),
      "-C",
      launcher,
    ]);
    const manifest = JSON.parse(
      readFileSync(join(launcher, "package/manifest.json"), "utf8"),
    );
    assert.deepEqual(
      Object.keys(manifest.artifacts).sort(),
      [...platforms].sort(),
    );
    assert.equal(
      JSON.parse(readFileSync(join(launcher, "package/package.json"), "utf8"))
        .scripts,
      undefined,
    );
  },
);

test(
  "assembly rejects missing targets, mixed revisions and a desktop without the released CLI",
  { skip: process.platform === "win32", timeout: 120000 },
  (t) => {
    const cases = [
      [
        (host, directory) =>
          host === "linux-arm64" && rmSync(directory, { recursive: true }),
        /all six CLI targets/,
      ],
      [
        (host, directory) =>
          host === "darwin-x64" &&
          rmSync(join(directory, "desktop-darwin-x64.json")),
        /all three desktop packages/,
      ],
      [
        (host, directory) =>
          host === "linux-x64" &&
          rmSync(join(directory, "smoke-linux-x64.json")),
        /native smoke evidence/,
      ],
      [
        (host, directory) => {
          if (host !== "windows-x64") return;
          const path = join(directory, "desktop-windows-x64.json");
          writeFileSync(
            path,
            JSON.stringify({
              ...JSON.parse(readFileSync(path, "utf8")),
              collectorSha256: "0".repeat(64),
            }),
          );
        },
        /must embed the released CLI/,
      ],
      [
        (host, directory) => {
          if (host !== "darwin-arm64") return;
          const path = join(directory, "cli-darwin-arm64.json");
          writeFileSync(
            path,
            JSON.stringify({
              ...JSON.parse(readFileSync(path, "utf8")),
              sourceRevision: "d".repeat(40),
            }),
          );
        },
        /mixed source revisions/,
      ],
    ];
    for (const [mutate, message] of cases) {
      const root = temporary(t, "anydash-reject-");
      const inputs = candidates(root, { mutate });
      assert.throws(
        () => assemble(inputs, join(root, "release"), identity, { pkg }),
        message,
      );
    }
  },
);

test("npm publication reuses identical versions, rejects different bytes and publishes the rest", (t) => {
  const root = temporary(t, "anydash-npm-");
  const release = join(root, "release");
  // Publication verifies the full set first; use a real assembled set.
  const inputs = candidates(root);
  assemble(inputs, release, identity, { pkg });
  const { privateKey, publicKey } = generateKeyPairSync("ed25519");
  const pub = publicKey
    .export({ format: "der", type: "spki" })
    .toString("base64");
  signChecksums(release, {
    privateKey: privateKey
      .export({ format: "der", type: "pkcs8" })
      .toString("base64"),
    publicKey: pub,
  });
  const integrity = (name) =>
    `sha512-${createHash("sha512")
      .update(readFileSync(join(release, name)))
      .digest("base64")}`;
  const registry = new Map([
    [
      `${platformPackage("linux-x64").name}@1.4.0`,
      integrity(`delinoio-anydash-linux-x64-1.4.0.tgz`),
    ],
  ]);
  const calls = [];
  const exec = (command, args) => {
    calls.push(args);
    if (args[0] === "whoami") return { status: 0, stdout: "delino\n" };
    if (args[0] === "view") {
      const value = registry.get(args[1]);
      return value
        ? { status: 0, stdout: `${value}\n` }
        : { status: 1, stdout: "", stderr: "E404" };
    }
    if (args[0] === "publish") {
      const archive = args[1].split("/").at(-1);
      const name =
        archive === launcherArchiveName("1.4.0")
          ? "@delinoio/anydash"
          : `@delinoio/${archive.replace(/^delinoio-/, "").replace(/-1\.4\.0\.tgz$/, "")}`;
      registry.set(`${name}@1.4.0`, integrity(archive));
      return { status: 0, stdout: "" };
    }
    throw new Error(`unexpected npm ${args.join(" ")}`);
  };
  publishNpm(release, pub, identity, exec);
  const published = calls
    .filter((args) => args[0] === "publish")
    .map((args) => args[1].split("/").at(-1));
  assert.equal(published.length, platforms.length);
  assert.equal(
    published.at(-1),
    launcherArchiveName("1.4.0"),
    "the launcher publishes last",
  );
  assert.ok(
    !published.includes("delinoio-anydash-linux-x64-1.4.0.tgz"),
    "identical versions are reused",
  );
  for (const args of calls.filter((args) => args[0] === "publish"))
    assert.deepEqual(args.slice(2), [
      "--access",
      "public",
      "--provenance",
      "--ignore-scripts",
    ]);
  registry.set(
    `${platformPackage("darwin-arm64").name}@1.4.0`,
    "sha512-different",
  );
  assert.throws(
    () => publishNpm(release, pub, identity, exec),
    /exists with different bytes/,
  );
});

test("published asset names are the feed the desktop updater reads", async () => {
  const { selectRelease, platformKey } =
    await import("../../apps/anydash/native/desktop/updater.mjs");
  const assets = [
    ...releaseFiles(identity.version),
    "SHA256SUMS",
    "SHA256SUMS.sig",
  ].map((name) => ({
    name,
    size: 1,
    browser_download_url: `https://github.com/delinoio/apps/releases/download/${identity.tag}/${name}`,
  }));
  const releases = [
    { tag_name: identity.tag, draft: false, prerelease: false, assets },
  ];
  assert.deepEqual(
    [
      platformKey("darwin", "arm64"),
      platformKey("darwin", "x64"),
      platformKey("win32", "x64"),
    ],
    [...desktopPlatforms],
  );
  for (const key of desktopPlatforms) {
    const selected = selectRelease(releases, "1.3.9", key);
    assert.equal(selected.version, identity.version);
    assert.equal(
      selected.archive.name,
      desktopArchiveName(key, identity.version),
    );
  }
});
