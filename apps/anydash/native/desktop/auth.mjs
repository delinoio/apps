// SPDX-License-Identifier: Apache-2.0
// Logto sign-in for native hosts: authorization code flow with PKCE in the
// system browser, then refresh-token grants per API resource (AnyDash and
// Delcore). Only the refresh token persists, in the OS secure store.
import { createHash, randomBytes } from "node:crypto";

export const REDIRECT_URI = "anydash://auth/callback";
const ACCOUNT = "refresh-token";
const USER_KEY = "user-key";
/** Reads `sub` from an ID token the token endpoint just returned over TLS. */
export function subject(idToken) {
  try {
    return JSON.parse(
      Buffer.from(String(idToken).split(".")[1], "base64url").toString("utf8"),
    ).sub;
  } catch {
    return undefined;
  }
}
const base64url = (bytes) => Buffer.from(bytes).toString("base64url");

export function pkcePair(random = randomBytes) {
  const verifier = base64url(random(32));
  const challenge = base64url(createHash("sha256").update(verifier).digest());
  return { verifier, challenge };
}

const signedOut = () =>
  Object.assign(new Error("Sign in to AnyDash."), { code: "SIGNED_OUT" });

export class NativeAuth {
  #pending;
  #tokens = new Map();
  #refresh;
  // Incremented by sign-out and by each completed sign-in. Token requests and
  // secure-store reads started under an older session must not store or
  // return their results, so a late refresh never restores a signed-out user.
  #session = 0;
  // Secure-store writes run one at a time so a sign-out's removal always
  // lands after any earlier save; the store spawns separate processes.
  #writes = Promise.resolve();

  constructor({
    endpoint,
    appId,
    resources,
    store,
    fetcher = fetch,
    openBrowser,
    random = randomBytes,
    now = Date.now,
  }) {
    Object.assign(this, {
      endpoint,
      appId,
      resources,
      store,
      fetcher,
      openBrowser,
      random,
      now,
    });
  }

  get configured() {
    return !!(this.endpoint && this.appId);
  }

  async refreshToken() {
    if (this.#refresh) return this.#refresh;
    const session = this.#session;
    const stored = await this.store.get(ACCOUNT);
    if (session !== this.#session) return this.#refresh;
    this.#refresh ??= stored;
    return this.#refresh;
  }

  async signedIn() {
    return !!(await this.refreshToken());
  }

  /** Opaque per-user cache key; never a token. */
  async userKey() {
    return (await this.store.get(USER_KEY)) ?? "";
  }

  /** Opens the browser; resolves when the deep link callback completes. */
  signIn() {
    if (!this.configured)
      return Promise.reject(
        new Error("Sign-in is not configured for this build."),
      );
    const { verifier, challenge } = pkcePair(this.random);
    const state = base64url(this.random(16));
    const url = new URL("/oidc/auth", this.endpoint);
    url.searchParams.set("client_id", this.appId);
    url.searchParams.set("redirect_uri", REDIRECT_URI);
    url.searchParams.set("response_type", "code");
    url.searchParams.set("scope", "openid offline_access profile");
    url.searchParams.set("code_challenge", challenge);
    url.searchParams.set("code_challenge_method", "S256");
    url.searchParams.set("state", state);
    url.searchParams.set("prompt", "consent");
    for (const resource of this.resources)
      url.searchParams.append("resource", resource);
    this.#pending?.reject(new Error("A newer sign-in started."));
    return new Promise((resolve, reject) => {
      this.#pending = { state, verifier, resolve, reject };
      Promise.resolve(this.openBrowser(url.href)).catch(reject);
    });
  }

  /** Handles `anydash://auth/callback?code=…&state=…`. */
  async callback(raw) {
    const pending = this.#pending;
    const url = new URL(raw);
    if (!pending || url.searchParams.get("state") !== pending.state)
      return false;
    this.#pending = undefined;
    try {
      const code = url.searchParams.get("code");
      if (!code) throw new Error("Sign-in was cancelled.");
      const body = await this.#token({
        grant_type: "authorization_code",
        code,
        redirect_uri: REDIRECT_URI,
        code_verifier: pending.verifier,
      });
      if (!body.refresh_token)
        throw new Error("Sign-in returned no refresh token.");
      const session = ++this.#session;
      this.#tokens.clear();
      await this.#saveRefresh(body.refresh_token, session);
      const sub = subject(body.id_token);
      if (sub)
        await this.#serial(async () => {
          if (session === this.#session)
            await this.store.set(
              USER_KEY,
              createHash("sha256").update(`anydash:${sub}`).digest("hex"),
            );
        });
      pending.resolve();
    } catch (error) {
      pending.reject(error);
    }
    return true;
  }

  async #token(params) {
    const response = await this.fetcher(
      new URL("/oidc/token", this.endpoint).href,
      {
        method: "POST",
        headers: { "content-type": "application/x-www-form-urlencoded" },
        body: new URLSearchParams({
          client_id: this.appId,
          ...params,
        }).toString(),
      },
    );
    const body = await response.json().catch(() => ({}));
    if (!response.ok) {
      const error = new Error("Sign-in expired. Sign in again.");
      error.code = response.status === 400 ? "SESSION_EXPIRED" : "TOKEN_FAILED";
      throw error;
    }
    return body;
  }

  #serial(write) {
    const run = this.#writes.then(write);
    this.#writes = run.catch(() => {});
    return run;
  }

  /** Saves a refresh token only while `session` is still the current one. */
  #saveRefresh(token, session) {
    return this.#serial(async () => {
      if (session !== this.#session) return;
      this.#refresh = token;
      await this.store.set(ACCOUNT, token);
    });
  }

  /** Access token for one API resource, refreshed shortly before expiry. */
  async accessToken(resource) {
    const cached = this.#tokens.get(resource);
    if (cached && cached.expiresAt - 60_000 > this.now()) return cached.token;
    const session = this.#session;
    const refresh = await this.refreshToken();
    if (!refresh || session !== this.#session) throw signedOut();
    let body;
    try {
      body = await this.#token({
        grant_type: "refresh_token",
        refresh_token: refresh,
        resource,
      });
    } catch (error) {
      if (session !== this.#session) throw signedOut();
      if (error.code === "SESSION_EXPIRED") await this.signOut();
      throw error;
    }
    // The session ended or was replaced while the request was in flight.
    if (session !== this.#session) throw signedOut();
    if (body.refresh_token)
      await this.#saveRefresh(body.refresh_token, session);
    if (session !== this.#session) throw signedOut();
    this.#tokens.set(resource, {
      token: body.access_token,
      expiresAt: this.now() + Number(body.expires_in ?? 3600) * 1000,
    });
    return body.access_token;
  }

  async signOut() {
    this.#session += 1;
    this.#tokens.clear();
    this.#refresh = undefined;
    await this.#serial(async () => {
      await this.store.remove(ACCOUNT);
      await this.store.remove(USER_KEY);
    });
  }
}
