// SPDX-License-Identifier: Apache-2.0
#[inline(never)]
pub fn count_pairs(values: &[u8]) -> u64 {
    let mut pairs = 0;
    for i in 0..values.len() {
        for j in i + 1..values.len() {
            pairs += u64::from(values[i] == values[j]);
        }
    }
    pairs
}
