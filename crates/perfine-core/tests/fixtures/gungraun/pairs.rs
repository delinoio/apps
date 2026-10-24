// SPDX-License-Identifier: Apache-2.0
// External executable API: https://docs.rs/gungraun/0.19.4/gungraun/struct.Command.html
use gungraun::{binary_benchmark, binary_benchmark_group, main, Command};
#[binary_benchmark]
fn pairs() -> Command {
    Command::new(std::env::var("PERFINE_FIXTURE_BINARY").expect("absolute binary path"))
        .arg(std::env::var("PERFINE_FIXTURE_SIZE").expect("case size"))
        .build()
}
binary_benchmark_group!(name = fixture; benchmarks = pairs);
main!(binary_benchmark_groups = fixture);
