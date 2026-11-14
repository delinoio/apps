// SPDX-License-Identifier: Apache-2.0
import { describe, expect, test } from "@rstest/core";
import {
  DEFAULT_ACCOUNTS_API_ORIGIN,
  resolveAccountsApiOrigin,
} from "./config.ts";

describe("resolveAccountsApiOrigin", () => {
  test("uses the production origin when the value is unset or empty", () => {
    expect(resolveAccountsApiOrigin(undefined)).toBe(
      DEFAULT_ACCOUNTS_API_ORIGIN,
    );
    expect(resolveAccountsApiOrigin("  ")).toBe(DEFAULT_ACCOUNTS_API_ORIGIN);
  });

  test("normalizes an explicit origin", () => {
    expect(resolveAccountsApiOrigin("http://127.0.0.1:8100/")).toBe(
      "http://127.0.0.1:8100",
    );
    expect(resolveAccountsApiOrigin("https://ACCOUNTS.api.delino.io")).toBe(
      "https://accounts.api.delino.io",
    );
  });

  test.each([
    "not a url",
    "ftp://accounts.api.delino.io",
    "https://accounts.api.delino.io/v1",
    "https://accounts.api.delino.io/?debug=1",
    "https://accounts.api.delino.io/#top",
  ])("rejects %s", (value) => {
    expect(() => resolveAccountsApiOrigin(value)).toThrow("PUBLIC_API_ORIGIN");
  });
});
