// SPDX-License-Identifier: Apache-2.0
import { existsSync, readFileSync, realpathSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";

export function collectNodeLicenses(appManifest, notices) {
  const inventory = [],
    seen = new Set();
  function visit(file) {
    file = realpathSync(file);
    const directory = path.dirname(file);
    if (seen.has(directory)) return;
    seen.add(directory);
    const pkg = JSON.parse(readFileSync(file, "utf8"));
    let texts;
    try {
      texts = notices(directory, null, false, pkg);
    } catch (error) {
      throw new Error(`${error.message}: ${pkg.name}@${pkg.version}`, {
        cause: error,
      });
    }
    inventory.push({
      name: pkg.name,
      version: pkg.version,
      license: pkg.license,
      authors: pkg.author,
      contributors: pkg.contributors,
      source: `https://www.npmjs.com/package/${pkg.name}/v/${pkg.version}`,
      notices: texts,
    });
    dependencies(file, pkg);
  }
  function dependencies(file, pkg) {
    const require = createRequire(file);
    for (const name of Object.keys(pkg.dependencies ?? {})) {
      // License inspection reads package metadata, not a JavaScript entry.
      // CSS-only packages and packages with private manifests can have no
      // Node-compatible exports. Use Node's dependency search directories.
      const manifest = (require.resolve.paths(name) ?? [])
        .map((directory) => path.join(directory, name, "package.json"))
        .find((candidate) => existsSync(candidate));
      if (!manifest)
        throw new Error(`fixkey-js-license-owner-missing: ${name}`);
      visit(manifest);
    }
  }
  dependencies(appManifest, JSON.parse(readFileSync(appManifest, "utf8")));
  return inventory;
}
