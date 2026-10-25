# Language adapters and extensions

## Configured language routes

| Language (`adapter.language`) | Measurement and profile input                                                               |
| ----------------------------- | ------------------------------------------------------------------------------------------- |
| Rust (`rust`)                 | Callgrind instruction counts and profiles, including explicitly configured Gungraun output. |
| C (`c`)                       | Callgrind instruction counts and profiles.                                                  |
| C++ (`cpp`)                   | Callgrind instruction counts and profiles.                                                  |
| Zig (`zig`)                   | Callgrind instruction counts and profiles.                                                  |
| Go (`go`)                     | An exact Go benchmark row and unit, plus gzip pprof.                                        |
| JavaScript (`java_script`)    | A declared numeric field from benchmark JSON, plus a Node CPU profile.                      |

These routes provide configured execution and profile interpretation. They do not imply that every profiler version, workload, or optimization has been verified. Record the actual tool versions and test your declared environment. Measurement runs in Linux x86_64 Docker containers on all supported CLI hosts. Windows uses Docker Desktop with its WSL 2 Linux-container backend; ARM Macs use explicitly identified x86_64 emulation. Native macOS/Windows and Linux ARM64 measurement are not provided.

## Recipe contract

An adapter recipe has `schema_version: 1`, `language`, `discovery_files`, `build`, `correctness`, `workloads`, `tool_probes`, and `source_prefixes`. Commands use an `executable` and an `arguments` array; they are not implicitly interpreted as shell strings.

Each workload declares `command`, `profiles`, `format`, `output`, `repetitions`, and `required_paths`. Profile paths are relative to `/build`; required source paths are relative to your repository. Every repetition must produce fresh, nonempty evidence. Build and workload outputs belong in `/build` or temporary storage; the source checkout at `/work` is read-only during measurement.

- Callgrind uses `format: callgrind` and `output: { kind: callgrind }`. Configure the actual Gungraun/Callgrind file path; Perfine does not guess it.
- Go uses `format: pprof` and `output.kind: go_benchmark`, with the exact `benchmark` name including its CPU suffix and original `unit`. Write the profiled test binary under `/build`.
- JavaScript uses `format: node_cpu` and `output.kind: node_benchmark`, with `value_field` and `unit`. Declare warmup and normal JIT conditions in the harness. A CPU profile's duration is not a substitute for benchmark output. `--jitless` and `--no-opt` experiments do not establish normal-JIT improvement.

Declare independent project correctness commands, not just assertions inside the workload. Required source paths must have observed positive profile costs. Configure `source_prefixes` when profiler paths differ from repository paths. Missing symbols, missing coverage, ambiguous instruction totals, or incomplete profiles prevent valid evidence.

## Extension contract

An integration must keep discovery/build/test, measurement, profile interpretation, AI integration, and execution-environment capabilities separate. Declare tools and versions, units, runtime conditions, evidence inputs/outputs, source mapping support, dependency/network needs, and capability level: execution, profile interpretation, or verified optimization.

Preserve original metrics, source mappings, costs, and call relationships when available. Do not combine incompatible units or label instruction counts as elapsed time. Integrate with the shared v1 lifecycle, frozen policy, cancellation, credential isolation, and evidence verification. A new language must reuse the experiment engine.

The bundled recipe accepts the six language routes above. There is no documented dynamic plugin-install command or arbitrary profiler discovery. Implementing an extension requires an adapter integration; selecting an `extension` language in the bundled recipe is not sufficient. A capability declaration without verification evidence is not verified optimization support.
