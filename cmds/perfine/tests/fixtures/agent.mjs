// SPDX-License-Identifier: Apache-2.0
// A structured-response fixture only. Real Docker isolation, source checks,
// compilation, correctness, Callgrind and policy verification remain enabled.
import fs from "node:fs";
const prompt = fs.readFileSync(0, "utf8");
const context = JSON.parse(prompt.slice(prompt.indexOf("\n") + 1));
const mode = process.argv.includes("--model")
  ? process.argv[process.argv.indexOf("--model") + 1]
  : "pass";
let answer;
if (context.profile) {
  answer = {
    hypotheses:
      mode === "none"
        ? []
        : [
            {
              explanation: "Replace repeated pair counting with a frequency table",
              source_paths: ["kernel.c"],
              profile_evidence: context.profile.raw_artifacts,
            },
          ],
  };
} else {
  const optimized = fs.readFileSync("/fixture/candidate.c", "utf8");
  const baseline = context.sources["kernel.c"];
  const regression = baseline.replace(
    "return pairs;",
    "for (volatile unsigned i = 0; i < 4000000; ++i) {} return pairs;",
  );
  answer = {
    edits: [
      {
        path: "kernel.c",
        content: mode === "regression" ? regression : optimized,
      },
    ],
  };
}
process.stdout.write(
  JSON.stringify({ type: "result", subtype: "success", is_error: false, structured_output: answer }),
);
