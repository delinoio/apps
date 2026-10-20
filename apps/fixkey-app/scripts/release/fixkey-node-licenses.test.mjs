// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import {
  mkdirSync,
  mkdtempSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { collectNodeLicenses } from "./fixkey-node-licenses.mjs";

function fixture(t) {
  const root = realpathSync(
    mkdtempSync(path.join(tmpdir(), "fixkey-node-licenses-")),
  );
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const manifest = (directory, value) => {
    mkdirSync(directory, { recursive: true });
    const file = path.join(directory, "package.json");
    writeFileSync(file, JSON.stringify(value));
    return file;
  };
  return { root, manifest };
}

test("license collection retains SPDX and author metadata for packages without license files", (t) => {
  const { root, manifest } = fixture(t);
  const app = manifest(root, { dependencies: { dependency: "1.0.0" } });
  const pkg = {
    name: "dependency",
    version: "1.0.0",
    license: "MIT",
    author: "Fixture Author",
    contributors: ["Fixture Contributor"],
  };
  manifest(path.join(root, "node_modules/dependency"), pkg);
  const inventory = collectNodeLicenses(
    app,
    (directory, explicit, owner, metadata) => {
      assert.equal(directory, path.join(root, "node_modules/dependency"));
      assert.equal(explicit, null);
      assert.equal(owner, false);
      assert.deepEqual(metadata, pkg);
      return ["pinned-MIT.txt"];
    },
  );
  assert.equal(inventory[0].authors, pkg.author);
  assert.deepEqual(inventory[0].contributors, pkg.contributors);
  assert.deepEqual(inventory[0].notices, ["pinned-MIT.txt"]);
});

test("CSS-only exports and private manifests do not require executable entries", (t) => {
  const { root, manifest } = fixture(t);
  const app = manifest(root, { dependencies: { "@fixture/css": "1.0.0" } });
  manifest(path.join(root, "node_modules/@fixture/css"), {
    name: "@fixture/css",
    version: "1.0.0",
    exports: { ".": { style: "./style.css" } },
  });
  assert.equal(
    collectNodeLicenses(app, () => ["LICENSE"])[0].name,
    "@fixture/css",
  );
});

test("symlinked packages resolve their own dependency graph and cycles terminate", (t) => {
  const { root, manifest } = fixture(t);
  const app = manifest(root, { dependencies: { first: "1.0.0" } });
  const store = path.join(root, "store/first");
  manifest(store, {
    name: "first",
    version: "1.0.0",
    dependencies: { second: "1.0.0" },
  });
  manifest(path.join(store, "node_modules/second"), {
    name: "second",
    version: "1.0.0",
    dependencies: { first: "1.0.0" },
  });
  mkdirSync(path.join(root, "node_modules"));
  symlinkSync(store, path.join(root, "node_modules/first"), "junction");
  const inventory = collectNodeLicenses(app, () => ["LICENSE"]);
  assert.deepEqual(
    inventory.map((pkg) => pkg.name),
    ["first", "second"],
  );
});

test("missing dependencies and missing license texts fail with package identity", (t) => {
  const { root, manifest } = fixture(t);
  const app = manifest(root, { dependencies: { dependency: "1.0.0" } });
  assert.throws(
    () => collectNodeLicenses(app, () => []),
    /fixkey-js-license-owner-missing: dependency/u,
  );
  manifest(path.join(root, "node_modules/dependency"), {
    name: "dependency",
    version: "1.0.0",
  });
  assert.throws(
    () =>
      collectNodeLicenses(app, () => {
        throw new Error("fixkey-dependency-license-text-missing");
      }),
    /fixkey-dependency-license-text-missing: dependency@1.0.0/u,
  );
});
