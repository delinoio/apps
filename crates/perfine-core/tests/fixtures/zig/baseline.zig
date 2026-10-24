// SPDX-License-Identifier: Apache-2.0
pub noinline fn count_pairs(values: []const u8) u64 {
    var pairs: u64 = 0;
    for (values, 0..) |value, i| {
        for (values[i + 1 ..]) |other| {
            pairs += @intFromBool(value == other);
        }
    }
    return pairs;
}
