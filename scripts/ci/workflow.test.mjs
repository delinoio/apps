import assert from "node:assert/strict";
import {
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
import yaml from "js-yaml";

const workflow = yaml.load(
  readFileSync(
    new URL("../../.github/workflows/CI.yml", import.meta.url),
    "utf8",
  ),
);

test("ordinary CI preserves affected-only dispatch and Linux allocation", () => {
  assert.equal(workflow.on.workflow_dispatch, null);
  for (const [id, job] of Object.entries(workflow.jobs)) {
    if (id === "fixkey-desktop") {
      assert.deepEqual(job.strategy.matrix.os, [
        "ubuntu-latest",
        "windows-2022",
      ]);
      assert.equal(
        job.if,
        "github.event_name == 'push' && needs.changes.outputs.fixkey == 'true'",
      );
    } else assert.equal(job["runs-on"], "ubuntu-latest");
  }
  for (const [id, owner] of [
    ["devbase-frontend", "devbase"],
    ["delino-site-frontend", "delino_site"],
    ["delino-docs-frontend", "delino_docs"],
    ["delidev-frontend", "delidev"],
    ["delitools-frontend", "delitools"],
    ["protocol", "protocol"],
    ["delcore-api", "delcore_api"],
    ["git-tracker-api", "git_tracker_api"],
    ["fixkey-frontend", "fixkey"],
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
      "${{ needs.changes.outputs.rust_" + check + " }}",
    );
    const steps = job.steps;
    assert.ok(
      steps.some((step) => step.run?.includes("libwebkit2gtk-4.1-dev")),
    );
    const command = steps.find((step) => step.id === check);
    assert.equal(command.run, `node scripts/ci/rust-check.mjs ${check}`);
    for (const [owner, pkg] of [
      ["delidev", "delidev-desktop"],
      ["delitools", "delitools-shell"],
      ["fixkey", "fixkey"],
    ]) {
      const build = steps.find((step) => step.id === `${owner}-build`);
      assert.equal(
        build.if,
        `contains(fromJSON(needs.changes.outputs.rust_${check}), '${pkg}')`,
      );
      assert.equal(
        build.run,
        `pnpm exec turbo run build:frontend --filter=${owner === "fixkey" ? "fixkey-desktop" : owner === "delitools" ? "delitools" : "delidev-desktop"}`,
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

test("remote caching is limited to frontend builds with main-push writes", () => {
  for (const id of ["fixkey-desktop", "rust-clippy", "rust-test"]) {
    const job = workflow.jobs[id];
    assert.equal(job.env.TURBO_TEAM, "${{ vars.TURBO_TEAM }}");
    assert.equal(job.env.TURBO_TOKEN, "${{ secrets.TURBO_TOKEN }}");
    assert.equal(
      job.env.TURBO_CACHE,
      "${{ vars.TURBO_TEAM != '' && secrets.TURBO_TOKEN != '' && (github.event_name == 'push' && github.ref == 'refs/heads/main' && 'local:rw,remote:rw' || 'local:rw,remote:r') || 'local:rw' }}",
    );
  }
  assert.equal(workflow.permissions["id-token"], undefined);
  for (const [id, job] of Object.entries(workflow.jobs)) {
    if (["fixkey-desktop", "rust-clippy", "rust-test"].includes(id)) continue;
    assert.equal(job.env?.TURBO_TOKEN, undefined, id);
  }
});

test("all CI formatting checks belong to Check Formatting", () => {
  const job = workflow.jobs["check-formatting"];
  assert.equal(job.name, "Check Formatting");
  assert.equal(job.needs, "changes");
  assert.equal(job.env.TURBO_SCM_BASE, "${{ needs.changes.outputs.base }}");
  assert.equal(job.env.TURBO_SCM_HEAD, "${{ needs.changes.outputs.head }}");
  const commands = new Map(
    job.steps.filter((step) => step.run).map((step) => [step.run, step]),
  );
  assert.ok(commands.has("pnpm format:check"));
  assert.equal(
    commands.get("cargo fmt --all --check").if,
    "needs.changes.outputs.rust_clippy != '[]'",
  );
  for (const command of [
    "node scripts/ci/check-go-format.mjs protos examples",
    "pnpm exec buf format --diff --exit-code",
  ])
    assert.equal(
      commands.get(command).if,
      "needs.changes.outputs.protocol == 'true'",
    );
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
    "validate",
    "check-formatting",
    "git-tracker-clients",
    "delcore-api",
    "git-tracker-api",
    "devbase-frontend",
    "delino-site-frontend",
    "delino-docs-frontend",
    "delidev-frontend",
    "delitools-frontend",
    "rust-clippy",
    "rust-test",
    "protocol",
    "fixkey-frontend",
    "fixkey-desktop",
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
    rust_clippy: '["fixkey-core"]',
    rust_test: '["fixkey-core", "fixkey"]',
    git_tracker: "true",
    fixkey: "true",
    devbase: "true",
    delino_site: "true",
    delino_docs: "true",
    delidev: "true",
    delitools: "true",
    delcore_api: "true",
    git_tracker_api: "true",
    protocol: "true",
  };
  assert.equal(run(needs).status, 0);
  for (const id of Object.keys(needs)) {
    for (const result of ["failure", "cancelled", "skipped"]) {
      const changed = structuredClone(needs);
      changed[id].result = result;
      assert.notEqual(run(changed).status, 0, `${id}: ${result}`);
    }
  }
  needs.changes.outputs = {
    rust_clippy: "[]",
    rust_test: "[]",
    git_tracker: "false",
    fixkey: "false",
    devbase: "false",
    delino_site: "false",
    delino_docs: "false",
    delidev: "false",
    delitools: "false",
    delcore_api: "true",
    git_tracker_api: "true",
    protocol: "true",
  };
  for (const id of [
    "git-tracker-clients",
    "devbase-frontend",
    "delino-site-frontend",
    "delino-docs-frontend",
    "delidev-frontend",
    "delitools-frontend",
    "rust-clippy",
    "rust-test",
    "fixkey-frontend",
    "fixkey-desktop",
  ])
    needs[id].result = "skipped";
  assert.equal(run(needs).status, 0);
  // The test job can be selected independently from Clippy.
  needs.changes.outputs.rust_test = '["fixkey"]';
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

test("server validation uses local protocols, PostgreSQL and licensed OCI packaging", () => {
  for (const id of ["delcore-api", "git-tracker-api"]) {
    const job = workflow.jobs[id];
    assert.equal(job["runs-on"], "ubuntu-latest");
    assert.ok(workflow.jobs["ci-result"].needs.includes(id));
    const commands = job.steps.map((step) => step.run ?? "").join("\n");
    assert.match(commands, /go vet \.\/servers\//u);
    assert.match(commands, /docker run.*postgres:17/u);
    assert.doesNotMatch(commands, /go get|client:setup|delinoio\/servers/u);
  }
  const dockerfile = readFileSync(
    new URL("../../servers/delcore-api/Dockerfile", import.meta.url),
    "utf8",
  );
  assert.match(dockerfile, /COPY protos \.\/protos/u);
  assert.match(dockerfile, /COPY servers\/LICENSE servers\/NOTICE/u);
  assert.match(dockerfile, /COPY LICENSE NOTICE/u);
});
