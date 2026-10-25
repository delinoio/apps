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

async function orderedEntries(scope: FocusScope) {
  const entries = Array.from(scope.entries.entries()).filter(
    ([, entry]) => !entry.disabled,
  );
  const paths = await Promise.all(
    entries.map(
      ([, entry]) =>
        new Promise<number[] | null>((resolve) => entry.path(resolve)),
    ),
  );
  return entries
    .map(([token, entry], index) => ({ token, entry, path: paths[index] }))
    .filter(
      (item): item is typeof item & { path: number[] } =>
        item.path !== null &&
        !item.entry.disabled &&
        scope.entries.get(item.token) === item.entry,
    )
    .sort((a, b) => {
      for (
        let index = 0;
        index < Math.min(a.path.length, b.path.length);
        index++
      ) {
        if (a.path[index] !== b.path[index])
          return a.path[index] - b.path[index];
      }
      return a.path.length - b.path.length;
    });
}

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
  const pending = useRef(Promise.resolve());
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    if (initial.current?.current) initial.current.current.focus();
    else {
      pending.current = pending.current.then(async () => {
        const entries = await orderedEntries(scope);
        if (mounted.current) entries[0]?.entry.focus();
      });
    }
    return () => {
      mounted.current = false;
      restore.current.current?.focus();
    };
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
    const shiftKey = event.shiftKey;
    pending.current = pending.current.then(async () => {
      const entries = await orderedEntries(scope);
      if (!mounted.current || !entries.length) return;
      const index = entries.findIndex(({ token }) => token === scope.active);
      const next =
        index < 0
          ? shiftKey
            ? entries.length - 1
            : 0
          : (index + (shiftKey ? -1 : 1) + entries.length) % entries.length;
      entries[next].entry.focus();
    });
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
