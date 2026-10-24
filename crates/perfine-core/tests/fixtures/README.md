# Perfine adapter fixture projects

The owning [adapter contract](../../../../docs/crates-perfine-adapters-contract.md),
[engine contract](../../../../docs/crates-perfine-core-contract.md), and
[project contract](../../../../docs/project-perfine.md) remain authoritative.
`manifest.json` describes this fixture interface; it is not a Perfine experiment
evidence manifest. These Apache-2.0 sources do not depend on the engine.

Each language counts unordered pairs of equal byte values. The baseline scans
pairs; the candidate builds a histogram. This models duplicate counting over
bounded keys and changes the hot function, not benchmark assertions. Separate
correctness executables or test files check empty, singleton, distinct, repeated,
and boundary-value inputs. Workload checksum checks provide an additional guard.
The API accepts byte arrays of at most 4096 elements; benchmark cases use 1024 and
128 elements. All inputs, iterations, and case declarations are in the manifest.

## Invocation

Run from the repository root with Python 3.9 or newer. Each invocation requires a
new output directory outside this fixture tree. The driver never overwrites an
existing directory. Select `rust`, `c`, `cpp`, `zig`, `go`, or `javascript`.

```sh
python3 "crates/perfine-core/tests/fixtures/run.py" prepare rust --variant candidate --out "/tmp/perfine-rust-source"
python3 "crates/perfine-core/tests/fixtures/run.py" check c --variant baseline --case bulk --out "/tmp/perfine-c-check"
python3 "crates/perfine-core/tests/fixtures/run.py" measure cpp --variant candidate --case protected --route callgrind --out "/tmp/perfine-cpp-profile"
python3 "crates/perfine-core/tests/fixtures/run.py" measure zig --variant baseline --route gungraun --out "/tmp/perfine-zig-profile"
python3 "crates/perfine-core/tests/fixtures/run.py" measure go --variant candidate --out "/tmp/perfine-go-profile"
python3 "crates/perfine-core/tests/fixtures/run.py" measure javascript --variant baseline --out "/tmp/perfine-node-profile"
```

`prepare` copies the common harness and selected implementation into `project/`,
renaming the implementation to `kernel.<extension>`. It performs no compilation.
`check` runs correctness tests and a workload smoke check. `measure` also collects
raw profiles and, for Go/Node, separate timing repetitions. Exit code 0 means the
requested action completed; it never means an optimization passed. Exit code 1
means execution failed, including a missing tool or an unsupported platform.
Argument errors and an existing output directory fail before execution.

`result.json` records the action, host, source hashes, repository revision,
commands, tool versions, profile paths, and errors. Command output is retained in
numbered stdout/stderr files. The revision identifies the source checkout; hashes
identify copied fixture content even before it is committed. The caller owns
output retention and may import these files through engine adapter tests.

The driver selects only the implementation file. A test consumer must create
isolated baseline/head revisions and apply the actual engine lifecycle, immutable
policy, same-SHA validation, comparison, and verdict. Use the `scenarios` array to
select baseline/candidate or the reverse transformation, a protected case, an
empty candidate proposal list, or omitted profile evidence. Regression scenarios
are intended stimuli, not predetermined measurement results. No synthetic
instruction counts, timing results, or profile files are included. Missing-profile
mutation belongs in the consumer's evidence input; it does not delete raw files.

## Toolchains and profiling routes

All compiler/runtime versions and exact commands are recorded per run. Rust uses
edition 2021, `-C opt-level=2`, and `-C debuginfo=2`. C uses C11, C++ uses C++17;
both use `-O2 -g` and separate kernel compilation without LTO. Rust uses
`black_box`, Go uses a benchmark sink, and Zig uses `doNotOptimizeAway` to keep
work observable. Zig is pinned to 0.15.2 with `ReleaseSafe`, Go requires 1.24 or
newer without automatic toolchain downloads, and JavaScript requires Node 24.x.
No `NODE_OPTIONS`, `GOFLAGS`, or compiler flags are inherited from the shell.

- Rust, C, C++, and Zig: `valgrind --tool=callgrind` runs the separately built
  native workload. `callgrind_annotate --inclusive=yes` must attribute
  `count_pairs`. Debug information and optimization follow the
  [official Callgrind manual](https://valgrind.org/docs/manual/cl-manual.html).
  The driver retains raw source/call costs; it does not parse them into engine
  metrics or confuse instruction counts with elapsed time.
- Optional native wrapper: install `gungraun-runner` 0.19.4 with
  `cargo install --locked gungraun-runner --version 0.19.4`, then select
  `--route gungraun`. The driver materializes the harness outside the repository
  and runs `cargo bench --locked --bench pairs`. Its dependency is pinned to 0.19.4.
  The [external executable API](https://docs.rs/gungraun/0.19.4/gungraun/struct.Command.html)
  accepts an absolute native binary path; the
  [binary benchmark guide](https://gungraun.github.io/gungraun/latest/html/benchmarks/binary_benchmarks/quickstart.html)
  documents the harness. The checked-in lock template pins transitive dependencies and is copied to the output.
  A successful harness build alone does not verify any native language route.
- Zig's [0.15.2 language reference](https://ziglang.org/documentation/0.15.2/)
  documents `zig test` and optimized native builds. It does not establish that a
  particular Zig binary executes correctly under Valgrind. That requires a run.
- Go uses `go test -bench ... -benchtime=1s -count=3` for timing and a separate
  `-cpuprofile` run plus `go tool pprof -top` for attribution to `CountPairs`.
  These follow [Go's testing flags](https://pkg.go.dev/cmd/go#hdr-Testing_flags)
  and [performance guide](https://go.dev/wiki/Performance). Profiles are not
  substituted for benchmark timing results.
- Node uses warm-up followed by `process.hrtime.bigint()` timing, three fresh
  process repetitions, and a separate
  [`--cpu-prof` run](https://nodejs.org/api/cli.html#--cpu-prof). A profile must
  contain sampled `countPairs` frames. Normal JIT remains enabled. CPU profiles
  include warm-up; recorded timing excludes it. Exploration with `--jitless` is
  not part of this fixture interface.

`measure` requires Linux x86_64. Go and Node alone permit
`--allow-unsupported-host` for exploratory measurements with the host recorded.
Native profiling has no override. Missing Valgrind, Zig, Gungraun, debug symbols,
or hot-path attribution fails explicitly. Documentation-supported commands do
not establish native acceptance. Each language and route must execute on the
declared platform before support is claimed. Emulated Docker runs also need their
image digest and emulation context recorded by the caller.

The fixture driver is not a sandbox. It uses a reduced subprocess environment and
per-invocation build directories, but the engine owns credential boundaries,
resource isolation, and experiment limits. Harness dependency resolution may need
network access; the six workload projects otherwise use standard libraries only.
