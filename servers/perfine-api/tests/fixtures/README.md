# Hosted evidence fixtures

These files are server-owned test inputs; server builds do not read client source directories.

`client-v1-config.json` is a frozen v1 configuration exported by the local client engine. Its recorded fingerprint pins JSON field ordering, map ordering and policy serialization for the independent server evaluator. It contains synthetic repository, tool and workload data, not account data.

The profiler captures exercise artifact privacy and eligibility checks:

- `go-darwin-arm64.pprof`: Go 1.27.3 on macOS arm64, captured with `go test -run '^Test' -bench '^BenchmarkPairs/bulk$' -benchtime=500ms -count=1 -cpuprofile=captured.pprof -o test.bin`.
- `node-darwin-arm64.cpuprofile`: Node 24.20.0 on macOS arm64, captured with `node --cpu-prof --cpu-prof-name=captured.cpuprofile benchmark.mjs 1024`.
- `c-linux-amd64-emulated.callgrind`: C workload compiled with `cc -O2 -g main.c kernel.c -o /build/workload`, then profiled with `valgrind --tool=callgrind --callgrind-out-file=/build/callgrind.out /build/workload 1024` under Linux amd64 emulation on an arm64 host.

These inputs establish parser and privacy behavior only, not native performance or live-provider acceptance. `github-test-key.pem` is a test signing key for mocked GitHub responses.
