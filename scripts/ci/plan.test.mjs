import assert from "node:assert/strict";
import test from "node:test";
import { select, selectRustServers, selectChecks } from "./plan.mjs";

test("isolated projects select only their validation", () => {
  assert.deepEqual(select(["apps/delitools/src/main.tsx"]), {
    delcore_api: false,
    devbase_api: false,
    git_tracker_api: false,
    git_tracker: false,
    fixkey: false,
    devbase: false,
    delino_site: false,
    delino_docs: false,
    delidev: false,
    delitools: true,
    protocol: false,
  });
  assert.deepEqual(select(["protos/delcore/v1/delcore.proto"]), {
    delcore_api: true,
    devbase_api: true,
    git_tracker_api: true,
    git_tracker: true,
    fixkey: false,
    devbase: false,
    delino_site: false,
    delino_docs: false,
    delidev: false,
    delitools: false,
    protocol: true,
  });
  assert.deepEqual(select(["pnpm-lock.yaml"]), {
    delcore_api: true,
    devbase_api: true,
    git_tracker_api: true,
    git_tracker: true,
    fixkey: true,
    devbase: true,
    delino_site: true,
    delino_docs: true,
    delidev: true,
    delitools: true,
    protocol: true,
  });
});

for (const [project, key] of [
  ["devbase", "devbase"],
  ["delino-site", "delino_site"],
  ["delino-docs", "delino_docs"],
]) {
  test(`${project} changes select only its frontend`, () => {
    for (const path of [
      `apps/${project}/src/main.tsx`,
      `apps/${project}/docs/services/index.md`,
      `docs/project-${project}.md`,
      `docs/apps-${project}-web-contract.md`,
    ]) {
      assert.deepEqual(select([path]), {
        delcore_api: false,
        devbase_api: key === "devbase" && path === "docs/project-devbase.md",
        git_tracker_api: false,
        git_tracker: false,
        fixkey: false,
        devbase: key === "devbase",
        delino_site: key === "delino_site",
        delino_docs: key === "delino_docs",
        delidev: false,
        delitools: false,
        protocol: false,
      });
    }
  });
}

test("consumer examples select protocol validation without native runners", () => {
  assert.deepEqual(select(["examples/delcore-client/main.go"]), {
    delcore_api: false,
    devbase_api: false,
    git_tracker_api: false,
    git_tracker: false,
    fixkey: false,
    devbase: false,
    delino_site: false,
    delino_docs: false,
    delidev: false,
    delitools: false,
    protocol: true,
  });
});

for (const path of [
  "apps/fixkey-app/src/App.tsx",
  "crates/fixkey-core/src/lib.rs",
  "apps/fixkey-app/scripts/release/fixkey.mjs",
  ".github/workflows/release-fixkey.yml",
  "docs/project-fixkey.md",
]) {
  test(`standalone FixKey ownership: ${path}`, () => {
    assert.deepEqual(select([path]), {
      delcore_api: false,
      devbase_api: false,
      git_tracker_api: false,
      git_tracker: false,
      fixkey: true,
      devbase: false,
      delino_site: false,
      delino_docs: false,
      delidev: false,
      delitools: false,
      protocol: false,
    });
  });
}

test("Git Tracker clients and Action avoid unrelated runners", () => {
  for (const path of [
    "apps/git-tracker/src/main.tsx",
    "actions/git-tracker/main.mjs",
    "docs/project-git-tracker.md",
  ]) {
    const selection = select([path]);
    assert.equal(selection.git_tracker, true);
    assert.equal(selection.fixkey, false);
    assert.equal(selection.protocol, false);
  }
});

for (const [service, key] of [
  ["delcore-api", "delcore_api"],
  ["devbase-api", "devbase_api"],
  ["git-tracker-api", "git_tracker_api"],
]) {
  test(`${service} changes select only the owning server`, () => {
    for (const path of [
      `servers/${service}/src/main.rs`,
      `docs/servers-${service}-contract.md`,
    ]) {
      const selection = select([path]);
      assert.equal(selection[key], true);
      for (const other of ["delcore_api", "devbase_api", "git_tracker_api"])
        if (other !== key) assert.equal(selection[other], false, path);
      for (const frontend of [
        "fixkey",
        "devbase",
        "delino_site",
        "delino_docs",
        "delidev",
        "delitools",
      ])
        assert.equal(selection[frontend], false);
    }
  });
}
test("server-wide license and helpers select every server", () => {
  for (const path of [
    "servers/LICENSE",
    "servers/scripts/dev-api.mjs",
    "servers/NOTICE",
  ]) {
    const selection = select([path]);
    assert.equal(selection.delcore_api, true);
    assert.equal(selection.devbase_api, true);
    assert.equal(selection.git_tracker_api, true);
    assert.equal(selection.fixkey, false);
  }
});

test("protocol contract changes select their owning server", () => {
  for (const [path, owner, other] of [
    ["docs/protos-delcore-v1-contract.md", "delcore_api", "git_tracker_api"],
    ["docs/protos-devbase-v1-contract.md", "devbase_api", "delcore_api"],
    [
      "docs/protos-git-tracker-v1-contract.md",
      "git_tracker_api",
      "delcore_api",
    ],
  ]) {
    const selection = select([path]);
    assert.equal(selection[owner], true, path);
    assert.equal(selection[other], false, path);
    for (const frontend of [
      "fixkey",
      "devbase",
      "delino_site",
      "delino_docs",
      "delidev",
      "delitools",
    ]) {
      assert.equal(selection[frontend], false, `${path}: ${frontend}`);
    }
  }
});

for (const path of ["Cargo.lock", "prebuilt-dependencies.lock.json"]) {
  test(`${path} selects native products without unrelated web or Go jobs`, () => {
    assert.deepEqual(select([path]), {
      delcore_api: false,
      devbase_api: false,
      git_tracker_api: false,
      git_tracker: false,
      fixkey: true,
      devbase: false,
      delino_site: false,
      delino_docs: false,
      delidev: true,
      delitools: true,
      protocol: false,
    });
  });
}

test("go.sum selects Go protocol and consumer validation only", () => {
  assert.deepEqual(select(["go.sum"]), {
    delcore_api: false,
    devbase_api: false,
    git_tracker_api: false,
    git_tracker: false,
    fixkey: false,
    devbase: false,
    delino_site: false,
    delino_docs: false,
    delidev: false,
    delitools: false,
    protocol: true,
  });
});

test("dependency lock selection combines with other changes", () => {
  const native = select(["Cargo.lock"]);
  const go = select(["go.sum"]);
  const combine = (left, right) =>
    Object.fromEntries(
      Object.keys(left).map((key) => [key, left[key] || right[key]]),
    );
  for (const [paths, expected] of [
    [["Cargo.lock", "go.sum"], combine(native, go)],
    [
      ["prebuilt-dependencies.lock.json", "apps/delino-docs/src/main.tsx"],
      combine(native, select(["apps/delino-docs/src/main.tsx"])),
    ],
    [
      ["go.sum", "apps/git-tracker/src/main.tsx"],
      combine(go, select(["apps/git-tracker/src/main.tsx"])),
    ],
  ]) {
    assert.deepEqual(select(paths), expected);
  }
  for (const path of ["package.json", "unknown.lock"]) {
    assert.deepEqual(
      select(["Cargo.lock", "go.sum", path]),
      select(["package.json"]),
    );
  }
});

test("shared Rust server changes select all API validators", () => {
  for (const path of [
    "servers/crates/server-common/src/auth.rs",
    "servers/Cargo.toml",
    "servers/Cargo.lock",
    "servers/crates/delino-protos/build.rs",
  ]) {
    const jobs = select([path]);
    assert.equal(jobs.delcore_api, true, path);
    assert.equal(jobs.devbase_api, true, path);
    assert.equal(jobs.git_tracker_api, true, path);
  }
});

test("Rust server matrix selects only affected services", () => {
  for (const [paths, expected] of [
    [["apps/delitools/src/main.tsx"], []],
    [["servers/delcore-api/src/main.rs"], ["delcore-api"]],
    [["servers/devbase-api/src/main.rs"], ["devbase-api"]],
    [["servers/git-tracker-api/src/main.rs"], ["git-tracker-api"]],
    [
      ["servers/crates/server-common/src/lib.rs"],
      ["delcore-api", "devbase-api", "git-tracker-api"],
    ],
    [
      ["protos/devbase/v1/devbase.proto"],
      ["delcore-api", "devbase-api", "git-tracker-api"],
    ],
    [
      [".github/workflows/CI.yml"],
      ["delcore-api", "devbase-api", "git-tracker-api"],
    ],
  ]) {
    assert.deepEqual(
      selectRustServers(select(paths)).map(({ service }) => service),
      expected,
    );
  }
});

const selectedJobs = (paths) =>
  Object.entries(select(paths))
    .filter(([, selected]) => selected)
    .map(([job]) => job);
const frontends = [
  "git_tracker",
  "fixkey",
  "devbase",
  "delino_site",
  "delino_docs",
  "delidev",
  "delitools",
];

for (const [paths, expected] of [
  [["packages/ui/src/components/button.tsx"], frontends],
  [["packages/ui/package.json"], frontends],
  ...[
    "Cargo.toml",
    "Cargo.lock",
    "prebuilt-dependencies.lock.json",
    "scripts/tauri-cli.mjs",
    "scripts/prebuilt-dependencies.mjs",
    "scripts/archive.mjs",
  ].map((path) => [[path], ["fixkey", "delidev", "delitools"]]),
  ...[".cargo/config.toml", "rust-toolchain", "rust-toolchain.toml"].map(
    (path) => [
      [path],
      [
        "delcore_api",
        "devbase_api",
        "git_tracker_api",
        "fixkey",
        "delidev",
        "delitools",
      ],
    ],
  ),
  [["go.mod"], ["protocol"]],
  [
    ["buf.yaml"],
    [
      "delcore_api",
      "devbase_api",
      "git_tracker_api",
      "git_tracker",
      "protocol",
    ],
  ],
  [
    ["buf.gen.yaml"],
    [
      "delcore_api",
      "devbase_api",
      "git_tracker_api",
      "git_tracker",
      "protocol",
    ],
  ],
  [["scripts/localization/catalog.mjs"], ["delino_site", "delino_docs"]],
  [
    [
      "packages/ui/src/styles/globals.css",
      "go.mod",
      "servers/devbase-api/src/main.rs",
    ],
    ["devbase_api", ...frontends, "protocol"],
  ],
]) {
  test(`shared categories select their consumers: ${paths.join(", ")}`, () => {
    assert.deepEqual(selectedJobs(paths), expected);
  });
}

test("instruction files are excluded before project and shared selection", () => {
  for (const path of [
    "AGENTS.md",
    "CLAUDE.md",
    "scripts/ci/AGENTS.md",
    "packages/ui/AGENTS.md",
    "apps/delitools/AGENTS.md",
    "servers/AGENTS.md",
    "servers/delcore-api/AGENTS.md",
    "servers/crates/server-common/AGENTS.md",
    "protos/AGENTS.md",
    "examples/AGENTS.md",
    "docs/AGENTS.md",
    "scripts/localization/AGENTS.md",
    "apps/devbase/CLAUDE.md",
  ]) {
    assert.deepEqual(selectedJobs([path]), [], path);
    assert.deepEqual(
      select([path, "apps/devbase/src/main.tsx"]),
      select(["apps/devbase/src/main.tsx"]),
      path,
    );
  }
  assert.deepEqual(selectedJobs(["apps/devbase/src/AGENTS.md.ts"]), [
    "devbase",
  ]);
});

test("unclassified shared paths retain global validation", () => {
  for (const path of [
    "package.json",
    "pnpm-workspace.yaml",
    "pnpm-lock.yaml",
    "turbo.json",
    "scripts/ci/plan.mjs",
    ".github/workflows/CI.yml",
    "packages/unknown/package.json",
    "unknown.lock",
  ]) {
    assert.ok(Object.values(select([path])).every(Boolean), path);
  }
});

test("shared validation checks select only affected check categories", () => {
  for (const [paths, expected] of [
    [
      ["AGENTS.md", "scripts/ci/AGENTS.md", "packages/ui/CLAUDE.md"],
      [false, false, false],
    ],
    [["apps/devbase/src/main.tsx"], [false, false, false]],
    [["packages/ui/src/components/button.tsx"], [false, true, false]],
    [["scripts/localization/catalog.mjs"], [true, false, false]],
    [["servers/scripts/dev-api.mjs"], [true, false, false]],
    [["apps/fixkey-app/scripts/release/fixkey.mjs"], [true, false, false]],
    [["apps/delitools/scripts/mobile-workflow.test.mjs"], [true, false, false]],
    [[".github/workflows/release-fixkey.yml"], [true, false, true]],
    [["go.sum"], [false, false, true]],
    [
      ["packages/ui/package.json", "scripts/localization/catalog.mjs"],
      [true, true, false],
    ],
    [["package.json"], [true, true, true]],
    [["pnpm-lock.yaml"], [true, true, true]],
  ])
    assert.deepEqual(
      Object.values(selectChecks(paths)),
      expected,
      paths.join(", "),
    );
});
