// SPDX-License-Identifier: Apache-2.0
mod kernel;
fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .expect("size")
        .parse()
        .expect("size integer");
    assert!(n <= 4096);
    let values: Vec<u8> = (0..n).map(|i| ((i * 17 + i / 7) % 256) as u8).collect();
    let mut checksum = 0;
    for _ in 0..100 {
        checksum += kernel::count_pairs(std::hint::black_box(&values));
    }
    println!("{checksum}");
}
