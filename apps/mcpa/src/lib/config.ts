// SPDX-License-Identifier: Apache-2.0

export const PRODUCTION_API_URL = "https://mcpa.api.delino.io";
export const DEVELOPMENT_API_URL = "http://127.0.0.1:46329";

/**
 * Resolves the Managed CPA API origin. An explicit `PUBLIC_MCPA_API_URL`
 * always wins. Without it, only production builds target the production API;
 * development builds use the fixed loopback API listener.
 */
export function resolveApiUrl(env: {
  PUBLIC_MCPA_API_URL?: string;
  PROD?: boolean;
}): string {
  const configured = env.PUBLIC_MCPA_API_URL?.trim();
  const url =
    configured || (env.PROD ? PRODUCTION_API_URL : DEVELOPMENT_API_URL);
  return url.replace(/\/+$/, "");
}
