// SPDX-License-Identifier: Apache-2.0
import { createContext, useContext, type ReactNode } from "@lynx-js/react";
import type { CSSProperties, NodesRef } from "@lynx-js/types";

export enum Theme {
  Light = "light",
  Dark = "dark",
}
export enum Tone {
  Neutral = "neutral",
  Success = "success",
  Danger = "danger",
}
export enum Variant {
  Primary = "primary",
  Secondary = "secondary",
  Outline = "outline",
  Ghost = "ghost",
  Danger = "danger",
}
export enum TextVariant {
  Body = "body",
  Title = "title",
  Caption = "caption",
}
export enum Direction {
  Column = "column",
  Row = "row",
}
export enum FocusKind {
  Control = "control",
  Input = "input",
}

export interface HostError {
  method: string;
  code: number;
}
export interface UIEnvironment {
  onHostError: (error: HostError) => void;
}
const Environment = createContext<UIEnvironment>({
  onHostError: (error) => console.error("lynx-ui.host-method-failed", error),
});
export const useEnvironment = () => useContext(Environment);

export function UIProvider({
  children,
  theme = Theme.Light,
  style,
  onHostError,
}: {
  children: ReactNode;
  theme?: Theme;
  style?: CSSProperties;
  onHostError?: UIEnvironment["onHostError"];
}) {
  const inherited = useEnvironment();
  return (
    <Environment.Provider value={onHostError ? { onHostError } : inherited}>
      <view className={`lui-root lui-${theme}`} style={style}>
        {children}
      </view>
    </Environment.Provider>
  );
}

export function invoke(
  node: NodesRef | null,
  method: string,
  params: Record<string, unknown>,
  environment: UIEnvironment,
  onSuccess?: () => void,
) {
  node
    ?.invoke({
      method,
      params,
      success: onSuccess,
      fail: ({ code }) => {
        // setFocus is a Clay API. Mobile hosts instead expose accessibility focus.
        // Restrict this fallback to METHOD_NOT_FOUND so real host failures stay visible.
        if (method === "setFocus" && code === 3) {
          node
            .invoke({
              method: "requestAccessibilityFocus",
              params: {},
              fail: ({ code: fallbackCode }) =>
                environment.onHostError({
                  method: "requestAccessibilityFocus",
                  code: fallbackCode,
                }),
            })
            .exec();
        } else environment.onHostError({ method, code });
      },
    })
    .exec();
}

export interface FocusHandle {
  focus(): void;
}
export const classes = (...values: (string | undefined | false)[]) =>
  values.filter(Boolean).join(" ");
