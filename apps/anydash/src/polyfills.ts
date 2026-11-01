// SPDX-License-Identifier: Apache-2.0
import { TextDecoder, TextEncoder } from "text-encoding-utf-8";

// Native Lynx isolates do not provide the web Encoding API or Fetch `Headers`.
// Generated Protobuf descriptors decode UTF-8 during module evaluation, and
// Connect errors construct `Headers` for their metadata. Keep this module first
// in the entry's import graph and run it in both threads; do not mark it
// background-only. Browser implementations stay untouched. Remove each shim
// once every supported Lynx host supplies the API.
if (typeof globalThis.TextEncoder === "undefined") {
  Object.defineProperty(globalThis, "TextEncoder", {
    value: TextEncoder,
    configurable: true,
    writable: true,
  });
}
if (typeof globalThis.TextDecoder === "undefined") {
  Object.defineProperty(globalThis, "TextDecoder", {
    value: TextDecoder,
    configurable: true,
    writable: true,
  });
}

type HeaderSource =
  MinimalHeaders | Record<string, string> | [string, string][] | undefined;
class MinimalHeaders {
  #values = new Map<string, string>();
  constructor(init?: HeaderSource) {
    if (!init) return;
    if (init instanceof MinimalHeaders)
      init.forEach((value, key) => this.append(key, value));
    else if (Array.isArray(init))
      for (const [key, value] of init) this.append(key, value);
    else for (const key of Object.keys(init)) this.append(key, init[key]);
  }
  append(key: string, value: string) {
    const name = key.toLowerCase();
    const previous = this.#values.get(name);
    this.#values.set(
      name,
      previous === undefined ? value : `${previous}, ${value}`,
    );
  }
  set(key: string, value: string) {
    this.#values.set(key.toLowerCase(), value);
  }
  get(key: string) {
    return this.#values.get(key.toLowerCase()) ?? null;
  }
  has(key: string) {
    return this.#values.has(key.toLowerCase());
  }
  delete(key: string) {
    this.#values.delete(key.toLowerCase());
  }
  forEach(
    callback: (value: string, key: string, parent: MinimalHeaders) => void,
  ) {
    for (const [key, value] of this.#values) callback(value, key, this);
  }
  entries() {
    return this.#values.entries();
  }
  keys() {
    return this.#values.keys();
  }
  values() {
    return this.#values.values();
  }
  [Symbol.iterator]() {
    return this.#values.entries();
  }
}
if (typeof globalThis.Headers === "undefined") {
  Object.defineProperty(globalThis, "Headers", {
    value: MinimalHeaders,
    configurable: true,
    writable: true,
  });
}
