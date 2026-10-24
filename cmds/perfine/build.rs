// SPDX-License-Identifier: Apache-2.0
fn main() {
    connectrpc_build::Config::new()
        .files(&["../../protos/perfine/v1/perfine.proto"])
        .includes(&["../../protos"])
        .include_file("_connectrpc.rs")
        .compile()
        .expect("generate canonical Perfine client protocol");
}
