// SPDX-License-Identifier: Apache-2.0

/** Production origin of accounts-api. */
export const DEFAULT_ACCOUNTS_API_ORIGIN = "https://accounts.api.delino.io";

/**
 * Resolves the API server origin from a build-time value. An unset or empty
 * value selects the production origin. Values with a path, query or fragment
 * are rejected so that RPC paths are never resolved against a wrong base.
 */
export function resolveAccountsApiOrigin(value: string | undefined): string {
  const raw = value?.trim() || DEFAULT_ACCOUNTS_API_ORIGIN;
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    throw new Error(`PUBLIC_DELINO_ACCOUNTS_API_ORIGIN is not a URL: ${raw}`);
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") {
    throw new Error(
      `PUBLIC_DELINO_ACCOUNTS_API_ORIGIN must use http or https: ${raw}`,
    );
  }
  if (url.pathname !== "/" || url.search !== "" || url.hash !== "") {
    throw new Error(
      `PUBLIC_DELINO_ACCOUNTS_API_ORIGIN must be an origin without a path, query or fragment: ${raw}`,
    );
  }
  return url.origin;
}
