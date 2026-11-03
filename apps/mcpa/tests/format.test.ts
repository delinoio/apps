// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "@rstest/core";
import {
  estimateUsdFromBytes,
  formatBytes,
  formatGib,
  formatUsd,
  GIB,
  normalizeUsdInput,
} from "../src/lib/format";

describe("usage USD estimates", () => {
  it("prices exactly USD 0.07 per GiB", () => {
    expect(estimateUsdFromBytes(GIB)).toBe("$0.07");
    expect(estimateUsdFromBytes(100n * GIB)).toBe("$7.00");
    expect(estimateUsdFromBytes(1_000_000n * GIB)).toBe("$70,000.00");
  });

  it("shows two to six fraction digits, rounded half up", () => {
    expect(estimateUsdFromBytes(0n)).toBe("$0.00");
    // 1 MiB = 0.07 / 1024 USD = 0.0000683593…
    expect(estimateUsdFromBytes(1n << 20n)).toBe("$0.000068");
    // 1.5 GiB = 0.105 USD
    expect(estimateUsdFromBytes(GIB + GIB / 2n)).toBe("$0.105");
    expect(formatUsd("0.0000005")).toBe("$0.000001");
    expect(formatUsd("0.0000004")).toBe("$0.00");
    expect(formatUsd("12.3")).toBe("$12.30");
    expect(formatUsd("1234.5678915")).toBe("$1,234.567892");
    expect(formatUsd("")).toBe("$0.00");
  });

  it("rounds the exact full-precision API strings for display only", () => {
    // The API returns exact decimals (up to 30 GiB and 32 USD fraction digits).
    expect(formatUsd("0.00000006519258022308349609375")).toBe("$0.00");
    expect(formatUsd("0.07000000000000000000000000000001")).toBe("$0.07");
    expect(formatUsd("0.0000005000000000000000000000001")).toBe("$0.000001");
    expect(formatGib("0.000000931322574615478515625")).toBe("< 0.001 GiB");
    expect(formatGib("8589934591.999999999068677425384521484375")).toBe(
      "8,589,934,592.000 GiB",
    );
  });

  it("keeps unparseable server values visible", () => {
    expect(formatUsd("n/a")).toBe("n/a");
  });

  it("formats GiB and byte counts", () => {
    expect(formatGib("1.0005")).toBe("1.001 GiB");
    expect(formatGib("0.0001")).toBe("< 0.001 GiB");
    expect(formatGib("0")).toBe("0.000 GiB");
    expect(formatBytes(512n)).toBe("512 B");
    expect(formatBytes(1536n)).toBe("1.5 KiB");
    expect(formatBytes(GIB * 3n)).toBe("3.0 GiB");
  });

  it("promotes the byte unit when rounding reaches 1024", () => {
    expect(formatBytes(1023n)).toBe("1023 B");
    expect(formatBytes(1048575n)).toBe("1.0 MiB");
    expect(formatBytes(1073741823n)).toBe("1.0 GiB");
    expect(formatBytes(102399n)).toBe("100 KiB");
    expect(formatBytes(1047552n)).toBe("1023 KiB");
    expect(formatBytes(1n << 40n)).toBe("1.0 TiB");
    expect(formatBytes((1n << 50n) - 1n)).toBe("1.0 PiB");
  });

  it("caps the byte unit at PiB", () => {
    expect(formatBytes(1n << 50n)).toBe("1.0 PiB");
    expect(formatBytes(1n << 60n)).toBe("1024 PiB");
    expect(formatBytes((1n << 60n) - 1n)).toBe("1024 PiB");
  });

  it("normalizes typed budget amounts to canonical decimals", () => {
    expect(normalizeUsdInput("25")).toBe("25");
    expect(normalizeUsdInput("$25.50")).toBe("25.5");
    expect(normalizeUsdInput("010.00")).toBe("10");
    expect(normalizeUsdInput("0")).toBe("0");
    expect(normalizeUsdInput("1.234")).toBe("1.234");
    expect(normalizeUsdInput("0.075")).toBe("0.075");
    expect(normalizeUsdInput("0.000001")).toBe("0.000001");
    expect(normalizeUsdInput("0.1234560")).toBeUndefined();
    expect(normalizeUsdInput("1.1234567")).toBeUndefined();
    expect(normalizeUsdInput("999999999999")).toBe("999999999999");
    expect(normalizeUsdInput("1000000000000")).toBeUndefined();
    expect(normalizeUsdInput("0.000000")).toBe("0");
    expect(normalizeUsdInput("-1")).toBeUndefined();
    expect(normalizeUsdInput("ten")).toBeUndefined();
  });
});
