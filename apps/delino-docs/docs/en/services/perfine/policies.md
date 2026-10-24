# Policies and evidence

## Freeze the experiment before evaluating

Fix source/base SHAs, harness and workflow revision, workload input hashes, cases, frequencies, protected cases, tool versions, build/runtime options, environment, metric/unit, limits, and policy before evaluation. Confirm workload representativeness first. Do not change cases, weights, or policy after seeing a result to obtain a pass.

Validate the baseline by comparing the same SHA twice. Timing measurements assess variability; they need not be identical. Repeat baseline validation when the base or measurement infrastructure changes. A dependent or combined candidate is a new experiment: compare it incrementally against the latest accepted revision and cumulatively against the initial base.

## Callgrind Ir preset

`Ir` means instruction count, not elapsed time. The portfolio score is `sum(frequency * head) / sum(frequency * base)` and improvement is `1 - score`. Unknown frequencies use 1 with an explicit disclosure. Counts must be complete positive exact integers. Individual case changes and weighted absolute changes remain visible; the geometric mean is supplemental.

This fragment belongs under `experiment.policy`:

```yaml
policy:
  kind: ir
  settings:
    minimum_improvement: 0.01
    maximum_case_regression: 0.01
    near_threshold: 0.001
    repeat_tolerance: 0.001
    allow_simple_exception: true
    allow_single_case_exception: true
```

The default requires at least 1% portfolio improvement, less than 1% regression in every target or related case, and passing correctness checks. Both incremental and cumulative comparisons must qualify.

The preset retains these explicitly disclosed exceptions and review requirements:

- Below 1% improvement can qualify only when both portfolios improve, the code is equally simple or simpler, and every individual regression remains below 1%.
- One unprotected case may regress by at least 1% but less than 3% only when both portfolios improve by at least 5%, the regression does not recur across its workload family, and profiles explain it. This is an exception, not a default-policy pass.
- A protected-case regression of at least 1%, or any case regression of at least 3%, requires explicit human judgment before publication.
- Incremental portfolio deterioration or widespread regressions are rejected. Two or more cases regressing by at least 1% constitute widespread regressions.

Near a threshold, or when profile and aggregate evidence conflict, repeat once under identical conditions. Inconsistent evidence remains inconclusive. Review explanations are required for an exception; enabling an exception in the preset does not supply those explanations. The current CLI has no documented command to enter all exception-review evidence. Do not treat an unavailable review path as an approved exception.

## Time, throughput, memory, and other metrics

Declare the objective, thresholds, repetition count, and uncertainty criteria before automatic evaluation. This illustrative time-policy fragment is not a recommended threshold for every workload:

```yaml
policy:
  kind: declared
  settings:
    objective: minimize
    minimum_improvement: 0.05
    maximum_case_regression: 0.01
    repetitions: 5
    uncertainty:
      maximum_relative_range: 0.02
      decision_margin: 0.01
      repeat_tolerance: 0.01
    near_threshold: 0.001
```

Use `maximize` for a metric where larger is better. Declared policies require at least two repetitions. Relative range is `(max - min) / mean`, including baseline variability. The decision margin conservatively reduces improvement and increases regression. These criteria do not calculate confidence intervals.

Use `policy: { kind: measurements_only }` to report measurements without an automatic pass. Match the selected metric and units to the actual adapter output.

## What evidence establishes

A result must retain source/harness identities, input hashes, tools/environment, policy, original units, artifact hashes, correctness results, profile coverage, and completion status. A checksum establishes byte identity; it does not prove that a parser or benchmark measures the intended behavior.

Missing or renamed cases, changed inputs, incompatible environments, incomplete uploads, or missing correctness/profile evidence cannot establish a pass. Keep original evidence for independent inspection. Local results and evidence remain in `.perfine/` until deletion.

A green CI job means execution succeeded, not necessarily that performance improved. Fixture results and mocked integrations test software behavior; they are not measurements of your application. Restrict claims to the measured environment and workload. A lower instruction count does not prove lower latency, and an inconclusive or no-candidate result is an honest outcome.
