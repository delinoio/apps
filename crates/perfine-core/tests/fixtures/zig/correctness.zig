// SPDX-License-Identifier: Apache-2.0
const std = @import("std");
const kernel = @import("kernel.zig");
test "independent examples" {
    try std.testing.expectEqual(@as(u64, 0), kernel.count_pairs(&.{}));
    try std.testing.expectEqual(@as(u64, 0), kernel.count_pairs(&.{0}));
    try std.testing.expectEqual(@as(u64, 0), kernel.count_pairs(&.{1, 2, 3}));
    try std.testing.expectEqual(@as(u64, 4), kernel.count_pairs(&.{255, 0, 255, 0, 255}));
    try std.testing.expectEqual(@as(u64, 4950), kernel.count_pairs(&([_]u8{7} ** 100)));
    var alphabet: [256]u8 = undefined;
    for (&alphabet, 0..) |*value, i| value.* = @intCast(i);
    try std.testing.expectEqual(@as(u64, 0), kernel.count_pairs(&alphabet));
}
