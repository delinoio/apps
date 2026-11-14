// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  mkdtempSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import {
  DeployComponent,
  FullDeployReason,
  main,
  outputName,
  plan,
} from "./deploy-plan.mjs";

function fixture(t) {
  const cwd = mkdtempSync(join(tmpdir(), "deploy-plan-"));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  const git = (...args) =>
    execFileSync("git", args, { cwd, encoding: "utf8" }).trim();
  git("init", "--quiet", "--initial-branch=main");
  git("config", "user.name", "CI fixture");
  git("config", "user.email", "fixture@example.invalid");
  git("config", "core.hooksPath", ".no-hooks");
  const write = (path, content = "fixture\n") => {
    mkdirSync(dirname(join(cwd, path)), { recursive: true });
    writeFileSync(join(cwd, path), content);
  };
  const commit = () => {
    git("add", ".");
    git("commit", "--quiet", "--allow-empty", "-m", "fixture");
    return git("rev-parse", "HEAD");
  };
  write("LICENSE");
  const base = commit();
  return { cwd, git, write, commit, base };
}

const none = {
  [DeployComponent.DelinoSite]: false,
  [DeployComponent.DelinoDocs]: false,
  [DeployComponent.DelinoAccountsWeb]: false,
  [DeployComponent.AccountsApi]: false,
};
const all = Object.fromEntries(Object.keys(none).map((key) => [key, true]));

test("unchanged components and non-deploy paths select nothing", (t) => {
  const f = fixture(t);
  assert.deepEqual(
    plan(f.cwd, { base: f.base, head: f.base }).components,
    none,
  );
  f.write("docs/project-delino-site.md");
  f.write("scripts/ci/plan.mjs");
  f.write(".github/workflows/CI.yml");
  const result = plan(f.cwd, { base: f.base, head: f.commit() });
  assert.equal(result.fullReason, null);
  assert.deepEqual(result.components, none);
});

test("component paths select only their component", async (t) => {
  for (const [path, component] of [
    ["apps/delino-site/docs/index.md", DeployComponent.DelinoSite],
    ["apps/delino-docs/rspress.config.ts", DeployComponent.DelinoDocs],
    ["apps/delino-accounts/src/App.tsx", DeployComponent.DelinoAccountsWeb],
    ["servers/accounts-api/Dockerfile", DeployComponent.AccountsApi],
    ["go.mod", DeployComponent.AccountsApi],
    ["go.sum", DeployComponent.AccountsApi],
  ]) {
    await t.test(path, (t) => {
      const f = fixture(t);
      f.write(path);
      assert.deepEqual(
        plan(f.cwd, { base: f.base, head: f.commit() }).components,
        {
          ...none,
          [component]: true,
        },
      );
    });
  }
});

test("prefix lookalikes do not select a component", (t) => {
  const f = fixture(t);
  f.write("apps/delino-site-old/index.md");
  f.write("servers/accounts-api-v2/main.go");
  assert.deepEqual(
    plan(f.cwd, { base: f.base, head: f.commit() }).components,
    none,
  );
});

test("shared Node workspace inputs select every static site", async (t) => {
  for (const path of [
    ".nvmrc",
    "package.json",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
    "turbo.json",
  ]) {
    await t.test(path, (t) => {
      const f = fixture(t);
      f.write(path);
      assert.deepEqual(
        plan(f.cwd, { base: f.base, head: f.commit() }).components,
        {
          ...all,
          [DeployComponent.AccountsApi]: false,
        },
      );
    });
  }
});

test("deployment workflow and planner changes select every component", async (t) => {
  for (const path of [
    ".github/workflows/deploy.yml",
    ".github/workflows/deploy-cloudflare-pages.yml",
    "scripts/ci/deploy-plan.mjs",
  ]) {
    await t.test(path, (t) => {
      const f = fixture(t);
      f.write(path);
      assert.deepEqual(
        plan(f.cwd, { base: f.base, head: f.commit() }).components,
        all,
      );
    });
  }
});

test("deletions select the owning component", (t) => {
  const f = fixture(t);
  f.write("servers/accounts-api/old.go");
  const base = f.commit();
  f.git("rm", "--quiet", "servers/accounts-api/old.go");
  assert.deepEqual(plan(f.cwd, { base, head: f.commit() }).components, {
    ...none,
    [DeployComponent.AccountsApi]: true,
  });
});

test("changes from superseded runs accumulate since the deployed revision", (t) => {
  const f = fixture(t);
  f.write("apps/delino-site/docs/index.md");
  f.commit();
  f.write("servers/accounts-api/main.go");
  const head = f.commit();
  assert.deepEqual(plan(f.cwd, { base: f.base, head }).components, {
    ...none,
    [DeployComponent.DelinoSite]: true,
    [DeployComponent.AccountsApi]: true,
  });
});

test("missing, unknown or diverged deployed revisions select everything", async (t) => {
  const f = fixture(t);
  f.write("README.md");
  const head = f.commit();
  f.git("checkout", "--quiet", "--orphan", "other");
  f.write("other.md");
  const orphan = f.commit();
  for (const [base, reason] of [
    [undefined, FullDeployReason.NoDeployedRevision],
    ["", FullDeployReason.NoDeployedRevision],
    ["a".repeat(40), FullDeployReason.UnknownDeployedRevision],
    [orphan, FullDeployReason.DeployedRevisionNotAncestor],
  ]) {
    await t.test(String(base), () => {
      const result = plan(f.cwd, { base, head });
      assert.equal(result.fullReason, reason);
      assert.deepEqual(result.components, all);
    });
  }
});

test("manual dispatch selects everything", (t) => {
  const f = fixture(t);
  const result = plan(f.cwd, { base: f.base, head: f.base, manual: true });
  assert.equal(result.fullReason, FullDeployReason.ManualDispatch);
  assert.deepEqual(result.components, all);
});

test("planner rejects an invalid head revision", (t) => {
  const f = fixture(t);
  for (const head of [undefined, "", "a".repeat(40)]) {
    assert.throws(() => plan(f.cwd, { base: f.base, head }));
  }
});

test("planner publishes snake_case outputs and records the plan", (t) => {
  const f = fixture(t);
  f.write("apps/delino-accounts/src/App.tsx");
  const head = f.commit();
  const output = join(f.cwd, "output");
  const summary = join(f.cwd, "summary");
  main(
    {
      DEPLOY_EVENT: "push",
      DEPLOY_BASE: f.base,
      DEPLOY_HEAD: head,
      GITHUB_OUTPUT: output,
      GITHUB_STEP_SUMMARY: summary,
    },
    f.cwd,
  );
  assert.equal(
    readFileSync(output, "utf8"),
    "delino_site=false\ndelino_docs=false\ndelino_accounts_web=true\naccounts_api=false\n",
  );
  assert.equal(
    outputName(DeployComponent.DelinoAccountsWeb),
    "delino_accounts_web",
  );
  const record = JSON.parse(
    readFileSync(summary, "utf8").slice("Deploy plan: ".length),
  );
  assert.equal(record.base, f.base);
  assert.equal(record.head, head);
  assert.equal(record.fullReason, null);
});
