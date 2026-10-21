# License contract

Repository-owned files under `servers/`, including code, migrations, configuration examples, metadata and documentation, use the Functional Source License, Version 1.1, ALv2 Future License (`FSL-1.1-ALv2`). `servers/LICENSE` contains the complete terms and `servers/NOTICE` identifies Delino as the copyright owner. This directory license overrides the root Apache-2.0 default for all repository-owned files in that subtree, whether or not an individual file has a license header. Server package metadata must use `FSL-1.1-ALv2`.

For each server version, the additional Apache-2.0 grant takes effect on the second anniversary of the date Delino makes that version available under FSL, as specified by its license terms. A new version has its own period. Retain version and release dates with distributions. FSL is source-available; do not describe the initial server license as Apache-2.0 or an unrestricted open-source grant.

Repository-owned files outside `servers/`, including clients, shared protocol source, generated bindings and root contracts, use Apache-2.0. Root `LICENSE` begins with a scope notice that states Apache-2.0 does not apply to the entire repository, identifies the server and third-party exceptions, and then reproduces the complete Apache-2.0 terms. Root `NOTICE` identifies Delino as the copyright owner. The root package license describes this default and does not override the server subtree license. Root `NOTICE` explicitly excludes `servers/` from the immediate Apache-2.0 grant. Including an Apache-2.0 component or its notices in a server distribution does not relicense the server files; including FSL server files does not relicense the independent Apache-2.0 protocol files.

Include `servers/LICENSE` and `servers/NOTICE` with server distributions, and root `LICENSE` and `NOTICE` for included Apache-2.0 protocol code. Other distributions include root `LICENSE` and `NOTICE`. Preserve third-party licenses, copyright notices and CEF notices. Imported third-party code and bundled fonts retain their original licenses; neither repository license replaces them.

Official terms: [FSL-1.1-ALv2](https://fsl.software/FSL-1.1-ALv2.template.md) and [Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0).

## Server Rust workspace

`servers/Cargo.toml` owns an independent Cargo workspace for server ports. Shared server Rust modules belong in `servers/crates/` under FSL-1.1-ALv2, separate from the Apache-2.0 client workspace and root `crates/`. See the [server Rust workspace contract](servers-rust-workspace-contract.md).
