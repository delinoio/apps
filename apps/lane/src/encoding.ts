// SPDX-License-Identifier: Apache-2.0
import { TextDecoder, TextEncoder } from "text-encoding-utf-8";

// Native Lynx isolates do not provide the web Encoding API. Generated protobuf
// descriptors decode UTF-8 during module evaluation, before React effects run.
// Keep this first in the entry's import graph and run it in BOTH MTS and BTS;
// do not mark it background-only. Remove once all supported hosts supply these
// APIs. Existing browser implementations remain untouched.
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
