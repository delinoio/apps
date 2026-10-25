# Perfine

Perfine measures workloads and evaluates profile-backed performance candidates. The CLI host policy covers these targets with current OS patches:

| Host    | CPU             | Minimum                             |
| ------- | --------------- | ----------------------------------- |
| Linux   | x86_64          | glibc 2.39                          |
| macOS   | x86_64 or ARM64 | macOS 15                            |
| Windows | x86_64          | Windows 11 25H2, build 26200 family |

The npm launcher requires Node.js 24 or later. When the exact version is published, install it with `npm install --global perfine@0.1.0`, then run `perfine --help`. Confirm release availability before installation; these instructions do not establish registry publication or completed platform acceptance.

`perfine` uses exact, matching optional dependency versions: `@delinoio/perfine-linux-x64`, `@delinoio/perfine-darwin-x64`, `@delinoio/perfine-darwin-arm64`, and `@delinoio/perfine-win32-x64`. npm selects the package for your OS and CPU. Keep optional dependencies enabled. The launcher verifies the selected executable's SHA-256 before each launch. It does not download a replacement binary or compile one from source. If the platform package is missing, mismatched or corrupt, reinstall the exact version with its matching optional dependency from a trusted release. Do not bypass verification. Offline installation requires both archives locally.

Standalone archives for each target are available through verified [GitHub Releases](https://github.com/delinoio/apps/releases), when published. They need neither npm nor Node.js. Check the archive against the release `SHA256SUMS`, then check the extracted executable against its included checksum before execution. On Linux use `sha256sum`; on macOS use `shasum -a 256`; on Windows use PowerShell `Get-FileHash -Algorithm SHA256`. Keep provenance, licenses and notices. macOS releases are not distribution-signed/notarized, and Windows releases lack Authenticode. Gatekeeper, SmartScreen or organization policy can restrict execution. Follow your administrator's approved process; checksums and runtime signing mechanics do not establish publisher identity.

Local measurement needs no hosted account. Measurement and optimization use Linux x86_64 (`linux/amd64`) Docker containers on every host. Provision Docker, the declared trusted image and workload tools first. Windows requires Docker Desktop with its WSL 2 Linux-container backend. ARM Macs use x86_64 emulation, which results must identify; these are not native ARM measurements. Native macOS/Windows measurement is not provided.

Quote paths with spaces or non-ASCII characters using your shell's syntax, use host-appropriate executable paths, and confirm Docker can access the configured mounts. Missing tools, unavailable Docker and permission failures require explicit repair; Perfine does not install prerequisites. Graceful cancellation stops new candidates; immediate cancellation also stops active adapter work through supported host controls. Windows does not provide identical Unix signal behavior. Retain partial evidence and inspect cleanup diagnostics before retrying. Cross-host resume is not guaranteed; incompatible immutable conditions require a new experiment.

A successful execution does not necessarily mean performance improved. Evidence and incomplete uploads remain local for inspection and manual resend. Publication requires separate explicit approval and creates drafts only. Local AI authentication must use a supported file in the isolated agent environment; keychain-only credentials are not automatically exported. Cloud AI optimization remains unavailable until its adapter supports and tests an official unattended authentication path.

Automated package/executable checks, minimum-OS validation, full Docker workflows and live-provider checks are separate evidence. A newer host or Windows Server test does not prove the Windows 11 minimum. Unperformed minimum-OS, Docker and account checks remain unverified; consult the exact release's validation records.
