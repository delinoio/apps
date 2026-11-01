// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import {
  appxVersion,
  iosExportOptions,
  releaseAppxManifest,
  releaseIdentity,
} from "./release-config.mjs";

test("store builds take the release version and a 1..9999 build number", () => {
  assert.deepEqual(
    releaseIdentity({
      ANYDASH_RELEASE_VERSION: "1.2.3",
      ANYDASH_BUILD_NUMBER: "42",
    }),
    { version: "1.2.3", buildNumber: "42" },
  );
  for (const env of [
    { ANYDASH_RELEASE_VERSION: "1.2", ANYDASH_BUILD_NUMBER: "1" },
    { ANYDASH_RELEASE_VERSION: "1.2.3", ANYDASH_BUILD_NUMBER: "0" },
    { ANYDASH_RELEASE_VERSION: "1.2.3", ANYDASH_BUILD_NUMBER: "10000" },
    { ANYDASH_RELEASE_VERSION: "1.2.3" },
  ])
    assert.throws(() => releaseIdentity(env));
});

test("the widget MSIX identity follows the signing subject and release version", () => {
  const manifest = readFileSync(
    new URL(
      "../native/widgets/windows/Provider/Package.appxmanifest",
      import.meta.url,
    ),
    "utf8",
  );
  const result = releaseAppxManifest(manifest, {
    publisher: "CN=Delino Inc., O=Delino Inc., C=KR",
    version: "1.2.3",
  });
  assert.match(
    result,
    /<Identity Name="Delino\.AnyDash\.Widgets" Publisher="CN=Delino Inc\., O=Delino Inc\., C=KR" Version="1\.2\.3\.0" ProcessorArchitecture="x64" \/>/,
  );
  assert.equal(
    result.replace(/<Identity\b[^>]*\/>/, ""),
    manifest.replace(/<Identity\b[^>]*\/>/, ""),
  );
  assert.equal(appxVersion("0.1.0"), "0.1.0.0");
  assert.throws(() => appxVersion("70000.0.0"), /65535/);
  for (const publisher of ["Delino", 'CN=Delino" Evil="x', undefined])
    assert.throws(() =>
      releaseAppxManifest(manifest, { publisher, version: "1.2.3" }),
    );
});

test("iOS export options target App Store Connect with the team", () => {
  const plist = iosExportOptions("ABCDE12345");
  assert.match(
    plist,
    /<key>method<\/key>\s*<string>app-store-connect<\/string>/,
  );
  assert.match(plist, /<key>teamID<\/key>\s*<string>ABCDE12345<\/string>/);
  assert.throws(() => iosExportOptions("bad"));
});
