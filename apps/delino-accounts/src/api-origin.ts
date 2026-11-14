// SPDX-License-Identifier: Apache-2.0
import { readAccountsApiOrigin } from "./config.ts";

/**
 * API server origin inlined into the client bundle at build time.
 *
 * This module is evaluated only inside the bundler (it reads
 * `import.meta.env`, which plain Node does not provide). It is kept apart from
 * `config.ts` so that `rsbuild.config.ts` and the unit tests can import the
 * pure resolver without evaluating the environment.
 */
export const ACCOUNTS_API_ORIGIN: string = readAccountsApiOrigin(
  import.meta.env,
);
