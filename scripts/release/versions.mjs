// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";

// Native and distribution mirrors follow their owning package, never another app.
export function synchronizeVersions(changes, read, components) {
  const current = (file) => changes[file] ?? read(file);
  const jsonVersion = (file, version, optional = false) => {
    const manifest = JSON.parse(current(file));
    let content = current(file).replace(
      /^  "version": "[^"]+"/m,
      `  "version": "${version}"`,
    );
    if (optional)
      for (const [name, previous] of Object.entries(
        manifest.optionalDependencies ?? {},
      ))
        content = content.replace(
          `"${name}": "${previous}"`,
          `"${name}": "${version}"`,
        );
    changes[file] = content;
  };
  const rustVersion = (file, name, version) => {
    const source = current(file);
    const section = source.match(
      /^\[package\]\n[\s\S]*?(?=^\[|$(?![\s\S]))/m,
    )?.[0];
    assert.ok(
      section?.includes(`name = "${name}"`),
      `Package mismatch: ${file}`,
    );
    changes[file] = source.replace(
      section,
      section.replace(/^version = "[^"]+"/m, `version = "${version}"`),
    );
    const lock = current("Cargo.lock");
    const entries = [
      ...lock.matchAll(/^\[\[package\]\]\n[\s\S]*?(?=^\[\[|$(?![\s\S]))/gm),
    ].filter(
      ([entry]) =>
        entry.includes(`\nname = "${name}"\n`) && !/^source = /m.test(entry),
    );
    assert.equal(entries.length, 1, `Ambiguous Cargo lock package: ${name}`);
    changes["Cargo.lock"] = lock.replace(
      entries[0][0],
      entries[0][0].replace(/^version = "[^"]+"/m, `version = "${version}"`),
    );
  };
  if (changes["apps/delitools/package.json"]) {
    const version = JSON.parse(changes["apps/delitools/package.json"]).version;
    jsonVersion("apps/delitools/src-tauri/tauri.conf.json", version);
    rustVersion(
      "apps/delitools/src-tauri/Cargo.toml",
      "delitools-shell",
      version,
    );
  }
  if (changes["packages/lane/package.json"]) {
    const version = JSON.parse(changes["packages/lane/package.json"]).version;
    jsonVersion("packages/lane/package.json", version, true);
    rustVersion("cmds/lane/Cargo.toml", "lane", version);
  }
  const perfine = components.find(
    (entry) => entry.component === "perfine" && entry.binaries,
  );
  if (perfine) {
    jsonVersion("packages/perfine/package.json", perfine.version, true);
    jsonVersion("actions/perfine/package.json", perfine.version);
    rustVersion("cmds/perfine/Cargo.toml", "perfine", perfine.version);
    rustVersion(
      "crates/perfine-core/Cargo.toml",
      "perfine-core",
      perfine.version,
    );
  }
  for (const file of Object.keys(changes))
    if (changes[file] === read(file)) delete changes[file];
  return changes;
}
