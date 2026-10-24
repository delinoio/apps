# Run and inspect experiments

Use a new UUID v7 for each experiment. In these examples, set `PERFINE_EXPERIMENT_ID` to that ID and `PERFINE_CANDIDATE_ID` to a candidate ID returned by Perfine. They are not commit SHAs. Complete [setup](/services/perfine/setup) first.

```sh
perfine doctor
perfine measure --experiment "$PERFINE_EXPERIMENT_ID"
perfine status --experiment "$PERFINE_EXPERIMENT_ID"
perfine report --experiment "$PERFINE_EXPERIMENT_ID"
```

`measure` runs two measurements of the same source SHA to validate the baseline. To start an optimization experiment, choose a new experiment ID and run:

```sh
perfine optimize --experiment "$PERFINE_EXPERIMENT_ID"
perfine compare --experiment "$PERFINE_EXPERIMENT_ID" --candidate "$PERFINE_CANDIDATE_ID"
perfine report --experiment "$PERFINE_EXPERIMENT_ID"
```

`optimize` validates the baseline, interprets profiles, obtains supported hypotheses and edits, checks correctness, measures candidates, and evaluates policy. It does not publish a pull request. Independent candidates have separate workspaces, branches, and build outputs. The current CLI executes them serially; the simultaneous-candidate limit is an upper bound.

## Limits and stopping

This example is only the `experiment.limits` fragment, with illustrative limits. Choose values for your workload before starting:

```yaml
limits:
  candidate_count: 3
  simultaneous_candidates: 1
  start_duration_seconds: 1800
  candidate_duration_seconds: 600
```

Simultaneous candidates cannot exceed five or the candidate count. The overall duration is the deadline for starting new candidates. It does not extend an active candidate's own timeout. Five is a ceiling, not a target number of candidates.

```sh
perfine cancel --experiment "$PERFINE_EXPERIMENT_ID"
perfine cancel --experiment "$PERFINE_EXPERIMENT_ID" --immediate
perfine resume --experiment "$PERFINE_EXPERIMENT_ID"
```

Graceful cancellation stops new candidates and lets active work finish within its timeout. Immediate cancellation also signals active processes. Completed evidence and candidate branches remain available. Incomplete work cannot support a performance conclusion.

Resume verifies source, workload inputs, tools, environment, policy, and artifact identities. It reuses verified completed measurements, does not repeat an attempted hypothesis, and does not reset deadlines. If conditions changed, create a new experiment instead of editing frozen evidence.

## Output and automation

Global options are `--repository`, `--config`, `--request-id`, `--json`, and `--no-color`. `NO_COLOR` also disables color. Use `--json` for versioned output; request IDs appear in responses. Diagnostic events go to stderr.

Execution states are `queued`, `running`, `completed`, `failed`, and `cancelled`. Verdicts are separately `pass`, `regression`, `inconclusive`, or `not_evaluated`. A completed run with no candidate is valid. Always inspect correctness, completeness, exceptions, and limitations before interpreting a verdict.

Mutations use request IDs for deduplication. Reuse the same ID and unchanged request when recovering an interrupted request. Deduplication expires 30 days after first creation; retries do not extend it. Deleted experiment IDs remain rejected after that period. Publication has additional retained approval and reconciliation checks.

Temporary infrastructure failures can be retried at most once within the original limits. Authentication, authorization, correctness, and performance failures are not automatic retry cases. A policy measurement repeat is separate from an infrastructure retry.

`perfine mcp serve` exposes the same command capabilities through newline-delimited JSON-RPC on stdio. MCP uses v1 inputs and the same policy and approval checks; it does not bypass authorization. Upload and publication are separate [hosted](/services/perfine/hosting) and [approval](/services/perfine/publication) operations.
