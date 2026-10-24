// SPDX-License-Identifier: Apache-2.0
#[inline(never)]
pub fn count_pairs(values: &[u8]) -> u64 {
    let mut counts = [0_u64; 256];
    let mut pairs = 0;
    for &value in values {
        pairs += counts[usize::from(value)];
        counts[usize::from(value)] += 1;
    }
    pairs
}
