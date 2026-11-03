import assert from "node:assert/strict";
import test from "node:test";
import { select, selectRustServers, selectChecks } from "./plan.mjs";

test("isolated projects select only their validation", () => {
  assert.deepEqual(select(["apps/delitools/src/main.tsx"]), {
    delcore_api: false,
    delitools_api: false,
    devbase_api: false,
    git_tracker_api: false,
    git_tracker: false,
    devbase: false,
    delino_site: false,
    delino_docs: false,
    delidev: false,
    delitools: true,
    perfine_api: false,
    lane_api: false,
    anydash_api: false,
    lane_cli: false,
    anydash_cli: false,
    lane: false,
    mcpa_api: false,
    mcpa: false,
    anydash: false,
    perfine_fixtures: false,
    protocol: false,
  });
  assert.deepEqual(select(["protos/delcore/v1/delcore.proto"]), {
    delcore_api: true,
    delitools_api: true,
    devbase_api: true,
    git_tracker_api: true,
    git_tracker: true,
    devbase: false,
    delino_site: false,
    delino_docs: false,
    delidev: false,
    delitools: false,
    perfine_api: true,
    lane_api: true,
    anydash_api: true,
    lane_cli: false,
    anydash_cli: false,
    lane: false,
    mcpa_api: true,
    mcpa: false,
    anydash: false,
    perfine_fixtures: false,
    protocol: true,
  });
  assert.deepEqual(select(["pnpm-lock.yaml"]), {
    delcore_api: true,
    delitools_api: true,
    devbase_api: true,
    git_tracker_api: true,
    git_tracker: true,
    devbase: true,
    delino_site: true,
    delino_docs: true,
    delidev: true,
    delitools: true,
    perfine_api: true,
    lane_api: true,
    anydash_api: true,
    lane_cli: true,
    anydash_cli: true,
    lane: true,
    mcpa_api: true,
    mcpa: true,
    anydash: true,
    perfine_fixtures: true,
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
    ]) {
      assert.deepEqual(select([path]), {
        delcore_api: false,
        delitools_api: false,
        devbase_api: false,
        git_tracker_api: false,
        git_tracker: false,
        devbase: key === "devbase",
        delino_site: key === "delino_site",
        delino_docs: key === "delino_docs",
        delidev: false,
        delitools: false,
        perfine_api: false,
        lane_api: false,
        anydash_api: false,
        lane_cli: false,
        anydash_cli: false,
        lane: false,
        mcpa_api: false,
        mcpa: false,
        anydash: false,
        perfine_fixtures: false,
        protocol: false,
      });
    }
  });
}

test("consumer examples select protocol validation without native runners", () => {
  assert.deepEqual(select(["examples/delcore-client/main.go"]), {
    delcore_api: false,
    delitools_api: false,
    devbase_api: false,
    git_tracker_api: false,
    git_tracker: false,
    devbase: false,
    delino_site: false,
    delino_docs: false,
    delidev: false,
    delitools: false,
    perfine_api: false,
    lane_api: false,
    anydash_api: false,
    lane_cli: false,
    anydash_cli: false,
    lane: false,
    mcpa_api: false,
    mcpa: false,
    anydash: false,
    perfine_fixtures: false,
    protocol: true,
  });
});

test("Git Tracker clients and Action avoid unrelated runners", () => {
  for (const path of [
    "apps/git-tracker/src/main.tsx",
    "actions/git-tracker/main.mjs",
  ]) {
    const selection = select([path]);
    assert.equal(selection.git_tracker, true);
    assert.equal(selection.protocol, false);
  }
});

for (const [service, key] of [
  ["delcore-api", "delcore_api"],
  ["delitools-api", "delitools_api"],
  ["devbase-api", "devbase_api"],
  ["git-tracker-api", "git_tracker_api"],
]) {
  test(`${service} changes select only the owning server`, () => {
    for (const path of [`servers/${service}/src/main.rs`]) {
      const selection = select([path]);
      assert.equal(selection[key], true);
      for (const other of [
        "delcore_api",
        "delitools_api",
        "devbase_api",
        "git_tracker_api",
      ])
        if (other !== key) assert.equal(selection[other], false, path);
      for (const frontend of [
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
    assert.equal(selection.delitools_api, true);
    assert.equal(selection.devbase_api, true);
    assert.equal(selection.git_tracker_api, true);
  }
});

for (const path of ["Cargo.lock", "prebuilt-dependencies.lock.json"]) {
  test(`${path} selects native products without unrelated web or Go jobs`, () => {
    assert.deepEqual(select([path]), {
      delcore_api: false,
      delitools_api: false,
      devbase_api: false,
      git_tracker_api: false,
      git_tracker: false,
      devbase: false,
      delino_site: false,
      delino_docs: false,
      delidev: true,
      delitools: true,
      perfine_api: false,
      lane_api: false,
      anydash_api: false,
      lane_cli: path === "Cargo.lock",
      anydash_cli: path === "Cargo.lock",
      lane: false,
      mcpa_api: false,
      mcpa: false,
      anydash: false,
      perfine_fixtures: path === "Cargo.lock",
      protocol: false,
    });
  });
}

test("go.sum selects Go protocol and consumer validation only", () => {
  assert.deepEqual(select(["go.sum"]), {
    delcore_api: false,
    delitools_api: false,
    devbase_api: false,
    git_tracker_api: false,
    git_tracker: false,
    devbase: false,
    delino_site: false,
    delino_docs: false,
    delidev: false,
    delitools: false,
    perfine_api: false,
    lane_api: false,
    anydash_api: false,
    lane_cli: false,
    anydash_cli: false,
    lane: false,
    mcpa_api: true,
    mcpa: false,
    anydash: false,
    perfine_fixtures: false,
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
      combine(
        select(["prebuilt-dependencies.lock.json"]),
        select(["apps/delino-docs/src/main.tsx"]),
      ),
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
    assert.equal(jobs.delitools_api, true, path);
    assert.equal(jobs.devbase_api, true, path);
    assert.equal(jobs.git_tracker_api, true, path);
  }
});

test("Rust server matrix selects only affected services", () => {
  for (const [paths, expected] of [
    [["apps/delitools/src/main.tsx"], []],
    [["servers/delcore-api/src/main.rs"], ["delcore-api"]],
    [["servers/delitools-api/src/main.rs"], ["delitools-api"]],
    [["servers/devbase-api/src/main.rs"], ["devbase-api"]],
    [["servers/git-tracker-api/src/main.rs"], ["git-tracker-api"]],
    [
      ["servers/crates/server-common/src/lib.rs"],
      [
        "anydash-api",
        "delcore-api",
        "delitools-api",
        "devbase-api",
        "git-tracker-api",
        "lane-api",
        "perfine-api",
      ],
    ],
    [
      ["protos/devbase/v1/devbase.proto"],
      [
        "anydash-api",
        "delcore-api",
        "delitools-api",
        "devbase-api",
        "git-tracker-api",
        "lane-api",
        "perfine-api",
      ],
    ],
    [
      [".github/workflows/CI.yml"],
      [
        "anydash-api",
        "delcore-api",
        "delitools-api",
        "devbase-api",
        "git-tracker-api",
        "lane-api",
        "perfine-api",
      ],
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
  "devbase",
  "delino_site",
  "delino_docs",
  "delidev",
  "delitools",
];

for (const [paths, expected] of [
  [["packages/ui/src/components/button.tsx"], [...frontends, "mcpa"]],
  [["packages/ui/package.json"], [...frontends, "mcpa"]],
  ...[
    "Cargo.toml",
    "Cargo.lock",
    "prebuilt-dependencies.lock.json",
    "scripts/tauri-cli.mjs",
    "scripts/prebuilt-dependencies.mjs",
    "scripts/archive.mjs",
  ].map((path) => [
    [path],
    [
      "delidev",
      "delitools",
      ...(/^Cargo\./u.test(path)
        ? ["lane_cli", "anydash_cli", "perfine_fixtures"]
        : []),
    ],
  ]),
  ...[".cargo/config.toml", "rust-toolchain", "rust-toolchain.toml"].map(
    (path) => [
      [path],
      [
        "delcore_api",
        "delitools_api",
        "devbase_api",
        "git_tracker_api",
        "delidev",
        "delitools",
        "perfine_api",
        "lane_api",
        "anydash_api",
        "lane_cli",
        "anydash_cli",
        "perfine_fixtures",
      ],
    ],
  ),
  [["go.mod"], ["mcpa_api", "protocol"]],
  [
    ["buf.yaml"],
    [
      "delcore_api",
      "delitools_api",
      "devbase_api",
      "git_tracker_api",
      "git_tracker",
      "perfine_api",
      "lane_api",
      "anydash_api",
      "lane_cli",
      "anydash_cli",
      "lane",
      "mcpa_api",
      "mcpa",
      "anydash",
      "protocol",
    ],
  ],
  [
    ["buf.gen.yaml"],
    [
      "delcore_api",
      "delitools_api",
      "devbase_api",
      "git_tracker_api",
      "git_tracker",
      "perfine_api",
      "lane_api",
      "anydash_api",
      "lane_cli",
      "anydash_cli",
      "lane",
      "mcpa_api",
      "mcpa",
      "anydash",
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
    ["devbase_api", ...frontends, "mcpa_api", "mcpa", "protocol"],
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

test("changeset Markdown alone selects no affected validators", () => {
  for (const path of [
    ".changeset/lane-navigation.md",
    ".changeset/README.md",
  ]) {
    assert.deepEqual(selectedJobs([path]), []);
    assert.ok(Object.values(selectChecks([path])).every((value) => !value));
  }
});

test("Lane navigation changes with a release record select only the frontend", () => {
  const paths = [
    ".changeset/lane-reuse-navigation-tab.md",
    "apps/lane/src/App.tsx",
    "apps/lane/tests/tabs-browser.mjs",
    "apps/lane/tests/tabs.test.ts",
    "apps/lane/tests/workspace-navigation.test.ts",
    "docs/apps-lane-foundation.md",
  ];
  assert.deepEqual(selectedJobs(paths), ["lane"]);
  assert.deepEqual(selectRustServers(select(paths)), []);
  assert.deepEqual(selectChecks(paths), {
    tooling: false,
    shared_ui: false,
    workflows: false,
  });
});

test("release records preserve selection from other changed inputs", () => {
  for (const path of [
    "crates/perfine-core/src/lib.rs",
    "servers/perfine-api/src/lib.rs",
    "servers/crates/server-common/src/lib.rs",
    "scripts/ci/plan.mjs",
    "pnpm-lock.yaml",
  ]) {
    const paths = [".changeset/component-update.md", path];
    assert.deepEqual(select(paths), select([path]), path);
    assert.deepEqual(selectChecks(paths), selectChecks([path]), path);
  }
});

test("Apache namespace metadata selects no affected validators", () => {
  const metadata = ["oss/LICENSE", "oss/NOTICE", "oss/servers/.gitkeep"];
  for (const paths of [...metadata.map((path) => [path]), metadata]) {
    assert.deepEqual(selectedJobs(paths), []);
    assert.ok(Object.values(selectChecks(paths)).every((value) => !value));
    const runtime = ["apps/devbase/src/main.tsx"];
    assert.deepEqual(select([...paths, ...runtime]), select(runtime));
    assert.deepEqual(
      selectChecks([...paths, ...runtime]),
      selectChecks(runtime),
    );
  }
});

test("unregistered Apache service inputs retain global validation", () => {
  for (const path of [
    "oss/servers/example/src/main.rs",
    "oss/servers/example/Cargo.toml",
    "oss/servers/example/LICENSE",
    "oss/servers/.gitkeep.rs",
    "oss/LICENSE.json",
    "oss/Cargo.toml",
  ]) {
    assert.ok(Object.values(select([path])).every(Boolean), path);
    assert.ok(Object.values(selectChecks([path])).every(Boolean), path);
  }
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
    ".changeset/config.json",
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
    [["packages/lynx-ui/src/components/button.tsx"], [false, true, false]],
    [["scripts/localization/catalog.mjs"], [true, false, false]],
    [["servers/scripts/dev-api.mjs"], [true, false, false]],
    [["apps/delitools/scripts/mobile-workflow.test.mjs"], [true, false, false]],
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

test("DeliTools server and consumer example select the server without the frontend", () => {
  for (const path of [
    "servers/delitools-api/src/lib.rs",
    "examples/delitools-client/main.go",
  ]) {
    const selection = select([path]);
    assert.equal(selection.delitools_api, true, path);
    assert.equal(selection.delitools, false, path);
    assert.equal(selection.devbase_api, false, path);
    assert.equal(selection.delcore_api, false, path);
  }
});

test("DeliTools server validation uses its own disposable database", () => {
  assert.deepEqual(
    selectRustServers(select(["servers/delitools-api/src/main.rs"])),
    [
      {
        service: "delitools-api",
        port: 55435,
        user: "delitools",
        database: "delitools_api_test",
      },
    ],
  );
});

test("Perfine ownership selects explicit consumers without unknown-path fallback", () => {
  for (const [path, expected] of [
    ["crates/perfine-core/src/lib.rs", ["perfine_fixtures"]],
    ["cmds/perfine/src/main.rs", ["perfine_fixtures"]],
    ["packages/perfine/package.json", []],
    ["actions/perfine/src/main.ts", []],
    ["servers/perfine-api/src/main.rs", ["perfine_api"]],
  ])
    assert.deepEqual(selectedJobs([path]), expected, path);
  for (const path of ["protos/perfine/v1/perfine.proto", "buf.gen.yaml"])
    for (const key of ["perfine_api", "protocol"])
      assert.equal(select([path])[key], true, `${path}: ${key}`);
  assert.deepEqual(
    selectRustServers(select(["crates/perfine-core/src/lib.rs"])),
    [],
  );
  assert.deepEqual(selectChecks(["crates/perfine-core/src/lib.rs"]), {
    tooling: false,
    shared_ui: false,
    workflows: false,
  });
});

test("Perfine API and server evaluator changes select server validation", () => {
  for (const path of [
    "servers/perfine-api/src/lib.rs",
    "servers/perfine-api/src/evaluation/policy.rs",
    "protos/perfine/v1/perfine.proto",
  ]) {
    assert.ok(
      selectRustServers(select([path])).some(
        (entry) => entry.service === "perfine-api",
      ),
      path,
    );
  }
  assert.deepEqual(
    selectRustServers(select(["servers/perfine-api/src/lib.rs"])),
    [
      {
        service: "perfine-api",
        port: 55436,
        user: "perfine",
        database: "perfine_api_test",
      },
    ],
  );
});

for (const path of [
  "apps/lane/src/App.tsx",
  "apps/lane-chrome-extension/src/background.js",
  "apps/lane-chrome-extension/manifest.json",
  "apps/lane-chrome-extension/tests/capture.test.mjs",
  "apps/lane/native/desktop/main.mjs",
]) {
  test(`Lane paths select only Lane: ${path}`, () => {
    assert.deepEqual(selectedJobs([path]), ["lane"]);
  });
}

test("Lane domain paths select owning validators and shared store consumers", () => {
  for (const [path, expected] of [
    ["servers/lane-api/src/lib.rs", ["lane_api"]],
    ["cmds/lane/src/main.rs", ["lane_cli"]],
    ["packages/lane/package.json", ["lane_cli"]],
    ["apps/lane/native/store/schema.sql", ["lane_cli", "lane"]],
    ["packages/lynx-ui/src/components/input.tsx", ["lane", "anydash"]],
    ["cmds/anydash/src/main.rs", ["anydash_cli"]],
    ["packages/anydash/bin/verify.mjs", ["anydash_cli"]],
    ["packages/anydash/install/install.sh", ["anydash_cli", "anydash"]],
  ])
    assert.deepEqual(selectedJobs([path]), expected, path);
  assert.deepEqual(selectRustServers(select(["servers/lane-api/src/lib.rs"])), [
    {
      service: "lane-api",
      port: 55437,
      user: "lane",
      database: "lane_api_test",
    },
  ]);
});

test("AnyDash server paths select only the AnyDash server validator", () => {
  for (const path of [
    "servers/anydash-api/src/lib.rs",
    "servers/anydash-api/migrations/00001_foundation.sql",
    "servers/anydash-api/ops/alerts.yml",
    "servers/anydash-api/ops/check.sh",
    "servers/anydash-api/loadtest/anydash-load.mjs",
  ])
    assert.deepEqual(selectedJobs([path]), ["anydash_api"], path);
  assert.deepEqual(
    selectRustServers(select(["servers/anydash-api/src/lib.rs"])),
    [
      {
        service: "anydash-api",
        port: 55439,
        user: "anydash",
        database: "anydash_api_test",
      },
    ],
  );
  for (const path of ["protos/anydash/v1/anydash.proto", "buf.gen.yaml"])
    assert.equal(select([path]).anydash_api, true, path);
  for (const path of [
    "docs/servers-anydash-api-contract.md",
    "servers/anydash-api/ops/README.md",
  ]) {
    assert.deepEqual(selectedJobs([path]), [], path);
    assert.deepEqual(selectRustServers(select([path])), [], path);
  }
});

test("AnyDash CLI and distribution paths select only the AnyDash CLI job", () => {
  for (const path of [
    "cmds/anydash/src/main.rs",
    "cmds/anydash/Cargo.toml",
    "packages/anydash/package.json",
    "packages/anydash/scripts/build.mjs",
  ]) {
    assert.deepEqual(selectedJobs([path]), ["anydash_cli"], path);
    assert.ok(
      Object.values(selectChecks([path])).every((value) => !value),
      path,
    );
  }
});

const recoveryPolicyPaths = [
  "AGENTS.md",
  "docs/apps-lane-foundation.md",
  "docs/repository-workflow-contract.md",
  "docs/servers-delcore-api-contract.md",
  "docs/servers-lane-api-contract.md",
  "docs/servers-perfine-api-contract.md",
  "servers/lane-api/ops/README.md",
];

test("internal documentation changes allocate no affected jobs", () => {
  const docs = [
    ...recoveryPolicyPaths,
    "README.md",
    "docs/project-perfine.md",
    "docs/protos-lane-v1-contract.md",
    "docs/packages-lynx-ui-contract.md",
    "servers/delcore-api/README.md",
  ];
  for (const paths of [...docs.map((path) => [path]), recoveryPolicyPaths]) {
    assert.deepEqual(selectedJobs(paths), [], paths.join(", "));
    assert.deepEqual(selectRustServers(select(paths)), []);
    assert.ok(Object.values(selectChecks(paths)).every((value) => !value));
    for (const runtime of [
      "apps/lane/src/App.tsx",
      "servers/lane-api/src/lib.rs",
      "protos/lane/v1/lane.proto",
      "scripts/ci/plan.mjs",
      "oss/servers/example/src/main.rs",
    ]) {
      assert.deepEqual(select([...paths, runtime]), select([runtime]));
      assert.deepEqual(
        selectChecks([...paths, runtime]),
        selectChecks([runtime]),
      );
    }
  }
});

test("application documentation and non-Markdown inputs keep owning validation", () => {
  for (const [path, owner] of [
    ["apps/delino-docs/docs/en/index.md", "delino_docs"],
    ["apps/delino-site/docs/services/index.md", "delino_site"],
    ["servers/lane-api/ops/alerts.yml", "lane_api"],
    ["servers/lane-api/ops/check.sh", "lane_api"],
    ["servers/lane-api/ops/README.md.rs", "lane_api"],
  ])
    assert.equal(select([path])[owner], true, path);
});

test("Managed CPA paths select the Go server and console validators", () => {
  for (const [path, expected] of [
    ["servers/mcpa-api/main.go", ["mcpa_api"]],
    ["servers/mcpa-api/internal/proxy/meter.go", ["mcpa_api"]],
    ["servers/mcpa-api/migrations/00001_initial.sql", ["mcpa_api"]],
    ["servers/mcpa-api/Dockerfile", ["mcpa_api"]],
    ["servers/mcpa-api/ops/alerts.yml", ["mcpa_api"]],
    ["servers/mcpa-api/package.json", ["mcpa_api"]],
    ["apps/mcpa/src/main.tsx", ["mcpa"]],
    ["apps/mcpa/package.json", ["mcpa"]],
    ["go.mod", ["mcpa_api", "protocol"]],
    ["go.sum", ["mcpa_api", "protocol"]],
    ["servers/LICENSE", undefined],
    ["servers/NOTICE", undefined],
  ]) {
    if (expected) assert.deepEqual(selectedJobs([path]), expected, path);
    else assert.equal(select([path]).mcpa_api, true, path);
  }
  for (const path of [
    "servers/mcpa-api/ops/README.md",
    "servers/mcpa-api/README.md",
  ])
    assert.deepEqual(selectedJobs([path]), [], path);
  // The Go server does not build from the Cargo workspace.
  for (const path of [
    "servers/Cargo.toml",
    "servers/Cargo.lock",
    "servers/crates/server-common/src/lib.rs",
    "Cargo.lock",
    "rust-toolchain",
  ])
    assert.equal(select([path]).mcpa_api, false, path);
  // The server imports only the mcpa and Delcore Go bindings.
  for (const [path, server, frontend] of [
    ["protos/mcpa/v1/mcpa.proto", true, true],
    ["protos/mcpa/v1/mcpa.pb.go", true, true],
    ["protos/delcore/v1/delcore.proto", true, false],
    ["protos/package.json", true, true],
    ["protos/lane/v1/lane.proto", false, false],
    ["protos/devbase/v1/devbase.proto", false, false],
  ]) {
    assert.equal(select([path]).mcpa_api, server, path);
    assert.equal(select([path]).mcpa, frontend, path);
  }
  // Server-wide helpers and shared UI keep the Managed CPA consumers selected.
  assert.equal(select(["servers/scripts/dev-api.mjs"]).mcpa_api, true);
  assert.equal(select(["packages/ui/src/components/button.tsx"]).mcpa, true);
  assert.equal(
    select(["packages/ui/src/components/button.tsx"]).mcpa_api,
    false,
  );
  // Rust server allocation stays unchanged by Managed CPA-only changes.
  for (const path of [
    "servers/mcpa-api/main.go",
    "apps/mcpa/src/main.tsx",
    "go.sum",
  ])
    assert.deepEqual(selectRustServers(select([path])), [], path);
});
