// SPDX-License-Identifier: Apache-2.0

/** Production origin of accounts-api. */
export const DEFAULT_ACCOUNTS_API_ORIGIN = "https://accounts.api.delino.io";

/** Build-time environment that carries `PUBLIC_API_ORIGIN`. */
export interface AccountsBuildEnv {
  readonly PUBLIC_API_ORIGIN?: string;
}

/**
 * Reads and validates the API server origin from a build-time environment.
 * This is the only place that reads `PUBLIC_API_ORIGIN`: the Rsbuild config
 * passes `process.env` and the client bundle passes `import.meta.env`.
 */
export function readAccountsApiOrigin(env: AccountsBuildEnv): string {
  return resolveAccountsApiOrigin(env.PUBLIC_API_ORIGIN);
}

/**
 * Resolves the API server origin from a build-time value. An unset or empty
 * value selects the production origin. Values with credentials, a path, a
 * query or a fragment are rejected so that RPC paths are never resolved
 * against a wrong base and credentials never reach the public bundle.
 *
 * Every `PUBLIC_` variable is inlined into the client bundle and build logs
 * are often public, so no error message echoes the raw value: it may carry a
 * credential, including inside an otherwise malformed value.
 */
export function resolveAccountsApiOrigin(value: string | undefined): string {
  const raw = value?.trim() || DEFAULT_ACCOUNTS_API_ORIGIN;
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    throw new Error("PUBLIC_API_ORIGIN is not a valid URL");
  }
  // The credential check runs first so that no later diagnostic can leak it.
  // `url.origin` silently drops userinfo, so it has to be checked explicitly.
  if (url.username !== "" || url.password !== "") {
    throw new Error("PUBLIC_API_ORIGIN must not contain credentials");
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") {
    throw new Error(
      `PUBLIC_API_ORIGIN must use http or https, got ${url.protocol}`,
    );
  }
  // `url.search` and `url.hash` are empty both when a component is absent and
  // when it is present but empty (`https://host?`, `https://host#`), so the
  // raw value is checked for the delimiters. A valid origin never contains them.
  if (
    url.pathname !== "/" ||
    url.search !== "" ||
    url.hash !== "" ||
    raw.includes("?") ||
    raw.includes("#")
  ) {
    throw new Error(
      "PUBLIC_API_ORIGIN must be an origin without a path, query or fragment",
    );
  }
  return url.origin;
}
