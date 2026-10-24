// SPDX-License-Identifier: Apache-2.0
mod kernel;
#[test]
fn independent_examples() {
    for (values, expected) in [
        (vec![], 0),
        (vec![0], 0),
        (vec![1, 2, 3], 0),
        (vec![255, 0, 255, 0, 255], 4),
        (vec![7; 100], 4950),
        ((0..=255).collect(), 0),
    ] {
        assert_eq!(kernel::count_pairs(&values), expected);
    }
}
