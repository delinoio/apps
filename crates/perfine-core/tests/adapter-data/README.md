# Captured parser inputs

These files are parser regression inputs, not comparative performance evidence.
They contain only the repository's byte-pair fixture workloads and runtime frames.
The fixture sources are unchanged. Source input is the committed six-language
fixture set; the capture commands materialize copies outside that directory.

- `go-darwin-arm64.pprof`: Go 1.27.3, macOS arm64, `go test -run '^Test' -bench '^BenchmarkPairs/bulk$' -benchtime=500ms -count=1 -cpuprofile=captured.pprof -o test.bin` with `baseline.go` copied as `kernel.go`.
- `node-darwin-arm64.cpuprofile`: Node 24.20.0, macOS arm64, normal JIT, `node --cpu-prof --cpu-prof-name=captured.cpuprofile benchmark.mjs 1024` with `baseline.mjs` copied as `kernel.mjs`.
- `c-linux-amd64-emulated.callgrind`: `cc -O2 -g main.c kernel.c -o /build/workload`, then `valgrind --tool=callgrind --callgrind-out-file=/build/callgrind.out /build/workload 1024`. Linux amd64 Docker image on an arm64 host with emulation. The integration test executes an independent correctness binary before this workload. This is not native Linux acceptance.

The tests check original units, sample costs, call relationships, and source
locations. No comparison, improvement or provider-authentication claim follows
from these captures. Live providers and all other native language routes require
separate execution evidence. The Docker integration tests are opt-in and require
an operator-provisioned digest-pinned image; they never download dependencies.
