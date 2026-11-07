// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "@rstest/core";
import { parseRoute, paths, RouteName, TenantTab } from "../src/lib/router";

describe("parseRoute percent-encoding", () => {
  it.each(["/tenants/%", "/tenants/%E0%A4%A", "/tenants/t1/resources/%zz"])(
    "returns not-found for malformed path %s",
    (path) => {
      expect(parseRoute(path)).toEqual({ name: RouteName.NotFound });
    },
  );

  it("decodes valid encoded tenant and resource IDs", () => {
    expect(parseRoute("/tenants/%E2%9C%93")).toEqual({
      name: RouteName.Tenant,
      tenantId: "✓",
      tab: TenantTab.Resources,
    });
    expect(parseRoute(paths.resource("a/b", "c d"))).toMatchObject({
      name: RouteName.Resource,
      tenantId: "a/b",
      resourceId: "c d",
    });
  });
});
