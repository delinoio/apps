# Install and configure Perfine

## Installation and compatibility

The CLI host policy supports these targets with current OS patches:

| Host    | CPU    | Minimum                                      |
| ------- | ------ | -------------------------------------------- |
| Linux   | x86_64 | glibc 2.39 or later                          |
| macOS   | x86_64 | macOS 15 or later                            |
| macOS   | ARM64  | macOS 15 or later                            |
| Windows | x86_64 | Windows 11 25H2, build 26200 family or later |

The npm launcher requires Node.js 24 or later. Standalone executables need neither npm nor Node.js. Other hosts are outside this support policy. Minimum-version policy does not establish completed validation; consult the exact release's records.

When version 0.1.0 is published, install that exact version:

```sh
npm install --global perfine@0.1.0
perfine --version
perfine --help
```

If that version is absent from npm, do not substitute an unrelated package or assume a release is available. Use a verified Perfine release when one is available. The `perfine` entry package declares these optional dependencies at the exact same release version:

- `@delinoio/perfine-linux-x64`
- `@delinoio/perfine-darwin-x64`
- `@delinoio/perfine-darwin-arm64`
- `@delinoio/perfine-win32-x64`

npm selects the package for your actual OS and CPU. Keep optional dependencies enabled. Perfine verifies the selected executable's SHA-256 before each launch. A missing, mismatched or corrupt platform package stops execution with reinstall guidance; there is no runtime download or source-build fallback. Offline installation requires both the entry archive and the matching platform archive locally.

Standalone archives for all four targets are distributed through [GitHub Releases](https://github.com/delinoio/apps/releases) when published. Select your OS and CPU, obtain the release `SHA256SUMS`, and verify the archive before extraction. Then verify the extracted executable against its included binary checksum. On Linux use `sha256sum`, on macOS use `shasum -a 256`, or on Windows use PowerShell `Get-FileHash -Algorithm SHA256`; compare the resulting digest with the corresponding checksum entry. Keep the included provenance, licenses, and notices. Extract to a location where your user can execute the binary and add that executable directory to your `PATH`, or invoke it by its quoted full path (`perfine.exe` on Windows).

macOS archives are not distribution-signed or notarized, and Windows archives lack Authenticode. Gatekeeper, SmartScreen or organization policies can restrict installation or startup. Use your administrator's approved process; do not disable system protections globally. A checksum checks integrity, and runtime-required signing mechanics do not establish publisher identity.

## Docker and host prerequisites

Measurement and optimization always use isolated Linux x86_64 (`linux/amd64`) Docker containers. Install and start Docker explicitly. Windows requires Docker Desktop with the WSL 2 Linux-container backend. ARM Macs use x86_64 emulation, which results identify; this is neither native ARM measurement nor native x86_64 acceptance. Native macOS and Windows measurement are outside scope.

Provision the configured trusted images and workload tools before execution. Perfine does not install missing dependencies. Use host-appropriate absolute executable paths, quote paths with spaces or non-ASCII characters using your shell's syntax, and ensure Docker can access the repository and execution mounts. An unavailable daemon, unsuitable architecture or inaccessible mount requires correction before measurement.

Package startup, minimum-OS execution, full Docker workflows and live-provider authentication have separate validation records. A successful build or a newer runner does not verify a minimum OS. In particular, Windows Server checks do not verify Windows 11 25H2. Unperformed Docker and live-provider checks remain unverified even when a release passes its required package checks.

## Prepare your project

1. Commit the source and workload changes that you want to measure. Perfine preserves uncommitted work and excludes it from candidates.
2. Provide representative workloads and independent correctness checks. Confirm that each workload exercises the code you intend to optimize.
3. Provision Docker and a trusted image containing the compiler/runtime, profiler, and all workload dependencies. Pin the image by digest. Candidate execution has no network access for installing dependencies.
4. Run `perfine init` from the project directory. Review the generated `perfine.yaml` and keep `.perfine/` excluded from Git.
5. Complete the configuration before running `perfine doctor`.

`init` produces an incomplete template, not a runnable benchmark. Supply real full commit SHAs, hashes of committed workload inputs, commands, cases, paths, and resource limits. For an existing file, Perfine shows the proposed content and requires `--overwrite-sha256` with the existing content's SHA-256 before overwriting it. Review changes before confirming.

## Configuration fields

| Field                   | Required input                                                                                                                                                           |
| ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `schema_version`        | `1`.                                                                                                                                                                     |
| `experiment`            | Version 1, repository identity, `initial_base`, `incremental_base`, and a written `workload_confirmation`.                                                               |
| `experiment.conditions` | Harness/workflow SHAs, workload input hashes, metric/unit, cases, correctness check names, and execution conditions.                                                     |
| `experiment.policy`     | The Ir preset, a declared non-Ir policy, or measurements only. See [policies](/services/perfine/policies).                                                               |
| `experiment.limits`     | Candidate count, simultaneous candidates, start duration, and per-candidate duration. All are required.                                                                  |
| `adapter`               | Version 1 language recipe, discovery files, build/correctness/workload commands, profiles, source mappings, and tool probes. See [adapters](/services/perfine/adapters). |
| `docker`                | Absolute Docker executable, immutable image, dependency names, numeric `uid:gid`, memory bytes, CPUs, process limit, and command timeout seconds.                        |
| `execution_root`        | An absolute task directory outside your repository and `.perfine/`.                                                                                                      |
| `toolchain_probe`       | A key in the adapter's tool probes that identifies the selected toolchain.                                                                                               |
| `agent`                 | Optional for measurement; required for AI optimization. Local-only agent configuration.                                                                                  |
| `action_commit`         | Optional verified distribution SHA for GitHub workflow generation.                                                                                                       |

Tool and environment probes replace template observation fields before Perfine freezes the experiment. Workload hashes must match committed files. Do not treat optimized production source as an immutable workload input.

For a local agent, configure `provider` as `codex` or `claude`, its dedicated `docker` image/resources, `executable`, `auth_file`, `network`, optional `model`, `maximum_source_bytes`, and `maximum_edits`. Use the AI CLI's own supported local file authentication. A keychain-only login is not automatically exported to a container. Keep this local configuration private and never submit it to hosting. The agent image must contain the selected CLI, and its network needs controlled outbound access.

Candidate code receives neither AI credentials nor GitHub publication credentials. Agent execution uses a separate environment and bounded profile/source input. Unsupported AI CLI flags or authentication methods fail rather than silently weakening that separation.
