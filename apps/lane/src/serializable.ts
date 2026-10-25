// SPDX-License-Identifier: Apache-2.0
import {
  create,
  type DescMessage,
  type MessageInitShape,
  type MessageShape,
} from "@bufbuild/protobuf";
// ReactLynx transports component props as JSON between its two runtimes.
// Protobuf int64 values therefore stay decimal strings in UI state, with exact
// bigint restoration only at the background adapter boundary.
export type UI<T> = T extends Uint8Array
  ? number[]
  : T extends bigint
    ? string
    : T extends readonly (infer E)[]
      ? UI<E>[]
      : T extends object
        ? { [K in keyof T as K extends "$unknown" ? never : K]: UI<T[K]> }
        : T;
export function toUI<T>(value: T): UI<T> {
  if (typeof value === "bigint") return value.toString() as UI<T>;
  if (value instanceof Uint8Array) return Array.from(value) as UI<T>;
  if (Array.isArray(value)) return value.map(toUI) as UI<T>;
  if (value && typeof value === "object")
    return Object.fromEntries(
      Object.entries(value)
        .filter(([key]) => key !== "$unknown")
        .map(([key, v]) => [key, toUI(v)]),
    ) as UI<T>;
  return value as UI<T>;
}
export function fromUI<S extends DescMessage>(
  schema: S,
  value: UI<MessageInitShape<S>>,
): MessageShape<S> {
  const object = { ...value } as Record<string, unknown>;
  for (const field of schema.fields) {
    const container = field.oneof
      ? (object[field.oneof.localName] as
          { case: string; value: unknown } | undefined)
      : undefined;
    if (field.oneof && container?.case !== field.localName) continue;
    let item = container ? container.value : object[field.localName];
    if (item === undefined || item === null) continue;
    if (field.fieldKind === "scalar" && field.scalar === 12)
      item = Uint8Array.from(item as number[]);
    else if (field.fieldKind === "message")
      item = fromUI(field.message, item as never);
    else if (
      field.fieldKind === "scalar" &&
      [3, 4, 6, 16, 18].includes(field.scalar)
    )
      item = BigInt(item as string);
    else if (field.fieldKind === "list" && field.listKind === "message")
      item = (item as unknown[]).map((x) => fromUI(field.message, x as never));
    if (container)
      object[field.oneof!.localName] = { case: container.case, value: item };
    else object[field.localName] = item;
  }
  return create(schema, object as MessageInitShape<S>);
}
