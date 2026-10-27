// SPDX-License-Identifier: Apache-2.0
// Minimal ReactLynx stand-in for Node tests. It is bundled in place of
// "@lynx-js/react" so the real Lane components render synchronously-inspectable
// host trees with working state, refs and effects. It is not a general React
// implementation: no context or Suspense.
export type Element = {
  type: string | Component | typeof Fragment;
  props: Record<string, unknown>;
  key?: string;
};
type Component = (props: any) => unknown;
export interface HostNode {
  type: string;
  props: Record<string, any>;
  children: (HostNode | string)[];
}
interface Instance {
  hooks: any[];
  cursor: number;
  seen: boolean;
}

export const Fragment = Symbol("Fragment");
export function jsx(
  type: Element["type"],
  props: Record<string, unknown>,
  key?: string,
): Element {
  return { type, props: props ?? {}, key };
}
export const jsxs = jsx;
export const jsxDEV = jsx;

const instances = new Map<string, Instance>();
let current: Instance | undefined;
let pendingEffects: (() => void)[] = [];
let scheduled = false;
let root:
  { component: Component; props: unknown; tree: HostNode[] } | undefined;

function schedule() {
  if (scheduled || !root) return;
  scheduled = true;
  queueMicrotask(() => {
    scheduled = false;
    flush();
  });
}
function slot<T>(init: () => T): T {
  if (!current) throw new Error("Hook called outside render");
  const index = current.cursor++;
  if (!(index in current.hooks)) current.hooks[index] = init();
  return current.hooks[index];
}
export function useState<T>(initial: T | (() => T)) {
  const cell = slot(() => ({
    value: typeof initial === "function" ? (initial as () => T)() : initial,
    set: undefined as undefined | ((v: T | ((p: T) => T)) => void),
  }));
  cell.set ??= (next) => {
    const value =
      typeof next === "function"
        ? (next as (p: T) => T)(cell.value as T)
        : next;
    if (Object.is(value, cell.value)) return;
    cell.value = value;
    schedule();
  };
  return [cell.value as T, cell.set] as const;
}
export function useRef<T>(initial: T) {
  return slot(() => ({ current: initial }));
}
export function useCallback<T>(callback: T, deps: unknown[]): T {
  const cell = slot(() => ({ value: callback, deps }));
  if (
    deps.length !== cell.deps.length ||
    deps.some((d, i) => !Object.is(d, cell.deps[i]))
  ) {
    cell.value = callback;
    cell.deps = deps;
  }
  return cell.value;
}
export function useEffect(effect: () => void | (() => void), deps?: unknown[]) {
  const cell = slot(() => ({
    deps: undefined as unknown[] | undefined,
    cleanup: undefined as void | (() => void),
    ran: false,
  }));
  const changed =
    !cell.ran ||
    !deps ||
    !cell.deps ||
    deps.length !== cell.deps.length ||
    deps.some((d, i) => !Object.is(d, cell.deps![i]));
  if (!changed) return;
  cell.ran = true;
  cell.deps = deps;
  pendingEffects.push(() => {
    cell.cleanup?.();
    cell.cleanup = effect();
  });
}

function expand(node: unknown, path: string, out: (HostNode | string)[]) {
  if (node === null || node === undefined || typeof node === "boolean") return;
  if (typeof node === "string" || typeof node === "number") {
    out.push(String(node));
    return;
  }
  if (Array.isArray(node)) {
    node.forEach((child, i) => {
      const key =
        child && typeof child === "object" && "key" in child
          ? child.key
          : undefined;
      expand(child, `${path}[${key ?? i}]`, out);
    });
    return;
  }
  const element = node as Element;
  const here = `${path}/${element.key ?? ""}${typeof element.type === "string" ? element.type : ((element.type as Component).name ?? "")}`;
  const { children, ...rest } = element.props as { children?: unknown };
  if (element.type === Fragment) {
    expand(children, here, out);
  } else if (typeof element.type === "function") {
    let instance = instances.get(here);
    if (!instance)
      instances.set(here, (instance = { hooks: [], cursor: 0, seen: true }));
    instance.cursor = 0;
    instance.seen = true;
    const previous = current;
    current = instance;
    let rendered: unknown;
    try {
      rendered = element.type({ ...rest, children });
    } finally {
      current = previous;
    }
    expand(rendered, here, out);
  } else {
    const host: HostNode = {
      type: element.type as string,
      props: rest,
      children: [],
    };
    expand(children, here, host.children);
    out.push(host);
  }
}
function flush() {
  if (!root) return;
  for (const instance of instances.values()) instance.seen = false;
  const tree: (HostNode | string)[] = [];
  expand(
    { type: root.component, props: root.props as Record<string, unknown> },
    "",
    tree,
  );
  root.tree = tree.filter((n): n is HostNode => typeof n !== "string");
  for (const [key, instance] of [...instances]) {
    if (instance.seen) continue;
    for (const hook of instance.hooks)
      if (hook && typeof hook.cleanup === "function") hook.cleanup();
    instances.delete(key);
  }
  const effects = pendingEffects;
  pendingEffects = [];
  for (const effect of effects) effect();
}
export function mount(component: Component, props: unknown) {
  root = { component, props, tree: [] };
  instances.clear();
  pendingEffects = [];
  scheduled = false;
  flush();
  return {
    tree: () => root!.tree,
    // Let queued renders, effects and already-resolved promises complete.
    async settle() {
      for (let i = 0; i < 5; i++)
        await new Promise((resolve) => setImmediate(resolve));
    },
    unmount() {
      for (const instance of instances.values())
        for (const hook of instance.hooks)
          if (hook && typeof hook.cleanup === "function") hook.cleanup();
      instances.clear();
      root = undefined;
    },
  };
}
