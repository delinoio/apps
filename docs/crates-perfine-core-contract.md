# Perfine shared engine

## Scope

`crates/perfine-core` owns the shared experiment engine, policy evaluation, provenance, and local evidence. Both local execution and hosted evaluation consume this Apache-2.0 engine without copying it into the server workspace.

## Runtime and Language

Rust is an explicit project choice instead of the non-server Go default. The CLI and API share the engine. Linux x86_64 is the first officially verified execution platform. Add an explicit root Cargo workspace member when Rust implementation begins; a directory skeleton is not a Rust package.

## Users and Operators

Local developers, AI adapters, CLI/MCP callers, and the hosted evaluator.

## Interfaces and Contracts

### Experiment lifecycle and provenance

- Require an explicit base commit SHA. Preserve uncommitted user changes; do not include or modify them automatically. Ask the user to commit intended input changes before experimenting.
- Give each independent candidate its own task-owned working directory, branch, and build directory from the same pinned base. Do not share writable build outputs or combine candidates silently.
- Fix base and head SHAs, harness/workflow revision, input hashes, cases, toolchain and tool versions, build options, runtime flags, runner/environment, metrics, weights, protected cases, and policy before evaluation. Unknown case frequencies use 1 with explicit disclosure under the original preset.
- First validate the measurement infrastructure with a same-SHA comparison. For timing metrics, use it to assess variability rather than requiring identical times. Repeat baseline validation when the base or measurement infrastructure changes.
- AI may propose missing workloads, deterministic inputs, and result checks. Require user confirmation of workload representativeness before fixing the workload and measuring the baseline. Do not change workloads or weights after seeing candidate results to obtain a pass.
- Inspect profiles before proposing hypotheses. Run only evidence-supported candidates; five is a concurrency ceiling, not a quota. Keep focused changes compatible with semantics, error behavior, ownership, and public interfaces. Restrict performance claims to measured environments.
- Preserve fork-specific preparation as a GitHub execution path, not a requirement for local-only operation. When using the original fork workflow, verify upstream/fork identities and current base, keep infrastructure out of upstream production diffs, validate actual dispatched workflows and artifacts, and do not force-push or bypass protections.
- A dependent or combined candidate is a separate experiment. Compare it both incrementally against the latest accepted revision and cumulatively against the initial base, unless those bases are identical.
- Verify exact run, workflow, source, case, input, tool, and artifact identities. Missing/renamed cases, changed inputs, incomplete artifacts, or incompatible environments cannot establish a pass.
- The local engine and hosted comparison use the same evidence and evaluation rules. A green CI job alone is not a performance pass.

### Performance policies

Provide the original Callgrind `Ir` policy as an editable preset. Users may change the policy before the experiment; once started it is immutable.

- Score: `sum(frequency * head) / sum(frequency * base)`; improvement: `1 - score`. Use complete positive counts. Also report individual case changes, weighted absolute changes, and geometric mean as a supplementary statistic.
- Default: at least 1% portfolio improvement, less than 1% regression in every target/related case, and passing correctness checks. Apply to incremental and cumulative comparisons.
- Preserve the below-1% exception only when both portfolios improve, code is equally simple or simpler, and each individual regression is below 1%.
- Preserve the single unprotected case exception for a regression of at least 1% but less than 3% only when both portfolios improve by at least 5%, the regression does not recur across the workload family, and profiles explain it. Disclose the exception; it is not a default-policy pass.
- A protected-case regression of at least 1%, or a regression of at least 3% in any case, requires explicit human judgment before publication. Reject incremental portfolio deterioration or widespread regressions.
- Near a threshold or when profile and aggregate evidence conflict, repeat once with identical conditions. Inconsistent results remain inconclusive. This measurement repeat is distinct from a transient infrastructure retry.

For time, throughput, and memory, require the user to declare objective, minimum improvement, allowed regressions, repetitions, and uncertainty criteria before automatic evaluation. Without that policy, report measurements without a pass claim. Keep policy-based exceptions and human judgments in the evidence.

### Limits, cancellation, retry, and resume

- Before optimization starts, require candidate-count limit, simultaneous-candidate limit, overall start deadline/duration, and maximum duration per candidate. Simultaneous candidates cannot exceed five.
- Overall duration is the deadline for starting new candidates. On graceful stop or that deadline, stop creating candidates and allow active candidates to finish within their individual time limits. Provide an explicit immediate-cancel action.
- Preserve completed evidence and candidate branches on cancellation. Exclude incomplete work from performance conclusions; do not discard user work.
- Use request IDs to deduplicate execution and publication. Reuse completed steps on resume only if source, inputs, tools/environment, and policy match; otherwise create a new experiment.
- Request deduplication records expire 30 days after first creation under the common contract. After that period the same request ID may be treated as new. State this limit explicitly. Deleted experiment IDs remain rejected, and PR publication checks retained candidate/approval/publication records and GitHub state independently.
- Retry temporary network/service failures automatically at most once within execution/resource limits. Do not automatically retry authentication, authorization, correctness, or performance failures. Reconcile uncertain publication outcomes with GitHub before retrying.
- Durable deletion is an explicit exception: retain failed deletion work until completion, retry cleanup, and expose operational alerts.

## Storage

Local evidence, profiles, and resume records live in Git-excluded `.perfine/` and remain until deletion. Preserve completed evidence and candidate branches on cancellation. [Protocol contract](protos-perfine-v1-contract.md) defines evidence manifests; [Api contract](servers-perfine-api-contract.md) defines hosted retention and deletion.

## Security

Preserve uncommitted user work. Isolate each candidate workspace, branch, build output, and credentials. Candidate build/test/measurement must not inherit AI or publication credentials. [Adapters contract](crates-perfine-adapters-contract.md) defines execution trust boundaries. Human approval and publication follow [Cli contract](cmds-perfine-cli-contract.md).

## Logging

Use structured logs with request and experiment IDs. Do not log secrets, source snippets, raw profiles, or raw test evidence. Use Rust `tracing` where applicable. CLI/operator output supports ANSI color by default and a documented opt-out.

## Build and Test

After Rust implementation, run root `cargo test` and the required Cargo formatting checks. Cover policy arithmetic, positive-count completeness, every exception, same-SHA validation, immutable fingerprints, independent and dependent candidates, deadlines, cancellation, retries, and resume. Exercise the same evaluation fixtures through local and hosted consumers.

Automated verification is the first-release gate. Follow the acceptance criteria and test scenarios in the [project index](project-perfine.md). Record source revision, commands, results, and unperformed checks in PR/CI evidence. Fixtures, mocked integrations, builds, and packaging do not establish live or native acceptance.

## Dependencies and Integrations

[Adapters contract](crates-perfine-adapters-contract.md), [Cli contract](cmds-perfine-cli-contract.md), [Protocol contract](protos-perfine-v1-contract.md), and [Api contract](servers-perfine-api-contract.md). The server consumes the Apache engine as a local path dependency while remaining a separate FSL workspace.

## Change Triggers

Update the project index and affected owning contracts together when ownership, interfaces, evidence, or policy changes. Update scoped instructions only for ownership or development-procedure changes; follow the [instruction-update policy](README.md#instruction-update-policy).

## References

- [Perfine project](project-perfine.md)
- [Repository defaults](repository-defaults.md)
- [Repository workflow](repository-workflow-contract.md)
- [Repository license](repository-license-contract.md)
