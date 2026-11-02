// SPDX-License-Identifier: Apache-2.0
import {
  Code,
  ConnectError,
  type Interceptor,
  type Transport,
} from "@connectrpc/connect";
import { createConnectTransport } from "@connectrpc/connect-web";
import { CSRF_HEADER } from "./session";

export type SessionHooks = {
  /** The current session CSRF token. */
  csrfToken: () => string | undefined;
  /**
   * Refreshes the session after an authentication failure. Resolves true when
   * the session is valid again and the request may be retried.
   */
  recover: () => Promise<boolean>;
};

/**
 * Adds the session CSRF token to every Connect request. After an
 * authentication failure it refreshes the session and retries once; the API
 * rejects unauthenticated requests before any handler runs, so the retry
 * cannot repeat a mutation.
 */
export function sessionInterceptor(hooks: SessionHooks): Interceptor {
  return (next) => async (request) => {
    const send = () => {
      const token = hooks.csrfToken();
      if (token) request.header.set(CSRF_HEADER, token);
      else request.header.delete(CSRF_HEADER);
      return next(request);
    };
    try {
      return await send();
    } catch (error) {
      if (
        !(error instanceof ConnectError) ||
        error.code !== Code.Unauthenticated
      )
        throw error;
      if (!(await hooks.recover())) throw error;
      return send();
    }
  };
}

/**
 * Creates the management transport. The browser sends the API-origin session
 * cookie (`credentials: "include"`); the CSRF token comes from /auth/session.
 */
export function createMcpaTransport(
  apiUrl: string,
  hooks: SessionHooks,
  fetchImpl?: typeof fetch,
): Transport {
  return createConnectTransport({
    baseUrl: apiUrl,
    interceptors: [sessionInterceptor(hooks)],
    fetch: (input, init) =>
      (fetchImpl ?? globalThis.fetch)(input, {
        ...init,
        credentials: "include",
      }),
  });
}
