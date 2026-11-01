// SPDX-License-Identifier: Apache-2.0
import { useCallback } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { createConnectQueryKey } from "@connectrpc/connect-query";
import type { DescMethod } from "@bufbuild/protobuf";
import { McpaService } from "@repo/protos/mcpa/v1/mcpa_pb";

export const rpc = McpaService.method;

/** Invalidates every cached request of the given methods. */
export function useInvalidate() {
  const queryClient = useQueryClient();
  return useCallback(
    (...methods: DescMethod[]) =>
      Promise.all(
        methods.map((schema) =>
          queryClient.invalidateQueries({
            queryKey: createConnectQueryKey({
              schema: schema as never,
              cardinality: undefined,
            }),
          }),
        ),
      ),
    [queryClient],
  );
}
