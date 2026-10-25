// SPDX-License-Identifier: Apache-2.0
import {
  useEffect,
  useRef,
  type ReactNode,
  type RefObject,
} from "@lynx-js/react";
import type { KeyEvent } from "@lynx-js/types";
import { FocusContext, type FocusScope } from "../focus";
import { TextVariant, Variant, type FocusHandle } from "../theme";
import { Button } from "./button";
import { Text } from "./layout";

export interface DialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description: string;
  closeLabel: string;
  children: ReactNode;
  initialFocusRef?: RefObject<FocusHandle | null>;
  returnFocusRef: RefObject<FocusHandle | null>;
}
function OpenDialog({
  onOpenChange,
  title,
  description,
  closeLabel,
  children,
  initialFocusRef,
  returnFocusRef,
}: DialogProps) {
  const scope = useRef<FocusScope>({
    entries: new Map(),
    active: null,
  }).current;
  const initial = useRef(initialFocusRef);
  const restore = useRef(returnFocusRef);
  useEffect(() => {
    if (initial.current?.current) initial.current.current.focus();
    else
      Array.from(scope.entries.values())
        .find((entry) => !entry.disabled)
        ?.focus();
    return () => restore.current.current?.focus();
  }, [scope]);
  const onKey = (event: KeyEvent) => {
    if (event.key === "Escape") {
      event.preventDefault?.();
      event.stopPropagation?.();
      onOpenChange(false);
    }
    if (event.key !== "Tab") return;
    event.preventDefault?.();
    event.stopPropagation?.();
    const entries = Array.from(scope.entries.entries()).filter(
      ([, entry]) => !entry.disabled,
    );
    if (!entries.length) return;
    const index = entries.findIndex(([token]) => token === scope.active);
    const next =
      (index + (event.shiftKey ? -1 : 1) + entries.length) % entries.length;
    scope.active = entries[next][0];
    entries[next][1].focus();
  };
  return (
    <FocusContext.Provider value={scope}>
      <view
        className="lui-dialog-backdrop"
        accessibility-exclusive-focus
        catchtap={() => {}}
        bindkeydown={onKey}
      >
        <view
          className="lui-dialog"
          accessibility-role-description="dialog"
          accessibility-label={`${title}. ${description}`}
        >
          <Text variant={TextVariant.Title}>{title}</Text>
          <Text muted>{description}</Text>
          {children}
          <Button
            className="lui-dialog-close"
            variant={Variant.Outline}
            onPress={() => onOpenChange(false)}
          >
            {closeLabel}
          </Button>
        </view>
      </view>
    </FocusContext.Provider>
  );
}
export function Dialog(props: DialogProps) {
  return props.open ? <OpenDialog {...props} /> : null;
}
