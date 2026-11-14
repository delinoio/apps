// SPDX-License-Identifier: Apache-2.0
declare module "*.css";

interface ImportMetaEnv {
  readonly SSG_MD: boolean;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
