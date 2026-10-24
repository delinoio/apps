# Perfine

Perfine measures workloads and evaluates profile-backed performance candidates. The first release supports Linux x86_64 with glibc 2.39 or later and Node.js 24 or later. Install the exact published version with `npm install --global perfine@0.1.0`, then run `perfine --help`.

The package includes the Rust executable and verifies its SHA-256 checksum before each launch. Installation does not download or execute additional code. Standalone release archives include the same executable, checksums, provenance and license notices. Verify the downloaded archive against the release `SHA256SUMS` before extraction; run `sha256sum -c SHA256SUMS` inside its `runtime` directory before execution.

Local measurement needs no hosted account. Provision the declared Docker image and workload tools before running. A successful execution does not necessarily mean performance improved. Evidence and incomplete uploads remain local for inspection and manual resend. Publication requires separate explicit approval and creates drafts only.

If a checksum fails, reinstall the exact version from a trusted release. Do not bypass verification. Other operating systems and architectures are unsupported. Cloud AI optimization is unavailable until the selected adapter implements an officially supported unattended credential path; use the local CLI with its supported local authentication.
