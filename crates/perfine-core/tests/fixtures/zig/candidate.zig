// SPDX-License-Identifier: Apache-2.0
pub noinline fn count_pairs(values: []const u8) u64 {
    var counts = [_]u64{0} ** 256;
    var pairs: u64 = 0;
    for (values) |value| {
        pairs += counts[value];
        counts[value] += 1;
    }
    return pairs;
}
