// SPDX-License-Identifier: Apache-2.0
// The ReactLynx testing environment leaves NodesRef UI methods unimplemented.
// Record focus requests and report success so components that focus inputs or
// controls render as they do on a host.
export const uiCalls: { method: string; params: unknown }[] = [];
const query = (
  globalThis as unknown as {
    lynx?: {
      createSelectorQuery?: () => { selectUniqueID(id: number): object };
    };
  }
).lynx?.createSelectorQuery?.();
const ref = query?.selectUniqueID(-1);
if (ref) {
  const prototype = Object.getPrototypeOf(ref) as Record<string, unknown>;
  prototype.invoke = function invoke(options: {
    method: string;
    params?: unknown;
    success?: (value: unknown) => void;
  }) {
    return {
      exec() {
        uiCalls.push({ method: options.method, params: options.params });
        options.success?.({});
      },
    };
  };
  prototype.path = function path(
    callback: (value: unknown, status: { code: number }) => void,
  ) {
    return { exec: () => callback({ path: [] }, { code: 0 }) };
  };
}
(globalThis as { __anydashUiCalls?: typeof uiCalls }).__anydashUiCalls =
  uiCalls;
