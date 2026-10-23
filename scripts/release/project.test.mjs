// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  chmodSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import yaml from "js-yaml";
import {
  Bump,
  Project,
  bumpVersion,
  prepareRelease,
  releaseTag,
  validateWebSource,
  validateReleaseSource,
  versionChanges,
} from "./project.mjs";

const root = new URL("../../", import.meta.url);
const read = (file) => readFileSync(new URL(file, root), "utf8");
const git = (cwd, ...args) =>
  execFileSync("git", args, {
    cwd,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  }).trim();

test("stable bumps reset lower components and reject malformed or overflowing versions", () => {
  assert.equal(bumpVersion("1.2.3", Bump.Patch), "1.2.4");
  assert.equal(bumpVersion("1.2.3", Bump.Minor), "1.3.0");
  assert.equal(bumpVersion("1.2.3", Bump.Major), "2.0.0");
  for (const version of ["01.2.3", "1.2.3-beta.1", "1.2", "2147483647.0.0"]) {
    assert.throws(() => bumpVersion(version, Bump.Major));
  }
  assert.throws(() => bumpVersion("1.2.3", "next"));
  assert.throws(() => versionChanges("delidev", Bump.Patch, read));
});

for (const project of Object.values(Project)) {
  test(`${project} changes only its synchronized source versions`, () => {
    const plan = versionChanges(project, Bump.Patch, read);
    const app = project === Project.Delino ? "delino-site" : project;
    assert.equal(
      JSON.parse(plan.changes[`apps/${app}/package.json`]).version,
      plan.version,
    );
    const native = project === Project.Delitools;
    assert.equal(
      Object.keys(plan.changes).length,
      native ? 4 : project === Project.Delino ? 2 : 1,
    );
    if (native) {
      assert.equal(
        JSON.parse(plan.changes[`apps/${app}/src-tauri/tauri.conf.json`])
          .version,
        plan.version,
      );
      const entry = "delitools-shell";
      assert.equal(
        plan.changes["Cargo.lock"].replace(
          `name = "${entry}"\nversion = "${plan.version}"`,
          `name = "${entry}"\nversion = "${plan.previous}"`,
        ),
        read("Cargo.lock"),
      );
      assert.throws(
        () =>
          versionChanges(project, Bump.Patch, (file) =>
            file.endsWith("tauri.conf.json")
              ? read(file).replace(
                  `"version": "${plan.previous}"`,
                  '"version": "9.9.9"',
                )
              : read(file),
          ),
        /versions disagree/u,
      );
    }
    assert.equal(plan.tag, `${project}@v${plan.version}`);
    assert.equal(releaseTag(project, plan.version), plan.tag);
  });
}

function fixture(t) {
  const directory = mkdtempSync(join(tmpdir(), "apps-release-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const remote = join(directory, "remote.git");
  const checkout = join(directory, "checkout");
  git(directory, "init", "--bare", "--initial-branch=main", remote);
  git(directory, "clone", remote, checkout);
  git(checkout, "config", "user.name", "Release Fixture");
  git(checkout, "config", "user.email", "release@example.invalid");
  const files = new Set(["Cargo.lock"]);
  for (const project of Object.values(Project)) {
    Object.keys(versionChanges(project, Bump.Patch, read).changes).forEach(
      (file) => files.add(file),
    );
  }
  for (const file of files) {
    mkdirSync(dirname(join(checkout, file)), { recursive: true });
    writeFileSync(join(checkout, file), read(file));
  }
  git(checkout, "add", ".");
  git(checkout, "commit", "-m", "Initial fixture");
  git(checkout, "push", "origin", "main");
  return {
    directory: checkout,
    remote,
    project: Project.Devbase,
    bump: Bump.Patch,
    runId: "12345",
  };
}

test("an atomic release can resume after main advances without another version bump", (t) => {
  const options = fixture(t);
  const result = prepareRelease(options);
  assert.equal(result.resumed, false);
  assert.equal(
    git(options.remote, "rev-parse", "refs/heads/main"),
    result.revision,
  );
  assert.equal(
    git(options.remote, "rev-parse", `refs/tags/${result.tag}`),
    result.revision,
  );
  writeFileSync(
    join(options.directory, "unrelated.txt"),
    "Unrelated main change\n",
  );
  git(options.directory, "add", "unrelated.txt");
  git(options.directory, "commit", "-m", "Advance main");
  git(options.directory, "push", "origin", "main");
  assert.deepEqual(prepareRelease(options), { ...result, resumed: true });
  assert.throws(
    () => prepareRelease({ ...options, bump: Bump.Minor }),
    /inputs changed/u,
  );
  assert.throws(
    () => prepareRelease({ ...options, project: Project.Delitools }),
    /inputs changed/u,
  );
  git(options.directory, "tag", "--delete", result.tag);
  git(options.directory, "tag", result.tag);
  assert.throws(() => prepareRelease(options));
});

test("a rejected atomic push leaves remote main and tags unchanged", (t) => {
  const options = fixture(t);
  const initial = git(options.remote, "rev-parse", "main");
  const hook = join(options.remote, "hooks", "pre-receive");
  writeFileSync(hook, "#!/bin/sh\nexit 1\n");
  chmodSync(hook, 0o755);
  assert.throws(() => prepareRelease(options));
  assert.equal(git(options.remote, "rev-parse", "main"), initial);
  assert.equal(git(options.remote, "tag", "--list"), "");
});

test("workflow run IDs do not match a longer run ID prefix", (t) => {
  const options = fixture(t);
  git(
    options.directory,
    "commit",
    "--allow-empty",
    "-m",
    "Unrelated run",
    "-m",
    "Release-Run: 123456",
  );
  git(options.directory, "push", "origin", "main");
  assert.equal(prepareRelease(options).resumed, false);
});

for (const project of [Project.Delino, Project.Delitools]) {
  test(`${project} can resume all committed version updates`, (t) => {
    const options = { ...fixture(t), project };
    const result = prepareRelease(options);
    assert.deepEqual(prepareRelease(options), { ...result, resumed: true });
  });
}

for (const project of [Project.Devbase, Project.Delino]) {
  test(`${project} source validation rejects branch names, wrong versions and tags outside main`, (t) => {
    const options = { ...fixture(t), project };
    const validate = validateReleaseSource;
    const result = prepareRelease(options);
    // Recovery must resolve the original tagged source even after main advances.
    git(options.directory, "commit", "--allow-empty", "-m", "Advance main");
    git(options.directory, "push", "origin", "main");
    assert.deepEqual(validate(options.directory, project, result.tag), {
      project: result.project,
      version: result.version,
      tag: result.tag,
      revision: result.revision,
    });
    for (const tag of [
      "main",
      "--all",
      `${project}@v1.2.3;touch unsafe`,
      `${project}-v1.2.3`,
      "other@v1.2.3",
    ]) {
      assert.throws(
        () => validate(options.directory, project, tag),
        /Invalid release tag/u,
      );
    }
    git(options.directory, "tag", `${project}@v9.9.9`);
    git(options.directory, "push", "origin", `refs/tags/${project}@v9.9.9`);
    assert.throws(
      () => validate(options.directory, project, `${project}@v9.9.9`),
      /Tag and source version disagree/u,
    );
    git(options.directory, "checkout", "-b", "unmerged");
    const app = project === Project.Delino ? "delino-site" : project;
    const file = `apps/${app}/package.json`;
    const manifest = JSON.parse(
      readFileSync(join(options.directory, file), "utf8"),
    );
    manifest.version = "9.8.7";
    writeFileSync(join(options.directory, file), JSON.stringify(manifest));
    git(options.directory, "add", ".");
    git(options.directory, "commit", "-m", "Unmerged source");
    git(options.directory, "tag", `${project}@v9.8.7`);
    git(options.directory, "push", "origin", `refs/tags/${project}@v9.8.7`);
    assert.throws(() =>
      validate(options.directory, project, `${project}@v9.8.7`),
    );
  });
}

test("web workflows validate main ancestry before allocating protected deployment jobs", () => {
  const release = yaml.load(read(".github/workflows/release-project.yml"));
  assert.deepEqual(
    release.on.workflow_dispatch.inputs.project.options,
    Object.values(Project),
  );
  assert.equal(release.concurrency["cancel-in-progress"], false);
  for (const project of [Project.Devbase, Project.Delino, Project.Delitools]) {
    const workflow = yaml.load(
      read(`.github/workflows/release-${project}.yml`),
    );
    assert.deepEqual(workflow.on.push.tags, [`${project}@v*`]);
    assert.equal(workflow.jobs.source.environment, undefined);
    const job = workflow.jobs.deploy;
    assert.deepEqual(job.needs, "source");
    if (project === Project.Delino) {
      assert.deepEqual(job.strategy.matrix.include, [
        {
          app: "delino-site",
          environment: "delino-site-production",
          concurrency_group: "release-delino-site",
          pages_project_variable: "CLOUDFLARE_PAGES_PROJECT_SITE",
        },
        {
          app: "delino-docs",
          environment: "delino-site-production",
          concurrency_group: "release-delino-docs",
          pages_project_variable: "CLOUDFLARE_PAGES_PROJECT_DOCS",
        },
        {
          app: "devbase",
          environment: "devbase-production",
          concurrency_group: "release-devbase",
          pages_project_variable: "CLOUDFLARE_PAGES_PROJECT",
        },
        {
          app: "git-tracker",
          environment: "delino-site-production",
          concurrency_group: "release-git-tracker",
          pages_project_variable: "CLOUDFLARE_PAGES_PROJECT_GIT_TRACKER",
        },
      ]);
      assert.equal(job.environment, "${{ matrix.environment }}");
      assert.deepEqual(job.concurrency, {
        group: "${{ matrix.concurrency_group }}",
        "cancel-in-progress": false,
      });
      // DevBase must share the owning workflow's gate and concurrency group.
      const devbase = yaml.load(read(".github/workflows/release-devbase.yml"));
      const owned = job.strategy.matrix.include.find(
        (entry) => entry.app === "devbase",
      );
      assert.equal(owned.environment, devbase.jobs.deploy.environment);
      assert.equal(owned.concurrency_group, devbase.concurrency.group);
      assert.equal(job.strategy["fail-fast"], false);
      const configuration = job.steps.find(
        (step) => step.name === "Validate deployment configuration",
      );
      assert.equal(
        configuration.env.CLOUDFLARE_PAGES_PROJECT,
        "${{ vars[matrix.pages_project_variable] }}",
      );
      const upload = job.steps.find((step) =>
        step.uses?.startsWith("cloudflare/wrangler-action@"),
      );
      assert.match(
        upload.with.command,
        /--project-name=\$\{\{ vars\[matrix\.pages_project_variable\] \}\}/u,
      );
      assert.equal(
        job.steps.find((step) => step.run === "pnpm build")[
          "working-directory"
        ],
        "apps/${{ matrix.app }}",
      );
    } else {
      assert.equal(job.environment, `${project}-production`);
    }
    assert.equal(job.steps[0].with.ref, "${{ needs.source.outputs.revision }}");
    const upload = job.steps.findIndex((step) =>
      step.uses?.startsWith("actions/upload-artifact@"),
    );
    const deploy = job.steps.findIndex((step) =>
      step.uses?.startsWith("cloudflare/wrangler-action@"),
    );
    assert.ok(upload >= 0 && upload < deploy);
    assert.match(job.steps[deploy].with.command, /--branch=main/u);
    assert.equal(job.steps.at(-1).if, "always()");
  }
});

test("Delino releases publish server images with narrowly scoped package access", () => {
  const workflow = yaml.load(read(".github/workflows/release-delino.yml"));
  const { jobs } = workflow;
  assert.equal(jobs.source.environment, undefined);
  assert.equal(workflow.permissions.packages, undefined);
  assert.deepEqual(jobs.images.permissions, {
    contents: "read",
    packages: "write",
  });
  assert.equal(jobs.images.needs, "source");
  assert.equal(jobs.images.environment, undefined);
  assert.deepEqual(jobs.images.strategy.matrix.server, [
    "delcore-api",
    "delitools-api",
    "devbase-api",
    "git-tracker-api",
  ]);
  assert.equal(
    jobs.images.steps[0].with.ref,
    "${{ needs.source.outputs.revision }}",
  );
});

test("Delino preparation synchronizes both apps even when their versions differ", () => {
  const plan = versionChanges(Project.Delino, Bump.Minor, (file) => {
    if (file !== "apps/delino-docs/package.json") return read(file);
    const manifest = JSON.parse(read(file));
    manifest.version = "9.9.9";
    return JSON.stringify(manifest, null, 2) + "\n";
  });
  assert.equal(
    plan.version,
    bumpVersion(
      JSON.parse(read("apps/delino-site/package.json")).version,
      Bump.Minor,
    ),
  );
  assert.deepEqual(Object.keys(plan.changes).sort(), [
    "apps/delino-docs/package.json",
    "apps/delino-site/package.json",
  ]);
  for (const content of Object.values(plan.changes)) {
    assert.equal(JSON.parse(content).version, plan.version);
  }
});

test("Delino source validation checks the docs version at the immutable tag", (t) => {
  const options = { ...fixture(t), project: Project.Delino };
  const result = prepareRelease(options);
  const file = join(options.directory, "apps/delino-docs/package.json");
  const manifest = JSON.parse(readFileSync(file, "utf8"));
  manifest.version = "9.9.9";
  writeFileSync(file, JSON.stringify(manifest));
  git(options.directory, "add", ".");
  git(options.directory, "commit", "-m", "Diverge docs version");
  const tag = `delino@v${result.version}`;
  git(options.directory, "tag", "--delete", tag);
  git(options.directory, "tag", tag);
  git(
    options.directory,
    "push",
    "--force",
    "origin",
    "main",
    `refs/tags/${tag}`,
  );
  assert.throws(
    () => validateWebSource(options.directory, Project.Delino, tag),
    /Tag and source version disagree/u,
  );
  assert.throws(
    () =>
      validateWebSource(
        options.directory,
        Project.Delino,
        `delino-site@v${result.version}`,
      ),
    /Invalid release tag/u,
  );
});
