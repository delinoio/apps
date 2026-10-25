// SPDX-License-Identifier: Apache-2.0
fn main() {
    let mut config = prost_build::Config::new();
    config.file_descriptor_set_path(
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("lane.bin"),
    );
    config
        .compile_protos(&["../../protos/lane/v1/lane.proto"], &["../../protos"])
        .expect("Lane protocol");
    println!("cargo:rerun-if-changed=../../protos/lane/v1/lane.proto");
}
