// SPDX-License-Identifier: Apache-2.0
import {
  createContext,
  useContext,
  useEffect,
  useRef,
  useState,
  type Ref,
} from "@lynx-js/react";
import type { NodesRef } from "@lynx-js/types";
import { FocusKind, invoke, useEnvironment, type FocusHandle } from "./theme";

export interface FocusEntry {
  focus(): void;
  disabled: boolean;
  path(callback: (indices: number[] | null) => void): void;
}
export interface FocusScope {
  entries: Map<object, FocusEntry>;
  active: object | null;
}
export const FocusContext = createContext<FocusScope | null>(null);

export function useControlFocus(
  disabled: boolean,
  kind = FocusKind.Control,
  handle?: Ref<FocusHandle>,
  autoFocus = false,
) {
  const node = useRef<NodesRef>(null);
  const token = useRef({});
  const scope = useContext(FocusContext);
  const environment = useEnvironment();
  const [focused, setFocused] = useState(false);
  const entry = useRef<FocusEntry>({ disabled, focus() {}, path() {} });
  entry.current.disabled = disabled;
  entry.current.focus = () => {
    if (entry.current.disabled) return;
    if (scope) scope.active = token.current;
    invoke(
      node.current,
      kind === FocusKind.Input ? "focus" : "setFocus",
      kind === FocusKind.Input ? {} : { focus: true },
      environment,
    );
  };
  entry.current.path = (callback) => {
    if (!node.current) {
      callback(null);
      return;
    }
    try {
      node.current
        .path((result, status) => {
          // Lynx documents `path`, while its 4.3 types call it `data`.
          // Accept both until the host result and type declaration agree.
          const path = result as {
            path?: { index: number }[];
            data?: { index: number }[];
          } | null;
          const nodes = path?.path ?? path?.data;
          callback(
            status.code === 0 && Array.isArray(nodes)
              ? nodes.map(({ index }) => index).reverse()
              : null,
          );
        })
        .exec();
    } catch {
      callback(null);
    }
  };
  useEffect(() => {
    scope?.entries.set(token.current, entry.current);
    return () => {
      scope?.entries.delete(token.current);
    };
  }, [scope]);
  useEffect(() => {
    const value = { focus: () => entry.current.focus() };
    const cleanup = typeof handle === "function" ? handle(value) : undefined;
    if (handle && typeof handle !== "function") handle.current = value;
    return () => {
      if (typeof handle === "function") {
        handle(null);
        if (typeof cleanup === "function") cleanup();
      } else if (handle) handle.current = null;
    };
  }, [scope, handle, autoFocus, disabled]);
  useEffect(() => {
    if (autoFocus && !disabled) entry.current.focus();
  }, [scope, autoFocus, disabled]);
  return {
    node,
    focused,
    focus: () => entry.current.focus(),
    onFocus: () => {
      setFocused(true);
      if (scope) scope.active = token.current;
    },
    onBlur: () => setFocused(false),
  };
}
