// SPDX-License-Identifier: Apache-2.0
import { clone, create, fromJsonString } from "@bufbuild/protobuf";
import * as P from "@delino/protos/perfine/v1/perfine_pb";
import { Field, Button } from "./ui";
import { NativeSelect } from "@delino/ui/components/native-select";

export function emptyConfiguration() {
  return create(P.ExperimentConfigurationSchema, {
    schemaVersion: 1,
    environment: { tools: [{ name: "", version: "" }] },
    inputs: [{ name: "", sha256: "" }],
    cases: [
      {
        id: "",
        family: "",
        frequency: 1,
        frequencySource: P.FrequencySource.ASSUMED_ONE,
      },
    ],
    limits: {
      candidateCount: 1,
      simultaneousCandidates: 1,
      startDuration: { seconds: 3600n },
      candidateTimeout: { seconds: 600n },
    },
    policy: irPolicy(),
  });
}
function irPolicy() {
  return create(P.PerformancePolicySchema, {
    schemaVersion: 1,
    kind: P.PolicyKind.CALLGRIND_IR,
    metric: {
      name: "Ir",
      unit: "instructions",
      engineKind: P.EngineMetricKind.IR,
      objective: P.MetricObjective.MINIMIZE,
    },
    minimumImprovement: 0.01,
    maximumCaseRegression: 0.01,
    repetitions: 1,
    uncertainty: {
      engineSchemaVersion: 1,
      nearThreshold: 0.001,
      repeatTolerance: 0.001,
    },
    allowedExceptions: [
      P.PolicyException.BELOW_MINIMUM_IMPROVEMENT,
      P.PolicyException.SINGLE_UNPROTECTED_REGRESSION,
    ],
  });
}
const lines = (s: string) => s.split("\n").filter((x) => x.trim());
export function ConfigurationForm({
  value: c,
  onChange,
  onPrepare,
  disabled,
  pending,
}: {
  value: P.ExperimentConfiguration;
  onChange: (c: P.ExperimentConfiguration) => void;
  onPrepare: () => void;
  disabled: boolean;
  pending: boolean;
}) {
  const edit = (fn: (next: P.ExperimentConfiguration) => void) => {
    const next = clone(P.ExperimentConfigurationSchema, c);
    fn(next);
    onChange(next);
  };
  const text = (
    label: string,
    value: string,
    change: (v: string) => void,
    pattern?: string,
    required = true,
  ) => (
    <Field
      label={label}
      value={value}
      required={required}
      pattern={pattern}
      onChange={(e) => change(e.target.value)}
    />
  );
  const number = (
    label: string,
    value: number,
    change: (v: number) => void,
    min = 0,
    max?: number,
    step = "any",
  ) => (
    <Field
      label={label}
      type="number"
      required
      min={min}
      max={max}
      step={step}
      value={Number.isFinite(value) ? value : ""}
      onChange={(e) =>
        change(e.target.value === "" ? NaN : Number(e.target.value))
      }
    />
  );
  const select = (
    label: string,
    value: number,
    values: [number, string][],
    change: (v: number) => void,
  ) => (
    <label className="field">
      {label}
      <NativeSelect
        value={value}
        onChange={(e) => change(Number(e.target.value))}
      >
        {values.map(([v, label]) => (
          <option key={v} value={v}>
            {label}
          </option>
        ))}
      </NativeSelect>
    </label>
  );
  const multiline = (
    label: string,
    value: string[],
    change: (v: string[]) => void,
    required = true,
  ) => (
    <label className="field">
      {label}
      <textarea
        required={required}
        value={value.join("\n")}
        onChange={(e) => change(e.target.value.split("\n"))}
      />
    </label>
  );
  const p = c.policy!,
    env = c.environment!,
    limits = c.limits!;
  return (
    <form
      className="stack"
      onSubmit={(e) => {
        e.preventDefault();
        if (!disabled && !pending) onPrepare();
      }}
    >
      <fieldset disabled={disabled} className="stack">
        <legend>Source and workload</legend>
        {(
          [
            ["Base SHA", "baseSha"],
            ["Initial base SHA", "initialBaseSha"],
            ["Harness SHA", "harnessSha"],
            ["Workflow SHA", "workflowSha"],
          ] as const
        ).map(([label, key]) => (
          <div key={key}>
            {text(
              label,
              c[key],
              (v) =>
                edit((n) => {
                  n[key] = v;
                }),
              "[a-fA-F0-9]{40}",
            )}
          </div>
        ))}
        {select(
          "Language",
          c.language,
          [
            [0, "Select language"],
            [P.Language.RUST, "Rust"],
            [P.Language.C, "C"],
            [P.Language.CPP, "C++"],
            [P.Language.ZIG, "Zig"],
            [P.Language.GO, "Go"],
            [P.Language.JAVASCRIPT, "JavaScript"],
          ],
          (v) =>
            edit((n) => {
              n.language = v;
            }),
        )}
        <h3>Workload inputs</h3>
        <p>
          Supply SHA-256 hashes of the actual input files. These are input
          identities; preparation computes configuration fingerprints.
        </p>
        {c.inputs.map((input, i) => (
          <div className="case-card stack" key={i}>
            {text(`Input ${i + 1} name`, input.name, (v) =>
              edit((n) => {
                for (const item of n.cases)
                  for (const reference of item.inputs) {
                    if (reference.name === n.inputs[i].name) reference.name = v;
                  }
                n.inputs[i].name = v;
              }),
            )}
            {text(
              `Input ${i + 1} SHA-256`,
              input.sha256,
              (v) =>
                edit((n) => {
                  for (const item of n.cases)
                    for (const reference of item.inputs) {
                      if (reference.name === n.inputs[i].name)
                        reference.sha256 = v;
                    }
                  n.inputs[i].sha256 = v;
                }),
              "[a-fA-F0-9]{64}",
            )}
            <Button
              type="button"
              variant="outline"
              disabled={c.inputs.length === 1}
              onClick={() =>
                edit((n) => {
                  for (const item of n.cases)
                    item.inputs = item.inputs.filter(
                      (reference) => reference.name !== n.inputs[i].name,
                    );
                  n.inputs.splice(i, 1);
                })
              }
            >
              Remove input {i + 1}
            </Button>
          </div>
        ))}
        <Button
          type="button"
          variant="outline"
          onClick={() =>
            edit((n) => {
              n.inputs.push(create(P.InputHashSchema));
            })
          }
        >
          Add input
        </Button>
        <h3>Workload cases</h3>
        {c.cases.map((item, i) => (
          <div className="case-card stack" key={i}>
            {text(`Case ${i + 1} ID`, item.id, (v) =>
              edit((n) => {
                n.cases[i].id = v;
              }),
            )}
            {text(`Case ${i + 1} family`, item.family, (v) =>
              edit((n) => {
                n.cases[i].family = v;
              }),
            )}
            {select(
              `Case ${i + 1} frequency source`,
              item.frequencySource,
              [
                [P.FrequencySource.ASSUMED_ONE, "Unknown frequency — assume 1"],
                [P.FrequencySource.DECLARED, "Declared frequency"],
              ],
              (v) =>
                edit((n) => {
                  n.cases[i].frequencySource = v;
                  n.cases[i].frequency = 1;
                }),
            )}
            {item.frequencySource === P.FrequencySource.DECLARED &&
              number(
                `Case ${i + 1} frequency`,
                item.frequency,
                (v) =>
                  edit((n) => {
                    n.cases[i].frequency = v;
                  }),
                Number.MIN_VALUE,
              )}
            <label className="check">
              <input
                type="checkbox"
                checked={item.protectedCase}
                onChange={(e) =>
                  edit((n) => {
                    n.cases[i].protectedCase = e.target.checked;
                  })
                }
              />
              Protect case {i + 1}
            </label>
            <Button
              type="button"
              variant="outline"
              disabled={c.cases.length === 1}
              onClick={() =>
                edit((n) => {
                  n.cases.splice(i, 1);
                })
              }
            >
              Remove case {i + 1}
            </Button>
          </div>
        ))}
        <Button
          type="button"
          variant="outline"
          onClick={() =>
            edit((n) => {
              n.cases.push(
                create(P.WorkloadCaseSchema, {
                  frequency: 1,
                  frequencySource: P.FrequencySource.ASSUMED_ONE,
                }),
              );
            })
          }
        >
          Add case
        </Button>
        {multiline(
          "Correctness checks (one per line)",
          c.correctnessChecks,
          (v) =>
            edit((n) => {
              n.correctnessChecks = v;
            }),
        )}
        {text(
          "Workload representativeness confirmation",
          c.workloadConfirmation,
          (v) =>
            edit((n) => {
              n.workloadConfirmation = v;
            }),
        )}
        <p>
          Explain why these workloads exercise the intended paths. Workload
          assertions do not replace correctness checks.
        </p>
        {multiline(
          "Parent candidate IDs (optional, one per line)",
          c.parentCandidateIds,
          (v) =>
            edit((n) => {
              n.parentCandidateIds = lines(v.join("\n"));
            }),
          false,
        )}
      </fieldset>
      <fieldset disabled={disabled} className="stack">
        <legend>Tools and environment</legend>
        <p>
          Enter public reproducibility settings only. Never enter credentials or
          environment secrets. Include the exact toolchain version in the
          toolchain description.
        </p>
        {(
          [
            ["Operating system", "operatingSystem"],
            ["Architecture", "architecture"],
            ["Runner", "runner"],
            ["Toolchain and version", "runtime"],
          ] as const
        ).map(([label, key]) => (
          <div key={key}>
            {text(label, env[key], (v) =>
              edit((n) => {
                n.environment![key] = v;
              }),
            )}
          </div>
        ))}
        {env.tools.map((tool, i) => (
          <div className="case-card stack" key={i}>
            {text(`Tool ${i + 1} name`, tool.name, (v) =>
              edit((n) => {
                n.environment!.tools[i].name = v;
              }),
            )}
            {text(`Tool ${i + 1} version`, tool.version, (v) =>
              edit((n) => {
                n.environment!.tools[i].version = v;
              }),
            )}
            <Button
              type="button"
              variant="outline"
              disabled={env.tools.length === 1}
              onClick={() =>
                edit((n) => {
                  n.environment!.tools.splice(i, 1);
                })
              }
            >
              Remove tool {i + 1}
            </Button>
          </div>
        ))}
        <Button
          type="button"
          variant="outline"
          onClick={() =>
            edit((n) => {
              n.environment!.tools.push(create(P.ToolVersionSchema));
            })
          }
        >
          Add tool
        </Button>
        {multiline(
          "Build options (one per line)",
          env.buildOptions,
          (v) =>
            edit((n) => {
              n.environment!.buildOptions = v;
            }),
          false,
        )}
        {multiline(
          "Runtime flags (one per line)",
          env.runtimeFlags,
          (v) =>
            edit((n) => {
              n.environment!.runtimeFlags = v;
            }),
          false,
        )}
        {text("CPU / hardware identity", env.hardware.cpu || "", (v) =>
          edit((n) => {
            n.environment!.hardware.cpu = v;
          }),
        )}
      </fieldset>
      <fieldset disabled={disabled} className="stack">
        <legend>Candidate limits</legend>
        {number(
          "Candidate count",
          limits.candidateCount,
          (v) =>
            edit((n) => {
              n.limits!.candidateCount = v;
            }),
          1,
          undefined,
          "1",
        )}
        {number(
          "Simultaneous candidates",
          limits.simultaneousCandidates,
          (v) =>
            edit((n) => {
              n.limits!.simultaneousCandidates = v;
            }),
          1,
          Math.min(5, limits.candidateCount),
          "1",
        )}
        {text(
          "New candidate start duration (seconds)",
          limits.startDuration!.seconds.toString(),
          (v) => {
            if (/^\d*$/.test(v))
              edit((n) => {
                n.limits!.startDuration!.seconds = BigInt(v || "0");
              });
          },
          "[0-9]+",
        )}
        {text(
          "Per-candidate timeout (seconds)",
          limits.candidateTimeout!.seconds.toString(),
          (v) => {
            if (/^\d*$/.test(v))
              edit((n) => {
                n.limits!.candidateTimeout!.seconds = BigInt(v || "0");
              });
          },
          "[0-9]+",
        )}
        <p>
          The server sets the start deadline at preparation time. After that
          deadline, active candidates may finish within their timeout.
        </p>
      </fieldset>
      <fieldset disabled={disabled} className="stack">
        <legend>Metric and policy</legend>
        {select(
          "Policy",
          p.kind,
          [
            [P.PolicyKind.CALLGRIND_IR, "Callgrind Ir preset"],
            [P.PolicyKind.DECLARED, "Declared policy"],
            [
              P.PolicyKind.MEASUREMENTS_ONLY,
              "Measurements only — no pass claim",
            ],
          ],
          (v) =>
            edit((n) => {
              n.policy =
                v === P.PolicyKind.CALLGRIND_IR
                  ? irPolicy()
                  : create(P.PerformancePolicySchema, {
                      schemaVersion: 1,
                      kind: v,
                      metric: { objective: P.MetricObjective.MINIMIZE },
                      ...(v === P.PolicyKind.DECLARED
                        ? {
                            repetitions: 2,
                            uncertainty: {
                              engineSchemaVersion: 1,
                              nearThreshold: 0,
                              repeatTolerance: 0,
                              maximumRelativeRange: 0,
                              decisionMargin: 0,
                            },
                          }
                        : {}),
                    });
            }),
        )}
        {p.kind === P.PolicyKind.CALLGRIND_IR ? (
          <p>
            Ir measures instruction count, not elapsed time. One measurement
            plus one identical-condition repeat when required.
          </p>
        ) : (
          <>
            {select(
              "Metric kind",
              p.metric!.engineKind,
              [
                [0, "Select metric kind"],
                [P.EngineMetricKind.TIME, "Time"],
                [P.EngineMetricKind.THROUGHPUT, "Throughput"],
                [P.EngineMetricKind.MEMORY, "Memory"],
                [P.EngineMetricKind.CUSTOM, "Custom"],
                ...(p.kind === P.PolicyKind.MEASUREMENTS_ONLY
                  ? [
                      [P.EngineMetricKind.IR, "Instruction count"] as [
                        number,
                        string,
                      ],
                    ]
                  : []),
              ],
              (v) =>
                edit((n) => {
                  n.policy!.metric!.engineKind = v;
                }),
            )}
            {text("Metric name", p.metric!.name, (v) =>
              edit((n) => {
                n.policy!.metric!.name = v;
              }),
            )}
            {text("Metric unit", p.metric!.unit, (v) =>
              edit((n) => {
                n.policy!.metric!.unit = v;
              }),
            )}
            {select(
              "Objective",
              p.metric!.objective,
              [
                [P.MetricObjective.MINIMIZE, "Minimize"],
                [P.MetricObjective.MAXIMIZE, "Maximize"],
              ],
              (v) =>
                edit((n) => {
                  n.policy!.metric!.objective = v;
                }),
            )}
          </>
        )}
        {p.kind !== P.PolicyKind.MEASUREMENTS_ONLY && (
          <>
            <p>
              Policy values below are fractions: 0.01 means 1%. A sample range
              is (maximum − minimum) / mean, not a confidence interval.
            </p>
            {number(
              "Minimum portfolio improvement",
              p.minimumImprovement,
              (v) =>
                edit((n) => {
                  n.policy!.minimumImprovement = v;
                }),
              0,
              0.999999,
            )}
            {number(
              "Maximum case regression",
              p.maximumCaseRegression,
              (v) =>
                edit((n) => {
                  n.policy!.maximumCaseRegression = v;
                }),
              0,
              0.999999,
            )}
            {p.kind === P.PolicyKind.DECLARED &&
              number(
                "Repetitions",
                p.repetitions,
                (v) =>
                  edit((n) => {
                    n.policy!.repetitions = v;
                  }),
                2,
                undefined,
                "1",
              )}
            {(
              [
                "nearThreshold",
                "repeatTolerance",
                ...(p.kind === P.PolicyKind.DECLARED
                  ? (["maximumRelativeRange", "decisionMargin"] as const)
                  : []),
              ] as const
            ).map((key) => (
              <div key={key}>
                {number(
                  {
                    nearThreshold: "Near-threshold margin",
                    repeatTolerance: "Repeat tolerance",
                    maximumRelativeRange: "Maximum relative sample range",
                    decisionMargin: "Decision margin",
                  }[key],
                  p.uncertainty![key]!,
                  (v) =>
                    edit((n) => {
                      n.policy!.uncertainty![key] = v;
                    }),
                  0,
                  0.999999,
                )}
              </div>
            ))}
            {p.kind === P.PolicyKind.CALLGRIND_IR &&
              [
                [
                  P.PolicyException.BELOW_MINIMUM_IMPROVEMENT,
                  "Allow below-minimum improvement only with both portfolios improving and equally simple or simpler code",
                ],
                [
                  P.PolicyException.SINGLE_UNPROTECTED_REGRESSION,
                  "Allow single unprotected regression under the documented evidence conditions",
                ],
              ].map(([value, label]) => (
                <label className="check" key={value}>
                  <input
                    type="checkbox"
                    checked={p.allowedExceptions.includes(
                      value as P.PolicyException,
                    )}
                    onChange={(e) =>
                      edit((n) => {
                        n.policy!.allowedExceptions = e.target.checked
                          ? [
                              ...n.policy!.allowedExceptions,
                              value as P.PolicyException,
                            ]
                          : n.policy!.allowedExceptions.filter(
                              (x) => x !== value,
                            );
                      })
                    }
                  />
                  {label}
                </label>
              ))}
          </>
        )}
      </fieldset>
      <Button type="submit" disabled={disabled || pending}>
        {pending ? "Preparing…" : "Prepare experiment for review"}
      </Button>
    </form>
  );
}

export function validateConfiguration(c: P.ExperimentConfiguration) {
  const sha = /^[a-fA-F0-9]{40}$/,
    hash = /^[a-fA-F0-9]{64}$/;
  const env = c.environment,
    l = c.limits,
    p = c.policy;
  if (
    ![c.baseSha, c.initialBaseSha, c.harnessSha, c.workflowSha].every((v) =>
      sha.test(v),
    ) ||
    !c.language
  )
    throw new Error(
      "Enter exact source, initial base, harness and workflow SHAs and select a language.",
    );
  if (
    !c.inputs.length ||
    c.inputs.some((i) => !i.name.trim() || !hash.test(i.sha256)) ||
    new Set(c.inputs.map((i) => i.name)).size !== c.inputs.length
  )
    throw new Error("Supply unique input names and actual SHA-256 hashes.");
  if (
    !c.cases.length ||
    c.cases.some(
      (x) =>
        !x.id.trim() ||
        !x.family.trim() ||
        !Number.isFinite(x.frequency) ||
        x.frequency <= 0,
    ) ||
    new Set(c.cases.map((x) => x.id)).size !== c.cases.length ||
    !c.correctnessChecks.length ||
    c.correctnessChecks.some((x) => !x.trim()) ||
    !c.workloadConfirmation.trim()
  )
    throw new Error(
      "Complete unique workload cases, correctness checks and workload representativeness confirmation.",
    );
  if (
    !env ||
    ![env.operatingSystem, env.architecture, env.runner, env.runtime].every(
      (x) => x.trim(),
    ) ||
    !env.tools.length ||
    env.tools.some((x) => !x.name.trim() || !x.version.trim()) ||
    new Set(env.tools.map((x) => x.name)).size !== env.tools.length ||
    !Object.values(env.hardware).some((x) => x.trim())
  )
    throw new Error(
      "Complete public environment, hardware identity and unique tool names with versions.",
    );
  if (
    !l ||
    !Number.isInteger(l.candidateCount) ||
    l.candidateCount < 1 ||
    !Number.isInteger(l.simultaneousCandidates) ||
    l.simultaneousCandidates < 1 ||
    l.simultaneousCandidates > Math.min(5, l.candidateCount) ||
    !l.startDuration ||
    l.startDuration.seconds <= 0n ||
    l.startDuration.seconds > 31536000n ||
    !l.candidateTimeout ||
    l.candidateTimeout.seconds <= 0n ||
    l.candidateTimeout.seconds > 31536000n
  )
    throw new Error(
      "Set positive candidate limits, concurrency at most five and durations from 1 to 31536000 seconds.",
    );
  if (!p?.metric?.engineKind || !p.metric.name.trim() || !p.metric.unit.trim())
    throw new Error("Select the metric kind and declare its name and unit.");
  if (p.kind !== P.PolicyKind.MEASUREMENTS_ONLY) {
    const u = p.uncertainty;
    const fractions = [
      p.minimumImprovement,
      p.maximumCaseRegression,
      u?.nearThreshold,
      u?.repeatTolerance,
      ...(p.kind === P.PolicyKind.DECLARED
        ? [u?.maximumRelativeRange, u?.decisionMargin]
        : []),
    ];
    if (
      !u ||
      u.engineSchemaVersion !== 1 ||
      fractions.some(
        (x) => x === undefined || !Number.isFinite(x) || x < 0 || x >= 1,
      ) ||
      !Number.isInteger(p.repetitions) ||
      p.repetitions < (p.kind === P.PolicyKind.DECLARED ? 2 : 1)
    )
      throw new Error(
        "Declare valid engine v1 uncertainty fractions from 0 to less than 1 and the required repetitions.",
      );
  }
}

// Imports seed editable settings only. Canonical identities always come from a
// fresh preparation response, including when the file was previously frozen.
export function importConfiguration(json: string): P.ExperimentConfiguration {
  const raw = JSON.parse(json);
  let c: P.ExperimentConfiguration;
  if ("fingerprintSha256" in raw) {
    const spec = fromJsonString(P.ExperimentSpecificationSchema, json);
    c = create(P.ExperimentConfigurationSchema, {
      schemaVersion: spec.schemaVersion,
      baseSha: spec.baseSha,
      initialBaseSha: spec.initialBaseSha,
      harnessSha: spec.harnessSha,
      workflowSha: spec.workflowSha,
      language: spec.language,
      environment: spec.environment,
      inputs: spec.inputs,
      cases: spec.cases,
      correctnessChecks: spec.correctnessChecks,
      policy: spec.policy,
      workloadConfirmation: spec.workloadConfirmation,
      parentCandidateIds: spec.parentCandidateIds,
      limits: {
        candidateCount: spec.limits?.candidateCount,
        simultaneousCandidates: spec.limits?.simultaneousCandidates,
        candidateTimeout: spec.limits?.candidateTimeout,
        startDuration: spec.startDuration,
      },
    });
  } else c = fromJsonString(P.ExperimentConfigurationSchema, json);
  if (
    c.schemaVersion !== 1 ||
    !c.environment ||
    !c.policy?.metric ||
    !c.limits?.startDuration ||
    !c.limits.candidateTimeout
  )
    throw new Error(
      "Import v1 configuration with environment, metric, start duration and candidate timeout, or complete the editable form.",
    );
  const u = c.policy.uncertainty;
  if (
    c.environment.runtimeVersion ||
    u?.confidenceLevel ||
    u?.maximumRelativeIntervalWidth ||
    u?.nearThresholdMargin ||
    (c.policy.kind !== P.PolicyKind.MEASUREMENTS_ONLY &&
      u?.engineSchemaVersion !== 1)
  )
    throw new Error(
      "This file uses legacy settings. Enter the toolchain with its version and engine v1 sample-range policy in the editable form.",
    );
  c.environment.environmentSha256 = "";
  for (const tool of c.environment.tools) tool.configurationSha256 = "";
  c.policy.policySha256 = "";
  return c;
}
