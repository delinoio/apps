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
    "https://accounts.api.delino.io?",
    "https://accounts.api.delino.io#",
    "https://accounts.api.delino.io/?",
    "https://accounts.api.delino.io/#",
    "https://accounts.api.delino.io?#",
    "https://accounts.api.delino.io/?#",
  ])("rejects %s", (value) => {
    expect(() => resolveAccountsApiOrigin(value)).toThrow("PUBLIC_API_ORIGIN");
  });

  test.each([
    "https://user:pass@accounts.api.delino.io",
    "https://user@accounts.api.delino.io",
    "https://:pass@accounts.api.delino.io",
    "http://user:pass@127.0.0.1:8100/",
    "https://user:pass@accounts.api.delino.io/v1",
    "https://user:pass@accounts.api.delino.io/?debug=1#top",
    "ftp://user:pass@accounts.api.delino.io",
  ])("rejects credentials in %s without echoing them", (value) => {
    let message = "";
    try {
      resolveAccountsApiOrigin(value);
    } catch (error) {
      message = (error as Error).message;
    }
    expect(message).toContain("PUBLIC_API_ORIGIN");
    expect(message).toContain("credentials");
    expect(message).not.toContain("pass");
    expect(message).not.toContain("user");
  });

  test.each([
    "https://user:pass@",
    "https://user:pass@acc ounts.api.delino.io",
    "user:pass@accounts.api.delino.io",
    "ftp://accounts.api.delino.io/secret-path?token=pass",
    "https://accounts.api.delino.io/secret-path?token=pass#pass",
  ])("does not echo the raw value of rejected input %s", (value) => {
    let message = "";
    try {
      resolveAccountsApiOrigin(value);
    } catch (error) {
      message = (error as Error).message;
    }
    expect(message).toContain("PUBLIC_API_ORIGIN");
    expect(message).not.toContain("pass");
    expect(message).not.toContain("secret-path");
  });
});
