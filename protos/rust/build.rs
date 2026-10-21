// SPDX-License-Identifier: Apache-2.0
fn main() {
    connectrpc_build::Config::new()
        .files(&[
            "../devbase/v1/devbase.proto",
            "../gittracker/v1/gittracker.proto",
            "../delcore/v1/delcore.proto",
        ])
        .includes(&[".."])
        .include_file("_connectrpc.rs")
        .compile()
        .expect("generate the repository protocol bindings");
}
