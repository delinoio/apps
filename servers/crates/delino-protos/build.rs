// SPDX-License-Identifier: FSL-1.1-ALv2
fn main() {
    connectrpc_build::Config::new()
        .files(&[
            "../../../protos/devbase/v1/devbase.proto",
            "../../../protos/gittracker/v1/gittracker.proto",
            "../../../protos/delcore/v1/delcore.proto",
        ])
        .includes(&["../../../protos"])
        .include_file("_connectrpc.rs")
        .compile()
        .expect("generate the repository protocol bindings");
}
