// SPDX-License-Identifier: Apache-2.0
import { useCallback, useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  createConnectQueryKey,
  useMutation,
  type UseMutationOptions,
} from "@connectrpc/connect-query";
import type {
  DescMessage,
  DescMethod,
  DescMethodUnary,
} from "@bufbuild/protobuf";
import { McpaService } from "@repo/protos/mcpa/v1/mcpa_pb";
import { uuidv7 } from "./uuid";

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

/** Mutation-key prefix of mutations whose request or response holds a secret. */
const SECRET_MUTATION_KEY = "mcpa-secret-mutation";

/**
 * Runs a mutation whose request or response holds a secret, such as an issued
 * call key or a provider API key.
 *
 * TanStack Query keeps every mutation, with its variables and data, in the
 * client's shared MutationCache until `gcTime` (five minutes by default)
 * passes after its last observer leaves. `reset()` only detaches the
 * observer. This hook gives the caller its own mutation key, sets `gcTime`
 * to zero and removes the caller's cached mutations on `discard()` and on
 * unmount, so the secret leaves memory with the dialog that shows or sends it.
 */
export function useSecretMutation<I extends DescMessage, O extends DescMessage>(
  schema: DescMethodUnary<I, O>,
  options: Omit<UseMutationOptions<I, O>, "mutationKey" | "gcTime"> = {},
) {
  const queryClient = useQueryClient();
  const [mutationKey] = useState(() => [SECRET_MUTATION_KEY, uuidv7()]);
  const mutation = useMutation(schema, { ...options, mutationKey, gcTime: 0 });
  const { reset } = mutation;
  const removeCached = useCallback(() => {
    const cache = queryClient.getMutationCache();
    for (const cached of cache.findAll({ mutationKey, exact: true }))
      cache.remove(cached);
  }, [queryClient, mutationKey]);
  useEffect(() => removeCached, [removeCached]);
  /** Clears the result, including its secret, from the hook and the cache. */
  const discard = useCallback(() => {
    reset();
    removeCached();
  }, [reset, removeCached]);
  return { ...mutation, discard };
}
