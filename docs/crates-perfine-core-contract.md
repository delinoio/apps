# Perfine shared engine

## Scope

`crates/perfine-core` owns the shared experiment engine, policy evaluation, provenance, and local evidence. Local CLI and MCP execution consume this Apache-2.0 engine. Hosted evaluation is independently owned by the server and preserves the same versioned evidence and policy semantics.

## Runtime and Language

Rust is an explicit project choice instead of the non-server Go default. The CLI consumes this engine; the API owns a compatible server evaluator. Linux x86_64 is the first officially verified execution platform. Add an explicit root Cargo workspace member when Rust implementation begins; a directory skeleton is not a Rust package.

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

### Rust v1 consumer boundary

- `types` owns validated UUID v7 resource IDs, full commit SHAs, SHA-256 fingerprints, schema major version, metric/unit identities, and the separate execution-state and verdict enums. Unknown additive object fields remain compatible; unsupported major versions and missing required fields are rejected.
- `evidence::Config::freeze` validates and seals repository/source revisions, conditions, policy, workload confirmation, and limits. Canonical fingerprints use the declared struct field order and sorted map keys. A deserialized frozen config must pass `validate` before use; engine and evaluator entry points perform this check.
- `evidence::verify` compares a manifest with a separately scheduled `RunIdentity` and reads every required artifact through `ArtifactReader`. The expected identity includes optional external workflow-run identity. Only verified runs can enter baseline validation and evaluation. The adapter remains responsible for faithfully parsing original measurements, profile coverage, and correctness results from artifact bytes; checksums establish byte identity, not the truth of a parser's assertions.
- `policy::evaluate` consumes an incremental round, a cumulative round when the bases differ, and at most one repeat round. Each comparison needs a validated same-SHA baseline. Repeat runs use fresh run IDs, the same sources and configuration, and the same candidate. The report retains evidence fingerprints, unknown-frequency disclosures, case changes, supplemental geometric mean, exceptions, and judgment/repeat requirements.
- The original Ir preset uses a 0.001 absolute-ratio near-threshold window and repeat tolerance. These settings are editable before freezing. Boundary comparisons allow a numerical rounding tolerance of 1e-12. Ir observations must be positive exact integers representable in f64, at most 2^53; varying Ir samples remain inconclusive. Arithmetic overflow is an error. Widespread regressions mean two or more cases at or above 1% in either comparison; the single-case exception also rejects observed regressions elsewhere in that case's workload family.
- A declared non-Ir policy supplies direction, thresholds, repetitions (at least two), maximum relative sample range, decision margin, and repeat tolerance. The range criterion includes both same-SHA runs. The decision margin reduces improvement and increases regression conservatively. A missing policy produces measurements with `not_evaluated`; unresolved profile conflicts or inconsistent repeats produce `inconclusive`.
- `lifecycle::Engine` provides transactional creation, candidate admission/completion/failure/retry, stop, deadline ticks, resume, completed experiment-step records, deletion, and cleanup results. Callers supply Unix seconds and must drive deadline ticks and process cancellation through the environment adapter. Deadlines never reset on resume or retry. Graceful stop with no candidate is a normal completed execution. Candidate records retain their own immutable measured head and evaluation.
- All mutations deduplicate under the store transaction. A receipt with `replayed: true` must not dispatch an external action again. The first creation time fixes the 30-day horizon. Permanent tombstones override deduplication and block access/recreation/resume; failed cleanup retains workspace cleanup targets and failure counts until completion.
- `publication` binds approval to experiment, candidate, evaluated evidence, target repository/base, exact source SHAs, production-diff hash, title, and body. Callers must establish current publish authority and verify the reviewed production diff. Persist dispatch intent before GitHub access. An interrupted or uncertain dispatch requires GitHub reconciliation; confirmed absence permits at most one retry. Retained approval/publication records prevent duplicate dispatch after request expiry. Publication is draft-only; this crate does not call GitHub or merge.
- `interfaces` separates language discovery, measurement, profile interpretation, agents, and execution environments. Command arguments are vectors. Environment adapters reserve canonical isolated paths, preserve dirty user input, enforce process deadlines and credential separation, and retain branches/evidence on cancellation. Capability metadata does not establish verified platform or tool support.

## Storage

`storage::FileStore` uses an OS file lock, checksum-verified versioned snapshots, atomic replacement, and file/directory synchronization. `storage::EvidenceDirectory` stores immutable content-addressed artifacts and revalidates them when loading a run. These local directories must be caller-owned and Git-excluded; hosted consumers supply an authorized transactional `Store` and artifact reader. A store transaction must commit all state and deduplication changes atomically or roll them back. Hosts must check experiment access/tombstones before exposing artifacts.

Local evidence, profiles, and resume records live in Git-excluded `.perfine/` and remain until deletion. Preserve completed evidence and candidate branches on cancellation. [Protocol contract](protos-perfine-v1-contract.md) defines evidence manifests; [Api contract](servers-perfine-api-contract.md) defines hosted retention and deletion.

## Security

Preserve uncommitted user work. Isolate each candidate workspace, branch, build output, and credentials. Candidate build/test/measurement must not inherit AI or publication credentials. [Adapters contract](crates-perfine-adapters-contract.md) defines execution trust boundaries. Human approval and publication follow [Cli contract](cmds-perfine-cli-contract.md).

## Logging

Use structured logs with request and experiment IDs. Do not log secrets, source snippets, raw profiles, or raw test evidence. Use Rust `tracing` where applicable. CLI/operator output supports ANSI color by default and a documented opt-out.

## Build and Test

After Rust implementation, run root `cargo test` and the required Cargo formatting checks. Cover policy arithmetic, positive-count completeness, every exception, same-SHA validation, immutable fingerprints, independent and dependent candidates, deadlines, cancellation, retries, and resume. Exercise the same evaluation fixtures through local and hosted consumers.

Automated verification is the first-release gate. Follow the acceptance criteria and test scenarios in the [project index](project-perfine.md). Record source revision, commands, results, and unperformed checks in PR/CI evidence. Fixtures, mocked integrations, builds, and packaging do not establish live or native acceptance.

## Dependencies and Integrations

[Adapters contract](crates-perfine-adapters-contract.md), [Cli contract](cmds-perfine-cli-contract.md), [Protocol contract](protos-perfine-v1-contract.md), and [Api contract](servers-perfine-api-contract.md). The server has no path dependency on this client crate. Its evaluator and compatibility tests follow the API contract.

## Change Triggers

Update the project index and affected owning contracts together when ownership, interfaces, evidence, or policy changes. Update scoped instructions only for ownership or development-procedure changes; follow the [instruction-update policy](README.md#instruction-update-policy).

## References

- [Perfine project](project-perfine.md)
- [Repository defaults](repository-defaults.md)
- [Repository workflow](repository-workflow-contract.md)
- [Repository license](repository-license-contract.md)

### Post-run review

A completed candidate may be reevaluated through the same engine finish operation with explicit retained review evidence. Its evaluation evidence fingerprints must match the previous evaluation exactly. Review cannot change measurements, immutable policy or source identities, and cannot proceed after a publication approval exists. Retain the caller's content-bound confirmation and attestation in the owning execution journal. Missing required repeat evidence cannot produce a pass.
