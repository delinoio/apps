// SPDX-License-Identifier: Apache-2.0
const std = @import("std");
const kernel = @import("kernel.zig");
pub fn main() !void {
    const args = try std.process.argsAlloc(std.heap.page_allocator);
    defer std.process.argsFree(std.heap.page_allocator, args);
    if (args.len != 2) return error.ExpectedSize;
    const n = try std.fmt.parseInt(usize, args[1], 10);
    if (n > 4096) return error.SizeTooLarge;
    var values: [4096]u8 = undefined;
    for (values[0..n], 0..) |*value, i| value.* = @intCast((i * 17 + i / 7) % 256);
    var checksum: u64 = 0;
    for (0..100) |_| {
        std.mem.doNotOptimizeAway(&values);
        checksum += kernel.count_pairs(values[0..n]);
    }
    // debug.print writes stderr; the driver records both output streams.
    std.debug.print("{d}\n", .{checksum});
}
