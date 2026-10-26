import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { test } from "node:test";
import {
  acquireNugetPackage,
  ensureWindowsAppSdk,
  findWindowsSdkMetadata,
  validateArchiveEntries,
  windowsAppSdkKeys,
} from "./desktop-windows-sdk.mjs";

function fixture(context) {
  // Resolve host-level temporary-directory aliases (for example macOS /var ->
  // /private/var) so the SDK cache link checks only see links that a test
  // deliberately creates underneath this canonical root.
  const root = realpathSync(mkdtempSync(join(tmpdir(), "lane-sdk-test-")));
  context.after(() => rmSync(root, { recursive: true, force: true }));
  return root;
}

function write(path, content = "fixture") {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, content);
  return path;
}

function harness(context, arch = "x64") {
  const root = fixture(context);
  const output = join(root, "desktop/build");
  const sdk = join(root, "Windows Kits/10");
  write(join(sdk, "UnionMetadata/10.0.26100.0/Windows.winmd"));
  const commands = [];
  const entries = {
    foundation: [
      "metadata/Microsoft.Windows.AppNotifications.winmd",
      "metadata/Microsoft.Windows.AppLifecycle.winmd",
      "include/MddBootstrap.h",
      `runtimes/win-${arch}/native/Microsoft.WindowsAppRuntime.Bootstrap.dll`,
      "license.txt",
    ],
    runtime: [
      "include/WindowsAppSDK-VersionInfo.h",
      "license.txt",
      "NOTICE.txt",
    ],
    cppwinrt: ["bin/cppwinrt.exe", "LICENSE"],
  };
  let failure;
  const options = {
    output,
    arch,
    env: { WindowsSdkDir: sdk },
    fetchImpl: () => assert.fail("Fixture must not use the network"),
    acquirePackage: async (_cache, spec) => {
      const component = spec.id.endsWith("foundation")
        ? "foundation"
        : spec.id.endsWith("runtime")
          ? "runtime"
          : "cppwinrt";
      return join(root, `${component}.nupkg`);
    },
    runCommand: (command, args) => {
      commands.push({ command, args });
      if (command === "tar") {
        const component = args[1].split(/[\\/]/).at(-1).replace(".nupkg", "");
        if (args[0] === "-tf") return entries[component].join("\n");
        if (failure === "extraction")
          throw new Error("fixture extraction failure");
        for (const entry of args.slice(args.indexOf("--") + 1))
          write(join(args[3], entry));
      } else {
        const projections = args[args.indexOf("-output") + 1];
        write(join(projections, "winrt/base.h"));
        if (failure === "generation")
          throw new Error("fixture generator failure");
        if (failure !== "incomplete")
          for (const header of [
            "Microsoft.Windows.AppNotifications.h",
            "Microsoft.Windows.AppLifecycle.h",
            "Windows.Data.Xml.Dom.h",
          ])
            write(join(projections, "winrt", header));
      }
      return "";
    },
  };
  return {
    root,
    options,
    commands,
    entries,
    fail: (value) => {
      failure = value;
    },
  };
}

test("requires all four overrides, including rejecting explicitly empty overrides", async () => {
  for (const env of [
    { [windowsAppSdkKeys[0]]: "" },
    { [windowsAppSdkKeys[0]]: resolve("custom") },
    Object.fromEntries(windowsAppSdkKeys.map((key) => [key, ""])),
  ])
    await assert.rejects(ensureWindowsAppSdk({ env }), /all four/);
});

test("valid overrides bypass output, SDK discovery, download and generation", async (context) => {
  const { options } = harness(context);
  const paths = await ensureWindowsAppSdk(options);
  assert.deepEqual(await ensureWindowsAppSdk({ env: paths }), {
    ...paths,
    notices: [],
  });
  await assert.rejects(
    ensureWindowsAppSdk({
      env: { ...paths, [windowsAppSdkKeys[0]]: "relative" },
    }),
    /absolute/,
  );
  write(join(paths.LANE_WINDOWS_APP_SDK_PROJECTIONS_DIR, "winrt/base.h"), "");
  await assert.rejects(ensureWindowsAppSdk({ env: paths }), /nonempty/);
});

test("selects numeric SDK versions and honors an explicit version without fallback", (context) => {
  const sdk = fixture(context);
  for (const version of ["10.0.9999.0", "10.0.26100.0"])
    write(join(sdk, "UnionMetadata", version, "Windows.winmd"), version);
  assert.equal(
    findWindowsSdkMetadata({ WindowsSdkDir: sdk }).version,
    "10.0.26100.0",
  );
  assert.equal(
    findWindowsSdkMetadata({
      WindowsSdkDir: sdk,
      WindowsSDKVersion: "10.0.9999.0\\",
    }).version,
    "10.0.9999.0",
  );
  assert.throws(
    () =>
      findWindowsSdkMetadata({
        WindowsSdkDir: sdk,
        WindowsSDKVersion: "10.0.123.0",
      }),
    /Install the Windows/,
  );
  assert.throws(
    () =>
      findWindowsSdkMetadata({
        WindowsSdkDir: sdk,
        WindowsSDKVersion: "../other",
      }),
    /must identify/,
  );
});

test("downloads pinned package bytes, verifies SHA512 and repairs a corrupt archive", async (context) => {
  const cache = fixture(context);
  const bytes = Buffer.from("verified package fixture");
  const spec = {
    id: "microsoft.fixture",
    version: "1.2.3",
    sha512: createHash("sha512").update(bytes).digest("base64"),
  };
  let downloads = 0;
  const fetchImpl = async (url) => {
    assert.equal(
      url,
      "https://api.nuget.org/v3-flatcontainer/microsoft.fixture/1.2.3/microsoft.fixture.1.2.3.nupkg",
    );
    downloads++;
    return new Response(bytes);
  };
  const path = await acquireNugetPackage(cache, spec, { fetchImpl });
  assert.deepEqual(readFileSync(path), bytes);
  await acquireNugetPackage(cache, spec, { fetchImpl });
  assert.equal(downloads, 1);
  write(path, "corrupt");
  await acquireNugetPackage(cache, spec, { fetchImpl });
  assert.equal(downloads, 2);
  write(path, "corrupt");
  await assert.rejects(
    acquireNugetPackage(cache, spec, {
      fetchImpl: async () => new Response("wrong bytes"),
    }),
    /SHA512 mismatch/,
  );
  assert.equal(readFileSync(path, "utf8"), "corrupt");
  await assert.rejects(
    acquireNugetPackage(cache, spec, {
      fetchImpl: async () => new Response("missing", { status: 404 }),
    }),
    /HTTP 404/,
  );
  assert.deepEqual(readdirSync(cache), ["microsoft.fixture.1.2.3.nupkg"]);
  await assert.rejects(
    acquireNugetPackage(cache, { ...spec, id: "../escape" }),
    /Invalid pinned/,
  );
});

test("rejects archive traversal and Windows absolute, ADS and backslash names", () => {
  assert.deepEqual(
    validateArchiveEntries("include/MddBootstrap.h\r\nlicense.txt\r\n"),
    ["include/MddBootstrap.h", "license.txt"],
  );
  for (const name of [
    "../escape",
    "include/../../escape",
    "/absolute",
    "C:/escape",
    "\\\\host\\share",
    "include\\..\\escape",
    "include/header:stream",
    "./file",
    "-C",
    "",
    "bad\u0000name",
  ])
    assert.throws(() => validateArchiveEntries(name), /Unsafe|Empty/);
});

test("generates both projections with OS metadata, validates files and reuses a complete cache", async (context) => {
  const fixture = harness(context);
  const paths = await ensureWindowsAppSdk(fixture.options);
  assert.deepEqual(Object.keys(paths), [...windowsAppSdkKeys, "notices"]);
  assert.deepEqual(
    paths.notices.map(({ name }) => name),
    ["foundation-license.txt", "runtime-NOTICE.txt", "cppwinrt-LICENSE.txt"],
  );
  const generated = fixture.commands.find(({ command }) => command !== "tar");
  assert.equal(generated.args.filter((value) => value === "-input").length, 3);
  assert.ok(generated.args.some((value) => value.endsWith("Windows.winmd")));
  assert.ok(generated.args.includes("-reference"));
  assert.ok(generated.args.includes("-base"));
  const count = fixture.commands.length;
  assert.deepEqual(await ensureWindowsAppSdk(fixture.options), paths);
  assert.equal(fixture.commands.length, count);
  const receipt = JSON.parse(
    readFileSync(
      join(dirname(paths.LANE_WINDOWS_APP_SDK_INCLUDE_DIR), "ready.json"),
      "utf8",
    ),
  );
  assert.equal(receipt.identity.metadata.version, "10.0.26100.0");
  assert.equal(receipt.identity.packages.runtime.version, "1.8.260921001");
  assert.ok(receipt.files["cppwinrt/LICENSE"]);
  assert.ok(receipt.files["runtime/NOTICE.txt"]);
  write(
    join(paths.LANE_WINDOWS_APP_SDK_PROJECTIONS_DIR, "winrt/base.h"),
    "modified",
  );
  await ensureWindowsAppSdk(fixture.options);
  assert.ok(fixture.commands.length > count);
  assert.equal(
    readFileSync(
      join(paths.LANE_WINDOWS_APP_SDK_PROJECTIONS_DIR, "winrt/base.h"),
      "utf8",
    ),
    "fixture",
  );
});

for (const failure of ["extraction", "generation", "incomplete"]) {
  test(`${failure} failure publishes no success and allows a clean retry`, async (context) => {
    const fixture = harness(context);
    fixture.fail(failure);
    await assert.rejects(ensureWindowsAppSdk(fixture.options));
    const cache = join(fixture.options.output, "windows-app-sdk");
    assert.deepEqual(readdirSync(cache), []);
    fixture.fail(undefined);
    const paths = await ensureWindowsAppSdk(fixture.options);
    assert.ok(
      existsSync(
        join(dirname(paths.LANE_WINDOWS_APP_SDK_INCLUDE_DIR), "ready.json"),
      ),
    );
  });
}

test("arm64 chooses only its matching bootstrap", async (context) => {
  const fixture = harness(context, "arm64");
  fixture.entries.foundation.push(
    "runtimes/win-x64/native/Microsoft.WindowsAppRuntime.Bootstrap.dll",
  );
  const paths = await ensureWindowsAppSdk(fixture.options);
  assert.match(paths.LANE_WINDOWS_APP_SDK_BOOTSTRAP_DLL, /win-arm64/);
  const extracted = fixture.commands.filter(({ args }) => args[0] === "-xf");
  assert.ok(
    !extracted.some(({ args }) => args.some((arg) => arg.includes("win-x64"))),
  );
});

test("rejects unsafe outputs, architectures, linked caches and concurrent writers", async (context) => {
  const fixture = harness(context);
  await assert.rejects(
    ensureWindowsAppSdk({ ...fixture.options, output: fixture.root }),
    /desktop\/build/,
  );
  await assert.rejects(
    ensureWindowsAppSdk({ ...fixture.options, arch: "ia32" }),
    /Unsupported/,
  );
  const cache = join(fixture.options.output, "windows-app-sdk");
  mkdirSync(join(cache, ".lock"), { recursive: true });
  await assert.rejects(ensureWindowsAppSdk(fixture.options), /already locked/);
  assert.ok(existsSync(join(cache, ".lock")));
  rmSync(join(cache, ".lock"), { recursive: true });
  rmSync(cache, { recursive: true });
  const external = join(fixture.root, "external");
  mkdirSync(external);
  symlinkSync(
    external,
    cache,
    process.platform === "win32" ? "junction" : "dir",
  );
  await assert.rejects(
    ensureWindowsAppSdk(fixture.options),
    /must not contain links/,
  );
  assert.deepEqual(readdirSync(external), []);
});

test("metadata changes produce a separate projection cache", async (context) => {
  const fixture = harness(context);
  const first = await ensureWindowsAppSdk(fixture.options);
  write(
    join(
      fixture.options.env.WindowsSdkDir,
      "UnionMetadata/10.0.26100.0/Windows.winmd",
    ),
    "updated metadata",
  );
  const second = await ensureWindowsAppSdk(fixture.options);
  assert.notEqual(
    first.LANE_WINDOWS_APP_SDK_PROJECTIONS_DIR,
    second.LANE_WINDOWS_APP_SDK_PROJECTIONS_DIR,
  );
});

test("rejects unsafe archive listings before extraction and cleans the staging area", async (context) => {
  const fixture = harness(context);
  fixture.entries.foundation.push("../../outside");
  await assert.rejects(
    ensureWindowsAppSdk(fixture.options),
    /Unsafe SDK archive/,
  );
  assert.ok(!fixture.commands.some(({ args }) => args[0] === "-xf"));
  assert.deepEqual(
    readdirSync(join(fixture.options.output, "windows-app-sdk")),
    [],
  );
});

test("repairs a malformed completion receipt and missing generated files", async (context) => {
  const fixture = harness(context);
  const paths = await ensureWindowsAppSdk(fixture.options);
  const receipt = join(
    dirname(paths.LANE_WINDOWS_APP_SDK_INCLUDE_DIR),
    "ready.json",
  );
  write(receipt, "invalid JSON");
  await ensureWindowsAppSdk(fixture.options);
  assert.ok(
    JSON.parse(readFileSync(receipt, "utf8")).files["cppwinrt/LICENSE"],
  );
  rmSync(
    join(
      paths.LANE_WINDOWS_APP_SDK_PROJECTIONS_DIR,
      "winrt/Microsoft.Windows.AppLifecycle.h",
    ),
  );
  await ensureWindowsAppSdk(fixture.options);
  assert.ok(
    existsSync(
      join(
        paths.LANE_WINDOWS_APP_SDK_PROJECTIONS_DIR,
        "winrt/Microsoft.Windows.AppLifecycle.h",
      ),
    ),
  );
});
