import assert from "node:assert/strict";
import {
  chmodSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { runInNewContext } from "node:vm";
import yaml from "js-yaml";
import { select, selectChecks } from "./plan.mjs";

const workflow = yaml.load(
  readFileSync(
    new URL("../../.github/workflows/CI.yml", import.meta.url),
    "utf8",
  ),
);

test("ordinary CI preserves affected-only dispatch and Linux allocation", () => {
  assert.equal(workflow.on.workflow_dispatch, null);
  for (const [id, job] of Object.entries(workflow.jobs)) {
    assert.equal(job["runs-on"], "ubuntu-latest");
  }
  for (const [id, owner] of [
    ["devbase-frontend", "devbase"],
    ["delino-console-frontend", "delino_console"],
    ["delino-site-frontend", "delino_site"],
    ["delino-docs-frontend", "delino_docs"],
    ["delidev-frontend", "delidev"],
    ["delitools-frontend", "delitools"],
    ["lane-frontend", "lane"],
    ["lane-cli", "lane_cli"],
    ["protocol", "protocol"],
    ["tooling", "tooling"],
    ["shared-ui", "shared_ui"],
    ["workflows", "workflows"],
  ]) {
    assert.equal(
      workflow.jobs[id].if,
      `needs.changes.outputs.${owner} == 'true'`,
    );
    assert.equal(workflow.jobs[id].needs, "changes");
  }
});

test("Rust jobs retain independent selection, prerequisites and strict commands", () => {
  for (const check of ["clippy", "test"]) {
    const job = workflow.jobs[`rust-${check}`];
    assert.equal(job.if, `needs.changes.outputs.rust_${check} != '[]'`);
    assert.equal(
      job.env.RUST_PACKAGES,
      check === "test"
        ? "${{ format('[\"{0}\"]', matrix.package) }}"
        : "${{ needs.changes.outputs.rust_clippy }}",
    );
    const steps = job.steps;
    assert.equal(
      steps.some((step) => step.run?.includes("apt-get")),
      true,
    );
    const protoc = steps.find((step) => step.id === "protoc");
    assert.equal(
      protoc.if,
      "contains(fromJSON(env.RUST_PACKAGES), 'perfine') || contains(fromJSON(env.RUST_PACKAGES), 'lane')",
    );
    assert.match(protoc.run, /protobuf-compiler libprotobuf-dev/u);
    assert.ok(
      steps.indexOf(protoc) < steps.findIndex((step) => step.id === check),
    );
    const command = steps.find((step) => step.id === check);
    assert.equal(command.run, `node scripts/ci/rust-check.mjs ${check}`);
    for (const [owner, pkg] of [
      ["delidev", "delidev-desktop"],
      ["delitools", "delitools-shell"],
    ]) {
      const build = steps.find((step) => step.id === `${owner}-build`);
      assert.equal(
        build.if,
        check === "test"
          ? `matrix.package == '${pkg}'`
          : `contains(fromJSON(needs.changes.outputs.rust_${check}), '${pkg}')`,
      );
      assert.equal(
        build.run,
        `pnpm exec turbo run build:frontend --filter=@apps/${owner}`,
      );
      assert.ok(steps.indexOf(build) < steps.indexOf(command));
    }
  }
  const steps = workflow.jobs.changes.steps;
  const planner = steps.find((step) => step.id === "rust-plan");
  assert.equal(planner.env.TURBO_SCM_BASE, "${{ steps.plan.outputs.base }}");
  assert.equal(planner.env.TURBO_SCM_HEAD, "${{ steps.plan.outputs.head }}");
  assert.ok(steps.some((step) => step.with?.tool === "cargo-mono@0.6.9"));
});

test("Rust tests fan out the affected crates without uploading artifacts", () => {
  const job = workflow.jobs["rust-test"];
  assert.deepEqual(job.strategy, {
    "fail-fast": false,
    matrix: { package: "${{ fromJSON(needs.changes.outputs.rust_test) }}" },
  });
  assert.equal(job.name, "Rust Test (${{ matrix.package }})");
  assert.ok(
    !job.steps.some((step) =>
      step.uses?.startsWith("actions/upload-artifact@"),
    ),
  );
  assert.ok(!job.steps.some((step) => step.run?.includes("record-validation")));
});

test("Clippy installs only the selected shells' apt packages", (t) => {
  const step = workflow.jobs["rust-clippy"].steps.find(
    (entry) => entry.id === "native-prerequisites",
  );
  const cwd = mkdtempSync(join(tmpdir(), "rust-apt-"));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  const output = join(cwd, "apt.jsonl");
  const sudo = join(cwd, "sudo");
  writeFileSync(
    sudo,
    `#!/usr/bin/env node
require("node:fs").appendFileSync(process.env.APT_TEST_OUTPUT, JSON.stringify(process.argv.slice(2)) + "\\n");
`,
  );
  chmodSync(sudo, 0o755);
  const evaluate = (expression, packages) =>
    runInNewContext(expression.replace(/^\$\{\{ | \}\}$/gu, ""), {
      env: { RUST_PACKAGES: JSON.stringify(packages) },
      fromJSON: JSON.parse,
      contains: (values, value) => values.includes(value),
    });
  for (const packages of [[], ["delidev-desktop"], ["delitools-shell"]]) {
    const cef = packages.some((pkg) =>
      ["delidev-desktop", "delitools-shell"].includes(pkg),
    );
    assert.equal(evaluate(step.if, packages), cef);
    if (!cef) continue;
    rmSync(output, { force: true });
    const result = spawnSync(
      "bash",
      ["-e", "-u", "-o", "pipefail", "-c", step.run],
      {
        encoding: "utf8",
        env: {
          ...process.env,
          PATH: `${cwd}:${process.env.PATH}`,
          APT_TEST_OUTPUT: output,
          ...Object.fromEntries(
            Object.entries(step.env).map(([key, value]) => [
              key,
              String(evaluate(value, packages)),
            ]),
          ),
        },
      },
    );
    assert.equal(result.status, 0, result.stderr);
    const [update, install] = readFileSync(output, "utf8")
      .trim()
      .split("\n")
      .map(JSON.parse);
    assert.deepEqual(update, ["apt-get", "update"]);
    assert.deepEqual(install.slice(0, 3), ["apt-get", "install", "-y"]);
    for (const pkg of [
      "clang",
      "cmake",
      "ninja-build",
      "libgtk-4-dev",
      "libgbm-dev",
      "libx11-dev",
      "libxtst-dev",
      "xdg-desktop-portal",
      "xdg-desktop-portal-gtk",
      "zenity",
    ])
      assert.equal(install.includes(pkg), cef, `${packages}: ${pkg}`);
    for (const pkg of [
      "libwebkit2gtk-4.1-dev",
      "libgtk-3-dev",
      "libayatana-appindicator3-dev",
    ])
      assert.equal(install.includes(pkg), false, `${packages}: ${pkg}`);
  }
});

test("write-capable remote cache credentials stay in trusted main jobs", () => {
  const scenarios = [
    {
      event: "pull_request",
      ref: "refs/pull/61/merge",
      cache: "local:rw",
      token: "",
    },
    {
      event: "pull_request",
      ref: "refs/heads/main",
      cache: "local:rw",
      token: "",
    },
    {
      event: "push",
      ref: "refs/heads/main",
      cache: "local:rw,remote:rw",
      token: "write-token",
    },
    { event: "push", ref: "refs/heads/feature", cache: "local:rw", token: "" },
    {
      event: "workflow_dispatch",
      ref: "refs/heads/main",
      cache: "local:rw,remote:r",
      token: "write-token",
    },
    {
      event: "workflow_dispatch",
      ref: "refs/heads/feature",
      cache: "local:rw",
      token: "",
    },
    {
      event: "pull_request_target",
      ref: "refs/heads/main",
      cache: "local:rw",
      token: "",
    },
  ];
  const evaluate = (expression, context) =>
    runInNewContext(expression.slice(3, -2).trim(), context);
  for (const id of ["rust-clippy", "rust-test"]) {
    const job = workflow.jobs[id];
    for (const scenario of scenarios) {
      const context = {
        github: { event_name: scenario.event, ref: scenario.ref },
        vars: { TURBO_TEAM: "delino" },
        secrets: { TURBO_TOKEN: "write-token" },
      };
      assert.equal(
        evaluate(job.env.TURBO_TOKEN, context),
        scenario.token,
        `${id}: ${scenario.event} ${scenario.ref}`,
      );
      assert.equal(
        evaluate(job.env.TURBO_CACHE, context),
        scenario.cache,
        `${id}: ${scenario.event} ${scenario.ref}`,
      );
    }
    for (const missing of ["team", "token"]) {
      const context = {
        github: { event_name: "push", ref: "refs/heads/main" },
        vars: { TURBO_TEAM: missing === "team" ? "" : "delino" },
        secrets: { TURBO_TOKEN: missing === "token" ? "" : "write-token" },
      };
      assert.equal(evaluate(job.env.TURBO_CACHE, context), "local:rw");
    }
  }
  for (const [id, job] of Object.entries(workflow.jobs)) {
    if (["rust-clippy", "rust-test"].includes(id)) continue;
    assert.equal(job.env?.TURBO_TOKEN, undefined, id);
  }
});

test("all CI formatting checks belong to Check Formatting", () => {
  const job = workflow.jobs["check-formatting"];
  assert.equal(job.name, "Check Formatting");
  assert.equal(job.needs, undefined);
  assert.equal(job.env, undefined);
  for (const step of job.steps) {
    assert.doesNotMatch(step.if ?? "", /needs\.changes/u);
  }
  const commands = new Set(job.steps.map((step) => step.run?.trim()));
  for (const command of [
    "pnpm format:check --all",
    "cargo fmt --all --check",
    "cargo fmt --manifest-path servers/Cargo.toml --all --check",
    "node scripts/ci/check-go-format.mjs protos examples tools/changeset",
    "pnpm exec buf format --diff --exit-code",
  ])
    assert.ok(commands.has(command), command);
  assert.equal(workflow.jobs.changes.outputs.delcore_api, undefined);
  assert.equal(workflow.jobs.changes.outputs.devbase_api, undefined);
  assert.equal(workflow.jobs.changes.outputs.git_tracker_api, undefined);
  for (const [id, config] of Object.entries(workflow.jobs)) {
    if (id === "check-formatting") continue;
    for (const step of config.steps)
      assert.doesNotMatch(
        step.run ?? "",
        /pnpm (?:format:check|proto:lint)|cargo fmt|check-go-format|buf format/u,
        id,
      );
  }
  assert.ok(workflow.jobs["ci-result"].needs.includes("check-formatting"));
  assert.match(
    workflow.jobs.protocol.steps.find((step) => step.id === "unit-protocol")
      .run,
    /pnpm exec buf lint/u,
  );
});

test("validation evidence and main-only cache saves survive the job split", () => {
  for (const id of [
    "tooling",
    "shared-ui",
    "workflows",
    "check-formatting",
    "git-tracker-clients",
    "rust-servers",
    "rust-servers-clippy",
    "devbase-frontend",
    "delino-console-frontend",
    "delino-site-frontend",
    "delino-docs-frontend",
    "delidev-frontend",
    "delitools-frontend",
    "lane-frontend",
    "lane-cli",
    "rust-clippy",
    "protocol",
  ]) {
    const steps = workflow.jobs[id].steps;
    assert.equal(
      steps.find((step) => step.name === "Preserve validation evidence").if,
      "${{ always() }}",
    );
    const upload = steps.find(
      (step) => step.name === "Preserve validation evidence",
    );
    assert.equal(upload.with.path, ".ci-evidence/");
    assert.equal(upload.with["include-hidden-files"], true);
    assert.equal(upload.with["if-no-files-found"], "error");
    const save = steps.find((step) =>
      step.uses?.startsWith("actions/cache/save@"),
    );
    assert.match(save.if, /success\(\).*refs\/heads\/main/u);
  }
});

test("result aggregation rejects every selected failed, canceled or skipped job", () => {
  const command = workflow.jobs["ci-result"].steps[0].run;
  const script = command.split("\n").slice(1, -1).join("\n");
  const run = (needs) =>
    spawnSync(process.execPath, ["--input-type=module", "-e", script], {
      env: { ...process.env, EVENT_NAME: "push", NEEDS: JSON.stringify(needs) },
      encoding: "utf8",
    });
  const needs = Object.fromEntries(
    workflow.jobs["ci-result"].needs.map((id) => [id, { result: "success" }]),
  );
  needs.changes.outputs = {
    rust_servers:
      '[{"service":"delcore-api"},{"service":"delitools-api"},{"service":"devbase-api"},{"service":"git-tracker-api"}]',
    rust_clippy: '["delidev-desktop"]',
    rust_test: '["delidev-desktop", "delitools-shell"]',
    git_tracker: "true",
    devbase: "true",
    delino_console: "true",
    delino_site: "true",
    delino_docs: "true",
    delidev: "true",
    delitools: "true",
    lane: "true",
    lane_cli: "true",
    tooling: "true",
    shared_ui: "true",
    workflows: "true",
    perfine_fixtures: "true",
    protocol: "true",
  };
  assert.equal(run(needs).status, 0);
  const noServers = structuredClone(needs);
  noServers.changes.outputs.rust_servers = "[]";
  noServers["rust-servers"].result = "skipped";
  noServers["rust-servers-clippy"].result = "skipped";
  assert.equal(run(noServers).status, 0);
  noServers["rust-servers"].result = "success";
  assert.notEqual(run(noServers).status, 0);
  for (const id of Object.keys(needs)) {
    for (const result of ["failure", "cancelled", "skipped"]) {
      const changed = structuredClone(needs);
      changed[id].result = result;
      assert.notEqual(run(changed).status, 0, `${id}: ${result}`);
    }
  }
  needs.changes.outputs = {
    rust_servers:
      '[{"service":"delcore-api"},{"service":"delitools-api"},{"service":"devbase-api"},{"service":"git-tracker-api"}]',
    rust_clippy: "[]",
    rust_test: "[]",
    git_tracker: "false",
    devbase: "false",
    delino_console: "false",
    delino_site: "false",
    delino_docs: "false",
    delidev: "false",
    delitools: "false",
    lane: "false",
    lane_cli: "false",
    tooling: "true",
    shared_ui: "true",
    workflows: "true",
    protocol: "true",
  };
  for (const id of [
    "perfine-profiles",
    "perfine-adapters",
    "git-tracker-clients",
    "devbase-frontend",
    "delino-console-frontend",
    "delino-site-frontend",
    "delino-docs-frontend",
    "delidev-frontend",
    "delitools-frontend",
    "lane-frontend",
    "lane-cli",
    "rust-clippy",
    "rust-test",
  ])
    needs[id].result = "skipped";
  assert.equal(run(needs).status, 0);
  // The test job can be selected independently from Clippy.
  needs.changes.outputs.rust_test = '["delitools-shell"]';
  assert.notEqual(run(needs).status, 0);
  needs["rust-test"].result = "success";
  assert.equal(run(needs).status, 0);
  needs["rust-clippy"].result = "success";
  assert.notEqual(run(needs).status, 0);
});

test("every prerequisite failure retains its command and outcome in evidence", (t) => {
  const cwd = mkdtempSync(join(tmpdir(), "validation-evidence-"));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  mkdirSync(join(cwd, ".github/workflows"), { recursive: true });
  writeFileSync(join(cwd, ".github/workflows/CI.yml"), yaml.dump(workflow));
  const recorder = fileURLToPath(
    new URL("./record-validation.mjs", import.meta.url),
  );
  for (const [job, config] of Object.entries(workflow.jobs)) {
    if (!config.steps.some((step) => step.name === "Record validation"))
      continue;
    const beforeRecord = config.steps.slice(
      0,
      config.steps.findIndex((step) => step.name === "Record validation"),
    );
    const commands = beforeRecord.filter((step) => step.run);
    assert.ok(
      commands.every((step) => step.id),
      `${job}: command without an ID`,
    );
    for (const failed of commands) {
      const outcomes = Object.fromEntries(
        commands.map((step) => [
          step.id,
          { outcome: step === failed ? "failure" : "skipped" },
        ]),
      );
      const result = spawnSync(process.execPath, [recorder], {
        cwd,
        encoding: "utf8",
        env: {
          ...process.env,
          VALIDATION_JOB: job,
          VALIDATION_RESULT: "failure",
          VALIDATION_STEPS: JSON.stringify(outcomes),
          GITHUB_SHA: "fixture-revision",
          GITHUB_STEP_SUMMARY: join(cwd, "summary.md"),
        },
      });
      assert.equal(result.status, 0, result.stderr);
      const evidence = JSON.parse(
        readFileSync(join(cwd, `.ci-evidence/${job}.json`), "utf8"),
      );
      assert.equal(evidence.revision, "fixture-revision");
      assert.equal(evidence.result, "failure");
      assert.deepEqual(
        evidence.checks.find((check) => check.command === failed.run),
        { command: failed.run, result: "failure" },
      );
      for (const step of commands.filter((step) => step !== failed)) {
        assert.equal(
          evidence.checks.find((check) => check.command === step.run).result,
          "skipped",
        );
      }
    }
  }
});

test("server validation uses local protocols and PostgreSQL without OCI checks", () => {
  for (const id of ["rust-servers"]) {
    const job = workflow.jobs[id];
    assert.equal(job["runs-on"], "ubuntu-latest");
    assert.ok(workflow.jobs["ci-result"].needs.includes(id));
    const commands = job.steps.map((step) => step.run ?? "").join("\n");
    assert.match(commands, /docker run.*postgres:17/u);
    assert.doesNotMatch(commands, /ci:oci|docker build/u);
    assert.doesNotMatch(commands, /go get|client:setup|delinoio\/servers/u);
  }
  for (const service of ["delcore-api", "devbase-api"]) {
    const dockerfile = readFileSync(
      new URL(`../../servers/${service}/Dockerfile`, import.meta.url),
      "utf8",
    );
    assert.match(dockerfile, /COPY protos \.\/protos/u);
    assert.match(dockerfile, /COPY servers\/LICENSE servers\/NOTICE/u);
    assert.match(dockerfile, /COPY LICENSE NOTICE/u);
  }
});

test("Rust API jobs install Clippy for the repository-selected toolchain", () => {
  const toolchain = readFileSync(
    new URL("../../rust-toolchain", import.meta.url),
    "utf8",
  ).trim();
  for (const id of ["rust-servers-clippy"]) {
    const setup = workflow.jobs[id].steps.find((step) =>
      step.uses?.startsWith("dtolnay/rust-toolchain@"),
    );
    assert.equal(setup.with.toolchain, toolchain, id);
    assert.ok(setup.with.components.split(/,\s*/u).includes("clippy"), id);
  }
});

test("Rust server matrix gates allocation and preserves independent evidence", () => {
  const job = workflow.jobs["rust-servers"];
  assert.equal(job.if, "needs.changes.outputs.rust_servers != '[]'");
  assert.equal(job.needs, "changes");
  assert.equal(job.env.TEST_DATABASE_URL, undefined);
  const integration = job.steps.find((step) => step.id === "integration");
  assert.equal(integration["timeout-minutes"], 10);
  assert.match(integration.run, /--ignored --nocapture/u);
  assert.ok(
    job.steps
      .find((step) => step.id === "integration")
      .env.TEST_DATABASE_URL.includes("matrix.port"),
  );
  assert.equal(job.strategy["fail-fast"], false);
  assert.equal(
    job.strategy.matrix.include,
    "${{ fromJSON(needs.changes.outputs.rust_servers) }}",
  );
  assert.equal(
    workflow.jobs.changes.outputs.rust_servers,
    "${{ steps.plan.outputs.rust_servers }}",
  );
  const record = job.steps.find((step) => step.name === "Record validation");
  assert.equal(record.env.VALIDATION_JOB, "rust-servers");
  assert.equal(record.env.VALIDATION_MATRIX, "${{ toJSON(matrix) }}");
  assert.match(
    job.steps.find((step) => step.name === "Preserve validation evidence").with
      .name,
    /matrix.service/u,
  );
  const commands = job.steps.map((step) => step.run ?? "").join("\n");
  assert.doesNotMatch(commands, /cargo clippy/u);
  assert.match(commands, /cargo test.*-p "\$SERVICE" -- --ignored/u);
  assert.doesNotMatch(commands, /\bgo (?:vet|build|test)\b/u);
  assert.ok(
    !job.steps.some((step) => step.uses?.startsWith("actions/setup-go@")),
  );
  assert.equal(job.env.GOWORK, undefined);
  assert.ok(!workflow.jobs["delcore-api"]);
  assert.ok(!workflow.jobs["devbase-api"]);
  assert.ok(!workflow.jobs["git-tracker-api"]);
});

test("Rust server Clippy runs independently with the affected-service matrix", () => {
  const job = workflow.jobs["rust-servers-clippy"];
  assert.equal(job.needs, "changes");
  assert.equal(job.if, workflow.jobs["rust-servers"].if);
  assert.deepEqual(job.strategy, workflow.jobs["rust-servers"].strategy);
  assert.ok(workflow.jobs["ci-result"].needs.includes("rust-servers-clippy"));
  const commands = job.steps.map((step) => step.run ?? "").join("\n");
  assert.match(
    commands,
    /cargo clippy.*-p "\$SERVICE".*--all-targets -- -D warnings/u,
  );
  assert.doesNotMatch(commands, /cargo test|docker|\bgo (?:vet|build|test)\b/u);
  assert.ok(
    !job.steps.some((step) => step.uses?.startsWith("actions/setup-go@")),
  );
  const record = job.steps.find((step) => step.name === "Record validation");
  assert.equal(record.env.VALIDATION_JOB, "rust-servers-clippy");
  assert.equal(record.env.VALIDATION_MATRIX, "${{ toJSON(matrix) }}");
  assert.match(
    job.steps.find((step) => step.name === "Preserve validation evidence").with
      .name,
    /validation-rust-servers-clippy-.*matrix.service/u,
  );
});

test("shared checks allocate independent jobs from planner outputs", () => {
  assert.equal(workflow.jobs.validate, undefined);
  assert.equal(
    workflow.jobs["shared-ui"].steps.find((step) => step.id === "lynx-ui").run,
    "pnpm --filter @packages/lynx-ui test",
  );
  assert.match(
    workflow.jobs["shared-ui"].steps.find(
      (step) => step.name === "Remove generated output",
    ).run,
    /packages\/lynx-ui\/dist/u,
  );
  for (const [id, key, command] of [
    ["tooling", "tooling", "pnpm test:tooling"],
    ["shared-ui", "shared_ui", "pnpm --filter @packages/ui test"],
    ["workflows", "workflows", "go tool actionlint -shellcheck=''"],
  ]) {
    assert.equal(
      workflow.jobs.changes.outputs[key],
      "${{ steps.plan.outputs." + key + " }}",
    );
    assert.equal(
      workflow.jobs[id].if,
      `needs.changes.outputs.${key} == 'true'`,
    );
    assert.equal(
      workflow.jobs[id].steps.find((step) => step.id === id).run,
      command,
    );
    for (const other of ["tooling", "shared-ui", "workflows"].filter(
      (other) => other !== id,
    )) {
      assert.ok(!workflow.jobs[id].steps.some((step) => step.id === other));
    }
  }
  const script = workflow.jobs["ci-result"].steps[0].run
    .split("\n")
    .slice(1, -1)
    .join("\n");
  for (const paths of [
    ["AGENTS.md", "servers/AGENTS.md"],
    ["packages/ui/src/components/button.tsx"],
    ["scripts/localization/catalog.mjs"],
    ["go.mod"],
    ["apps/devbase/src/main.tsx"],
    ["apps/lane/src/App.tsx"],
  ]) {
    const selection = { ...select(paths), ...selectChecks(paths) };
    const needs = Object.fromEntries(
      workflow.jobs["ci-result"].needs.map((id) => [id, { result: "skipped" }]),
    );
    const jobOwners = {
      tooling: "tooling",
      "shared-ui": "shared_ui",
      workflows: "workflows",
      "git-tracker-clients": "git_tracker",
      "devbase-frontend": "devbase",
      "delino-console-frontend": "delino_console",
      "delino-site-frontend": "delino_site",
      "delino-docs-frontend": "delino_docs",
      "delidev-frontend": "delidev",
      "delitools-frontend": "delitools",
      "lane-frontend": "lane",
      "lane-cli": "lane_cli",
      "perfine-profiles": "perfine_fixtures",
      "perfine-adapters": "perfine_fixtures",
      protocol: "protocol",
    };
    for (const [id, key] of Object.entries(jobOwners))
      needs[id].result = selection[key] ? "success" : "skipped";
    needs.changes = {
      result: "success",
      outputs: {
        ...Object.fromEntries(
          Object.entries(selection).map(([key, value]) => [key, String(value)]),
        ),
        rust_clippy: "[]",
        rust_test: "[]",
        rust_servers: "[]",
      },
    };
    needs["check-formatting"].result = "success";
    const run = (input) =>
      spawnSync(process.execPath, ["--input-type=module", "-e", script], {
        env: {
          ...process.env,
          EVENT_NAME: "push",
          NEEDS: JSON.stringify(input),
        },
        encoding: "utf8",
      });
    assert.equal(run(needs).status, 0, paths.join(", "));
    for (const id of ["tooling", "shared-ui", "workflows"]) {
      const changed = structuredClone(needs);
      changed[id].result =
        needs[id].result === "success" ? "skipped" : "success";
      assert.notEqual(run(changed).status, 0, `${paths}: ${id}`);
    }
  }
});

test("Perfine jobs use owned package contracts and real fixture interfaces", () => {
  for (const [id, owner] of [
    ["perfine-profiles", "perfine_fixtures"],
    ["perfine-adapters", "perfine_fixtures"],
  ]) {
    const job = workflow.jobs[id];
    assert.equal(job.needs, "changes");
    assert.equal(job.if, `needs.changes.outputs.${owner} == 'true'`);
    assert.ok(workflow.jobs["ci-result"].needs.includes(id));
    assert.equal(job.env.TURBO_SCM_BASE, "${{ needs.changes.outputs.base }}");
    assert.equal(job.env.TURBO_SCM_HEAD, "${{ needs.changes.outputs.head }}");
    assert.equal(
      job.steps.find((step) => step.name === "Preserve validation evidence").if,
      "${{ always() }}",
    );
  }
  assert.equal(workflow.jobs["perfine-distribution"], undefined);
  const profiles = workflow.jobs["perfine-profiles"];
  assert.deepEqual(
    [
      ...new Set(
        profiles.strategy.matrix.include.map(({ language }) => language),
      ),
    ],
    ["rust", "c", "cpp", "zig", "go", "javascript"],
  );
  assert.equal(profiles.strategy["fail-fast"], false);
  assert.match(
    profiles.steps.find((step) => step.id === "zig").run,
    /sha256sum --check/u,
  );
  assert.match(
    profiles.steps.find((step) => step.id === "gungraun").run,
    /--version 0\.19\.4/u,
  );
  assert.deepEqual(workflow.jobs["perfine-adapters"].strategy.matrix.language, [
    "rust",
    "c",
    "cpp",
    "zig",
    "go",
    "javascript",
  ]);
  assert.equal(workflow.jobs["perfine-adapters"].strategy["fail-fast"], false);
  assert.match(
    workflow.jobs["perfine-adapters"].steps.find(
      (step) => step.id === "adapters",
    ).run,
    /node scripts\/ci\/perfine-acceptance\.mjs/u,
  );
});

test("Lane integration owns a pinned disposable loopback S3 fixture", () => {
  const job = workflow.jobs["rust-servers"];
  const step = job.steps.find((step) => step.id === "integration");
  assert.match(
    step.run,
    /if \[ "\$SERVICE" = lane-api \]; then\s+python3 "servers\/lane-api\/tests\/integration_fixture\.py"\s+else\s+cargo test/u,
  );
  assert.equal(job["runs-on"], "ubuntu-latest");
  assert.equal(step["timeout-minutes"], 10);
  assert.match(
    job.steps.find((step) => step.id === "protoc").run,
    /if \[ "\$SERVICE" = lane-api \]; then sudo apt-get install -y python3-venv; fi/u,
  );
  const helper = readFileSync(
    new URL(
      "../../servers/lane-api/tests/integration_fixture.py",
      import.meta.url,
    ),
    "utf8",
  );
  assert.match(helper, /MOTO_VERSION = "5\.1\.22"/u);
  assert.match(helper, /moto\[server\]==\{MOTO_VERSION\}/u);
  assert.match(
    helper,
    /ThreadedMotoServer\(ip_address="127\.0\.0\.1", port=0/u,
  );
  assert.match(helper, /client\.create_bucket/u);
  assert.match(helper, /client\.head_bucket/u);
  assert.match(helper, /TemporaryDirectory/u);
  assert.match(helper, /finally:\s+stop\(mock\)/u);
  assert.match(helper, /SIGINT, signal\.SIGTERM/u);
  assert.match(helper, /"--ignored", "--nocapture"/u);
  assert.doesNotMatch(step.run, /docker build|ci:oci/u);
});

test("Lane Ubuntu validation includes extension and desktop RealQA fixtures", () => {
  const job = workflow.jobs["lane-frontend"];
  assert.equal(job["runs-on"], "ubuntu-latest");
  assert.equal(job.if, "needs.changes.outputs.lane == 'true'");
  assert.equal(
    job.steps.find((step) => step.id === "frontend").run.trim(),
    "pnpm --filter @apps/lane test\npnpm --filter @apps/lane-chrome-extension test\nnode --test apps/lane/native/desktop/realqa*.test.mjs",
  );
  const cleanup = job.steps.find(
    (step) => step.name === "Remove generated output",
  );
  assert.equal(cleanup.if, "${{ always() }}");
  assert.equal(
    cleanup.run,
    'rm -rf "apps/lane/dist" "apps/lane-chrome-extension/dist"',
  );
});
