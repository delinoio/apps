// SPDX-License-Identifier: Apache-2.0
declare module "text-encoding-utf-8" {
  export class TextEncoder {
    readonly encoding: string;
    encode(input?: string): Uint8Array;
  }
  export class TextDecoder {
    constructor(
      label?: string,
      options?: { fatal?: boolean; ignoreBOM?: boolean },
    );
    readonly encoding: string;
    readonly fatal: boolean;
    readonly ignoreBOM: boolean;
    decode(
      input?: ArrayBuffer | ArrayBufferView,
      options?: { stream?: boolean },
    ): string;
  }
}
