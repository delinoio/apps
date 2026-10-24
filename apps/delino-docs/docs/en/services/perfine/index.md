# Perfine

Perfine helps you investigate performance with profiles, isolated code candidates, correctness checks, and reproducible measurements. A run can finish with no candidate or no improvement. Perfine does not promise that every project becomes faster.

Run the CLI locally without a Perfine account, hosted space, or GitHub App. Local AI optimization uses your own Codex CLI or Claude Code authentication. Measurement does not need AI authentication. Hosting is optional; hosted results are private. Perfine hosting is free in the first release; your AI subscription and GitHub Actions costs remain yours.

The first execution target is Linux x86_64. Rust, C, C++, Zig, Go, and JavaScript have configured measurement and profile adapters. Adapter availability, automated fixtures, and a successful package build do not establish a performance improvement in your application or native acceptance on your machine.

## Choose a workflow

- [Install and configure local execution](/services/perfine/setup).
- [Run, inspect, cancel, and resume an experiment](/services/perfine/cli).
- [Configure language adapters and extensions](/services/perfine/adapters).
- [Understand policies and evidence](/services/perfine/policies).
- [Use private hosted spaces and upload evidence](/services/perfine/hosting).
- [Review and approve a draft pull request](/services/perfine/publication).
- [Set up a manual GitHub Action](/services/perfine/github-actions).
- [Resolve compatibility and execution problems](/services/perfine/troubleshooting).

## Availability and limits

These pages describe the v1 interfaces. Confirm that the exact package or release asset exists before installing it. Packaging checks do not prove that npm or GitHub release publication has occurred. Cloud AI optimization is currently unavailable; use the local CLI. Upload currently needs a prepared envelope, and some native artifact formats cannot be accepted by hosting. See the upload guide before relying on hosted evaluation.

The dashboard address is [perfine.delino.io](https://perfine.delino.io). These instructions do not certify a live deployment, GitHub account integration, AI account integration, or native profiler run. UI, CLI, and Perfine documentation are English; source code and user input can use other languages.
