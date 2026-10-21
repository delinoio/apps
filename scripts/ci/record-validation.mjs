import {
  appendFileSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import yaml from "js-yaml";

const job = process.env.VALIDATION_JOB;
const workflow = yaml.load(readFileSync(".github/workflows/CI.yml", "utf8"));
if (!workflow.jobs[job]) throw new Error("Unknown validation job");
const steps = JSON.parse(process.env.VALIDATION_STEPS);
const record = {
  revision: process.env.GITHUB_SHA,
  job,
  result: process.env.VALIDATION_RESULT,
  ...(process.env.VALIDATION_MATRIX
    ? { matrix: JSON.parse(process.env.VALIDATION_MATRIX) }
    : {}),
  comparison: {
    base: process.env.TURBO_SCM_BASE,
    head: process.env.TURBO_SCM_HEAD,
  },
  checks: workflow.jobs[job].steps
    .filter((step) => step.id && step.run)
    .map((step) => ({
      command: step.run,
      result: steps[step.id]?.outcome ?? "not-run",
    })),
  unperformed: [
    "native window launch",
    "device installation",
    "live provider accounts",
    "signed store uploads",
  ],
};
mkdirSync(".ci-evidence", { recursive: true });
writeFileSync(
  `.ci-evidence/${job}.json`,
  JSON.stringify(record, null, 2) + "\n",
);
appendFileSync(
  process.env.GITHUB_STEP_SUMMARY,
  `Revision: ${record.revision}\n\nJob: ${job}; result: ${record.result}\n\nNative/device/account/store acceptance is not established by these checks.\n`,
);
