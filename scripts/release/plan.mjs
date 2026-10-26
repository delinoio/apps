// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { bumpVersion } from "./project.mjs";

export const Component = Object.freeze({
  Delino: "delino",
  Delitools: "delitools",
  Devbase: "devbase",
  Perfine: "perfine",
  Lane: "lane",
});
export const serverDependencies = Object.freeze({
  "delcore-api": [],
  "devbase-api": ["delcore-api"],
  "git-tracker-api": ["delcore-api"],
  "delitools-api": ["delcore-api"],
  "perfine-api": ["delcore-api"],
  "lane-api": ["delcore-api"],
});
const owners = {
  "delino-site": "delino",
  "delino-docs": "delino",
  "git-tracker": "delino",
  "delcore-api": "delino",
  "git-tracker-api": "delino",
  devbase: "devbase",
  "devbase-api": "devbase",
  delitools: "delitools",
  "delitools-api": "delitools",
  lane: "lane",
  "lane-api": "lane",
  "perfine-api": "perfine",
};
const frontendServers = {
  devbase: ["devbase-api"],
  "git-tracker": ["git-tracker-api"],
  lane: ["lane-api"],
  delitools: ["delitools-api"],
};
const weight = { none: 0, patch: 1, minor: 2, major: 3 };
const greatest = (a = "none", b = "none") => (weight[a] >= weight[b] ? a : b);
export const compareVersion = (a, b) => {
  const x = a.split(".").map(BigInt),
    y = b.split(".").map(BigInt);
  for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return x[i] > y[i] ? 1 : -1;
  return 0;
};
export function makePlan(records, inventory, read, priorVersions = {}) {
  const bumps = {},
    selected = new Set(),
    changes = {};
  for (const record of records)
    for (const [name, bump] of Object.entries(record.packages)) {
      assert.ok(inventory[name], `Unknown package: ${name}`);
      assert.ok(Object.hasOwn(weight, bump), `Invalid bump: ${bump}`);
      bumps[name] = greatest(bumps[name], bump);
      selected.add(name);
    }
  // The protocol workspace also generates Rust bindings outside npm's graph.
  if (selected.has("@repo/protos"))
    for (const server of Object.keys(serverDependencies))
      selected.add(`@servers/${server}`);
  // Shared workspace changes select their consumers without inventing version bumps.
  let expanded = true;
  while (expanded) {
    expanded = false;
    for (const [name, file] of Object.entries(inventory)) {
      const manifest = JSON.parse(read(file));
      if (
        !selected.has(name) &&
        Object.keys({
          ...manifest.dependencies,
          ...manifest.devDependencies,
          ...manifest.optionalDependencies,
        }).some((dependency) => selected.has(dependency))
      ) {
        selected.add(name);
        expanded = true;
      }
    }
  }
  const components = {},
    servers = new Set();
  function selectComponent(component, bump = "none") {
    const entry = (components[component] ??= {
      component,
      bump: "none",
      apps: [],
      binaries: false,
      mobile: false,
    });
    entry.bump = greatest(entry.bump, bump);
    return entry;
  }
  function selectServer(server) {
    if (servers.has(server)) return;
    servers.add(server);
    for (const dependency of serverDependencies[server])
      selectServer(dependency);
  }
  for (const name of selected) {
    // The Chrome companion is versioned here but submitted to the store separately.
    // Keep it out of the web matrix and component success tags until store
    // publication has its own release gate.
    if (name === "@apps/lane-chrome-extension") continue;
    const file = inventory[name],
      parts = file.split("/"),
      leaf = parts[1];
    let owner = owners[leaf];
    if (name === "perfine" || name === "@actions/perfine") owner = "perfine";
    if (name === "@delinoio/lane") owner = "lane";
    const app = parts[0] === "apps" && owner && !leaf.endsWith("-api");
    const server =
      parts[0] === "servers" && Object.hasOwn(serverDependencies, leaf);
    if (
      !owner ||
      (!app &&
        !server &&
        !["perfine", "@actions/perfine", "@delinoio/lane"].includes(name))
    ) {
      if (
        bumps[name] &&
        bumps[name] !== "none" &&
        !["packages", "protos", "examples"].includes(parts[0])
      )
        throw new Error(
          `No release target for ${name}; use none for internal-only changes`,
        );
      continue;
    }
    const entry = selectComponent(owner, bumps[name]);
    if (app) {
      entry.apps.push(leaf);
      for (const dependency of frontendServers[leaf] ?? [])
        selectServer(dependency);
      if (leaf === "lane") entry.binaries = true;
      if (leaf === "delitools") entry.mobile = true;
    }
    if (server) selectServer(leaf);
    if (["perfine", "@actions/perfine", "@delinoio/lane"].includes(name))
      entry.binaries = true;
  }
  for (const [name, bump] of Object.entries(bumps)) {
    if (bump === "none") continue;
    const file = inventory[name],
      manifest = JSON.parse(read(file));
    assert.ok(manifest.version, `Package has no version: ${name}`);
    changes[file] = read(file).replace(
      /^  "version": "[^"]+"/m,
      `  "version": "${bumpVersion(manifest.version, bump)}"`,
    );
  }
  const versions = { ...priorVersions };
  for (const entry of Object.values(components)) {
    const candidates = Object.entries(inventory)
      .filter(
        ([name, file]) =>
          owners[file.split("/")[1]] === entry.component ||
          (entry.component === "perfine" && name === "perfine"),
      )
      .map(([, file]) => JSON.parse(read(file)).version)
      .filter(Boolean);
    const previous = [priorVersions[entry.component] ?? "0.0.0", ...candidates]
      .sort(compareVersion)
      .at(-1);
    entry.version = bumpVersion(
      previous,
      entry.bump === "none" ? "patch" : entry.bump,
    );
    entry.tag = `${entry.component}@v${entry.version}`;
    entry.apps.sort();
    versions[entry.component] = entry.version;
    delete entry.bump;
  }
  return {
    changes,
    versions,
    components: Object.values(components).sort((a, b) =>
      a.component.localeCompare(b.component),
    ),
    servers: [...servers].sort(),
    records: records.map(({ id }) => id),
    publicNotes: records
      .filter((r) => r.public)
      .map(({ packages, public: note }) => ({
        packages: Object.keys(packages),
        note,
      })),
  };
}
