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

  it("normalizes typed budget amounts to canonical decimals", () => {
    expect(normalizeUsdInput("25")).toBe("25");
    expect(normalizeUsdInput("$25.50")).toBe("25.5");
    expect(normalizeUsdInput("010.00")).toBe("10");
    expect(normalizeUsdInput("0")).toBe("0");
    expect(normalizeUsdInput("1.234")).toBeUndefined();
    expect(normalizeUsdInput("-1")).toBeUndefined();
    expect(normalizeUsdInput("ten")).toBeUndefined();
  });
});
