// SPDX-License-Identifier: Apache-2.0
import { execFileSync } from "node:child_process";
import { appendFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Deployable component IDs from the project indexes. The deployment contract
// owns this table: docs/repository-deployment-contract.md.
export const DeployComponent = Object.freeze({
  DelinoSite: "delino-site",
  DelinoDocs: "delino-docs",
  DelinoAccountsWeb: "delino-accounts-web",
  AccountsApi: "accounts-api",
});

// Why a plan selects every component instead of a changed subset.
export const FullDeployReason = Object.freeze({
  ManualDispatch: "manual-dispatch",
  NoDeployedRevision: "no-deployed-revision",
  UnknownDeployedRevision: "unknown-deployed-revision",
  DeployedRevisionNotAncestor: "deployed-revision-not-ancestor",
});

// Changes to the deployment workflow or planner redeploy everything.
const deployInputs = [
  ".github/workflows/deploy.yml",
  ".github/workflows/deploy-cloudflare-pages.yml",
  "scripts/ci/deploy-plan.mjs",
];

// Root files that every pnpm workspace build reads.
const nodeWorkspaceInputs = [
  ".nvmrc",
  "package.json",
  "pnpm-lock.yaml",
  "pnpm-workspace.yaml",
  "turbo.json",
];

// Root Go module files. accounts-api imports no other module directories yet;
// add them here when it does.
const goModuleInputs = ["go.mod", "go.sum"];

const componentInputs = {
  [DeployComponent.DelinoSite]: {
    files: nodeWorkspaceInputs,
    directories: ["apps/delino-site/"],
  },
  [DeployComponent.DelinoDocs]: {
    files: nodeWorkspaceInputs,
    directories: ["apps/delino-docs/"],
  },
  [DeployComponent.DelinoAccountsWeb]: {
    files: nodeWorkspaceInputs,
    directories: ["apps/delino-accounts/"],
  },
  [DeployComponent.AccountsApi]: {
    files: goModuleInputs,
    directories: ["servers/accounts-api/"],
  },
};

// GitHub Actions output names cannot be referenced with dot syntax when they
// contain hyphens, so outputs use snake_case component IDs.
export function outputName(component) {
  return component.replaceAll("-", "_");
}

function git(cwd, args) {
  return execFileSync("git", args, { cwd, encoding: "utf8" });
}

function isCommit(cwd, revision) {
  try {
    git(cwd, ["cat-file", "-e", `${revision}^{commit}`]);
    return true;
  } catch {
    return false;
  }
}

function isAncestor(cwd, base, head) {
  try {
    git(cwd, ["merge-base", "--is-ancestor", base, head]);
    return true;
  } catch {
    return false;
  }
}

function selectAll(value) {
  return Object.fromEntries(
    Object.values(DeployComponent).map((component) => [component, value]),
  );
}

// Plans the components to deploy for `head`. `base` is the head revision of
// the last fully successful deployment run; every component whose inputs
// changed since then is selected, so runs that failed or were superseded do
// not drop changes.
export function plan(cwd, { base, head, manual }) {
  head = git(cwd, ["rev-parse", "--verify", `${head}^{commit}`]).trim();
  let fullReason;
  if (manual) {
    fullReason = FullDeployReason.ManualDispatch;
  } else if (!base) {
    fullReason = FullDeployReason.NoDeployedRevision;
  } else if (!isCommit(cwd, base)) {
    fullReason = FullDeployReason.UnknownDeployedRevision;
  } else if (!isAncestor(cwd, base, head)) {
    fullReason = FullDeployReason.DeployedRevisionNotAncestor;
  }
  if (fullReason) {
    return {
      base: base || null,
      head,
      fullReason,
      components: selectAll(true),
    };
  }

  const paths = git(cwd, [
    "diff",
    "--name-only",
    "--no-renames",
    "-z",
    base,
    head,
    "--",
  ])
    .split("\0")
    .filter(Boolean);
  if (paths.some((path) => deployInputs.includes(path))) {
    return { base, head, fullReason: null, components: selectAll(true) };
  }
  const components = Object.fromEntries(
    Object.entries(componentInputs).map(([component, inputs]) => [
      component,
      paths.some(
        (path) =>
          inputs.files.includes(path) ||
          inputs.directories.some((directory) => path.startsWith(directory)),
      ),
    ]),
  );
  return { base, head, fullReason: null, components };
}

export function main(env = process.env, cwd = process.cwd()) {
  const result = plan(cwd, {
    base: env.DEPLOY_BASE,
    head: env.DEPLOY_HEAD,
    manual: env.DEPLOY_EVENT === "workflow_dispatch",
  });
  const record = { event: "deploy_plan", ...result };
  console.log(JSON.stringify(record));
  appendFileSync(
    env.GITHUB_OUTPUT,
    Object.entries(result.components)
      .map(([component, value]) => `${outputName(component)}=${value}\n`)
      .join(""),
  );
  appendFileSync(
    env.GITHUB_STEP_SUMMARY,
    `Deploy plan: ${JSON.stringify(record)}\n`,
  );
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  main();
}
