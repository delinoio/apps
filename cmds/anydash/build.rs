// SPDX-License-Identifier: Apache-2.0
fn main() {
    let mut config = prost_build::Config::new();
    config.file_descriptor_set_path(
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("anydash.bin"),
    );
    config
        .compile_protos(
            &["../../protos/anydash/v1/anydash.proto"],
            &["../../protos"],
        )
        .expect("AnyDash protocol");
    println!("cargo:rerun-if-changed=../../protos/anydash/v1/anydash.proto");
}
