// SPDX-License-Identifier: Apache-2.0
import { expect, it, rstest } from "@rstest/core";
import { invoke } from "../src/theme";

it("reports failed setValue without confirming or logging the entered value", () => {
  const onHostError = rstest.fn();
  const onSuccess = rstest.fn();
  const node = {
    invoke({ fail }: { fail?: (error: { code: number }) => void }) {
      return { exec: () => fail?.({ code: 3 }) };
    },
  } as unknown as Parameters<typeof invoke>[0];

  invoke(
    node,
    "setValue",
    { value: "private entry" },
    { onHostError },
    onSuccess,
  );

  expect(onSuccess).not.toHaveBeenCalled();
  expect(onHostError).toHaveBeenCalledWith({ method: "setValue", code: 3 });
  expect(JSON.stringify(onHostError.mock.calls)).not.toContain("private entry");
});
