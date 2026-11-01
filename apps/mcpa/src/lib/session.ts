// SPDX-License-Identifier: Apache-2.0

export enum SessionStatus {
  SignedIn = "signed-in",
  SignedOut = "signed-out",
  MfaRequired = "mfa-required",
}

export type Session =
  | {
      status: SessionStatus.SignedIn;
      userId: string;
      csrfToken: string;
      expiresAt?: Date;
    }
  | { status: SessionStatus.SignedOut }
  | { status: SessionStatus.MfaRequired; reauthenticationUrl: string };

export class SessionRequestError extends Error {
  constructor(
    message: string,
    readonly httpStatus?: number,
  ) {
    super(message);
    this.name = "SessionRequestError";
  }
}

const RETURN_PATH_KEY = "mcpa.returnPath";

/**
 * The exact console URL sent as `return_to`. The API accepts only URLs listed
 * in `MCPA_WEB_RETURN_URLS`, without query or fragment, so the console always
 * returns to its root and restores the remembered page itself.
 */
export function consoleReturnUrl(origin = window.location.origin): string {
  return `${origin}/`;
}

export type SessionClient = {
  getSession(): Promise<Session>;
  refresh(csrfToken: string): Promise<void>;
  logout(csrfToken: string): Promise<void>;
  loginUrl(returnTo: string): string;
};

export const CSRF_HEADER = "X-Mcpa-CSRF";

/**
 * Talks to the API's browser-session endpoints. The session lives in an
 * HttpOnly cookie on the API origin, so every request includes credentials.
 */
export function createSessionClient(
  apiUrl: string,
  fetchImpl: typeof fetch = (...args) => fetch(...args),
): SessionClient {
  const post = async (path: string, csrfToken: string) => {
    const response = await fetchImpl(`${apiUrl}${path}`, {
      method: "POST",
      credentials: "include",
      headers: { [CSRF_HEADER]: csrfToken },
    });
    if (!response.ok) {
      throw new SessionRequestError(
        `Request to ${path} failed.`,
        response.status,
      );
    }
  };
  return {
    async getSession() {
      let response: Response;
      try {
        response = await fetchImpl(`${apiUrl}/auth/session`, {
          credentials: "include",
          headers: { Accept: "application/json" },
        });
      } catch {
        throw new SessionRequestError("Managed CPA could not be reached.");
      }
      const body = (await response.json().catch(() => ({}))) as Record<
        string,
        unknown
      >;
      if (response.status === 401) return { status: SessionStatus.SignedOut };
      if (response.status === 403 && body.error === "mfa_required") {
        return {
          status: SessionStatus.MfaRequired,
          reauthenticationUrl: String(body.reauthentication_url ?? ""),
        };
      }
      if (
        !response.ok ||
        typeof body.user_id !== "string" ||
        typeof body.csrf_token !== "string"
      ) {
        throw new SessionRequestError(
          "Your session could not be checked.",
          response.status,
        );
      }
      const expiresAt =
        typeof body.expires_at === "string"
          ? new Date(body.expires_at)
          : undefined;
      return {
        status: SessionStatus.SignedIn,
        userId: body.user_id,
        csrfToken: body.csrf_token,
        expiresAt:
          expiresAt && !Number.isNaN(expiresAt.getTime())
            ? expiresAt
            : undefined,
      };
    },
    refresh: (csrfToken) => post("/auth/refresh", csrfToken),
    logout: (csrfToken) => post("/auth/logout", csrfToken),
    loginUrl: (returnTo) =>
      `${apiUrl}/auth/login?return_to=${encodeURIComponent(returnTo)}`,
  };
}

/** Accepts only same-origin absolute paths, never protocol-relative URLs. */
export function safeReturnPath(value: string | null | undefined): string {
  if (!value || !value.startsWith("/") || value.startsWith("//")) return "/";
  if (value.startsWith("/\\")) return "/";
  return value;
}

export function rememberReturnPath(path: string, storage = sessionStorage) {
  try {
    storage.setItem(RETURN_PATH_KEY, safeReturnPath(path));
  } catch {
    // Storage can be unavailable in private modes; sign-in still works and
    // returns to the console home.
  }
}

export function takeReturnPath(storage = sessionStorage): string {
  try {
    const value = storage.getItem(RETURN_PATH_KEY);
    storage.removeItem(RETURN_PATH_KEY);
    return safeReturnPath(value);
  } catch {
    return "/";
  }
}
