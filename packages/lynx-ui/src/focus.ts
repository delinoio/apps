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
  const entry = useRef<FocusEntry>({ disabled, focus() {} });
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
  useEffect(() => {
    scope?.entries.set(token.current, entry.current);
    const value = { focus: () => entry.current.focus() };
    if (typeof handle === "function") handle(value);
    else if (handle) handle.current = value;
    if (autoFocus && !disabled) value.focus();
    return () => {
      scope?.entries.delete(token.current);
      if (typeof handle === "function") handle(null);
      else if (handle) handle.current = null;
    };
  }, [scope, handle, autoFocus, disabled]);
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
