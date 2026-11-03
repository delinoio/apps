// SPDX-License-Identifier: Apache-2.0
// The ReactLynx testing environment leaves NodesRef UI methods unimplemented.
// Record focus requests and report success so components that focus inputs or
// controls render as they do on a host.
export const uiCalls: {
  method: string;
  params: unknown;
  element?: Element | null;
}[] = [];
type SelectToken = { _nodeSelectToken?: { type: number; identifier: string } };
// Resolves the rendered element a NodesRef selects, by selector or UI sign.
function elementOf(ref: SelectToken) {
  const token = ref._nodeSelectToken;
  if (!token || typeof document === "undefined") return null;
  return token.type === 0
    ? document.querySelector(token.identifier)
    : (Array.from(document.querySelectorAll("*")).find(
        (element) =>
          (element as Element & { $$uiSign?: number }).$$uiSign ===
          Number(token.identifier),
      ) ?? null);
}
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
  prototype.invoke = function invoke(
    this: SelectToken,
    options: {
      method: string;
      params?: unknown;
      success?: (value: unknown) => void;
    },
  ) {
    return {
      exec: () => {
        uiCalls.push({
          method: options.method,
          params: options.params,
          element: elementOf(this),
        });
        options.success?.({});
      },
    };
  };
  // Report the documented sibling-index path, root first after reversal, so
  // focus containment follows the rendered order as it does on a host.
  prototype.path = function path(
    this: SelectToken,
    callback: (value: unknown, status: { code: number }) => void,
  ) {
    return {
      exec: () => {
        const node = elementOf(this);
        if (!node) return callback({ path: [] }, { code: 0 });
        const path: { index: number }[] = [];
        for (
          let current: Node | null = node;
          current?.parentNode;
          current = current.parentNode
        )
          path.push({
            index: Array.prototype.indexOf.call(
              current.parentNode.childNodes,
              current,
            ),
          });
        callback({ path }, { code: 0 });
      },
    };
  };
}
(globalThis as { __anydashUiCalls?: typeof uiCalls }).__anydashUiCalls =
  uiCalls;
