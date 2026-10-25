# Install and configure Perfine

## Installation and compatibility

The first distribution targets Linux x86_64 with glibc 2.39 or later. The npm launcher requires Node.js 24 or later. Other operating systems and architectures are not supported execution targets in this release.

When version 0.1.0 is published, install that exact version:

```sh
npm install --global perfine@0.1.0
perfine --version
perfine --help
```

If that version is absent from npm, do not substitute an unrelated package or assume a release is available. Use a verified Perfine release when one is available. The npm package includes the executable and checks its SHA-256 before each launch; installation does not fetch a second executable.

Standalone Linux x86_64 archives are distributed through [GitHub Releases](https://github.com/delinoio/apps/releases). Obtain the matching checksum file and verify the archive before extraction. Inside the extracted archive's `runtime` directory, run `sha256sum -c SHA256SUMS` before executing the binary. Keep the included provenance, licenses, and notices. Standalone execution does not require the npm launcher.

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
