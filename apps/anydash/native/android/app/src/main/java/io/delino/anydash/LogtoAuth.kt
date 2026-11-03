// SPDX-License-Identifier: Apache-2.0
package io.delino.anydash

import android.net.Uri
import android.util.Base64
import org.json.JSONObject
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLEncoder
import java.security.MessageDigest
import java.security.SecureRandom

/** RFC 7636 proof values; pure so JVM tests cover them. */
internal object Pkce {
    private val random = SecureRandom()
    fun base64Url(bytes: ByteArray): String =
        java.util.Base64.getUrlEncoder().withoutPadding().encodeToString(bytes)
    fun verifier(): String = base64Url(ByteArray(32).also(random::nextBytes))
    fun state(): String = base64Url(ByteArray(16).also(random::nextBytes))
    fun challenge(verifier: String): String =
        base64Url(MessageDigest.getInstance("SHA-256").digest(verifier.toByteArray(Charsets.US_ASCII)))

    /** Authorization URL for both API resources: AnyDash for API calls, Delcore for spaces. */
    fun authorizationUrl(endpoint: String, appId: String, redirect: String, challenge: String, state: String,
                         resources: List<String>): String {
        val query = listOf(
            "client_id" to appId,
            "redirect_uri" to redirect,
            "response_type" to "code",
            "scope" to "openid offline_access profile",
            "code_challenge" to challenge,
            "code_challenge_method" to "S256",
            "state" to state,
            "prompt" to "consent",
        ) + resources.map { "resource" to it }
        return "$endpoint/oidc/auth?" + form(query)
    }

    fun form(values: List<Pair<String, String>>) =
        values.joinToString("&") { (key, value) -> "${encode(key)}=${encode(value)}" }
    private fun encode(value: String) = URLEncoder.encode(value, "UTF-8")

    /** The `sub` claim of an ID token, without verifying it; used only for the local cache key. */
    fun subject(idToken: String?): String? {
        val parts = idToken?.split('.') ?: return null
        if (parts.size != 3) return null
        return try {
            JSONObject(String(java.util.Base64.getUrlDecoder().decode(parts[1]), Charsets.UTF_8)).optString("sub").ifEmpty { null }
        } catch (_: Exception) { null }
    }
}

class AuthException(message: String) : Exception(message)

/**
 * Logto authorization code flow with PKCE. The refresh token stays in the
 * Keystore-encrypted session store; access tokens stay in memory.
 */
internal class LogtoAuth(private val store: SessionStore) {
    /** Guards `tokens`, `session` and session-store writes that depend on the current session. */
    private val tokens = mutableMapOf<String, Pair<String, Long>>()
    /**
     * Session generation. Sign-in and sign-out advance it, so a token request that
     * started under an earlier session can neither store its rotated refresh token
     * nor cache or return its access token, and its failure cannot end a newer session.
     */
    private var session = 0L
    val configured get() = BuildConfig.LOGTO_ENDPOINT.isNotEmpty() && BuildConfig.LOGTO_APP_ID.isNotEmpty()
    val signedIn get() = store.get(SessionStore.SESSION)?.optString("refreshToken")?.isNotEmpty() == true
    val userKey: String? get() = store.get(SessionStore.SESSION)?.optString("userKey")?.ifEmpty { null }

    fun startSignIn(): String {
        if (!configured) throw AuthException("Sign-in is not configured for this build.")
        val verifier = Pkce.verifier()
        val state = Pkce.state()
        store.put(SessionStore.PENDING, JSONObject().put("verifier", verifier).put("state", state)
            .put("createdAt", System.currentTimeMillis()))
        return Pkce.authorizationUrl(BuildConfig.LOGTO_ENDPOINT, BuildConfig.LOGTO_APP_ID, REDIRECT,
            Pkce.challenge(verifier), state, listOf(BuildConfig.ANYDASH_RESOURCE, BuildConfig.DELCORE_RESOURCE))
    }

    fun hasPendingSignIn() = store.get(SessionStore.PENDING) != null
    fun cancelSignIn() = store.remove(SessionStore.PENDING)

    /** Exchanges the callback code after checking state and age (10 minutes). */
    fun completeSignIn(callback: Uri) {
        val pending = store.get(SessionStore.PENDING) ?: throw AuthException("Sign-in was cancelled.")
        val generation = synchronized(tokens) { session }
        store.remove(SessionStore.PENDING)
        val state = callback.getQueryParameter("state").orEmpty()
        val code = callback.getQueryParameter("code")
        val age = System.currentTimeMillis() - pending.getLong("createdAt")
        if (code.isNullOrEmpty() || age !in 0..600_000 ||
            !MessageDigest.isEqual(state.toByteArray(), pending.getString("state").toByteArray())
        ) throw AuthException("Sign-in was cancelled.")
        val body = token(listOf(
            "grant_type" to "authorization_code",
            "code" to code,
            "redirect_uri" to REDIRECT,
            "code_verifier" to pending.getString("verifier"),
        ), generation)
        val refresh = body.optString("refresh_token").ifEmpty { throw AuthException("Sign-in returned no refresh token.") }
        val subject = Pkce.subject(body.optString("id_token"))
        synchronized(tokens) {
            session += 1
            tokens.clear()
            store.put(SessionStore.SESSION, JSONObject().put("refreshToken", refresh)
                .put("userKey", subject?.let { sha256Hex("anydash:$it") } ?: ""))
        }
    }

    /** A cached or refreshed access token for one API resource. */
    fun accessToken(resource: String): String {
        val (generation, stored) = synchronized(tokens) {
            tokens[resource]?.let { (token, expiresAt) -> if (expiresAt - 60_000 > System.currentTimeMillis()) return token }
            session to store.get(SessionStore.SESSION)
        }
        val current = stored ?: throw AuthException("Sign in to AnyDash.")
        val body = token(listOf(
            "grant_type" to "refresh_token",
            "refresh_token" to current.getString("refreshToken"),
            "resource" to resource,
        ), generation)
        val token = body.optString("access_token").ifEmpty { throw AuthException("Sign in to AnyDash.") }
        val expiresAt = System.currentTimeMillis() + body.optLong("expires_in", 3600) * 1000
        synchronized(tokens) {
            // The session ended or was replaced while the request was in flight.
            if (generation != session) {
                AnyDashLog.info("anydash_token_discarded")
                throw AuthException("Sign in to AnyDash.")
            }
            body.optString("refresh_token").takeIf { it.isNotEmpty() }?.let {
                store.put(SessionStore.SESSION, current.put("refreshToken", it))
            }
            tokens[resource] = token to expiresAt
        }
        return token
    }

    fun signOut() {
        synchronized(tokens) {
            session += 1
            tokens.clear()
            store.clear()
        }
    }

    /** Ends the session only while `generation` is still the current one. */
    private fun signOutIfCurrent(generation: Long) {
        synchronized(tokens) { if (generation == session) signOut() }
    }

    private fun token(params: List<Pair<String, String>>, generation: Long): JSONObject {
        val connection = URL("${BuildConfig.LOGTO_ENDPOINT}/oidc/token").openConnection() as HttpURLConnection
        try {
            connection.requestMethod = "POST"
            connection.connectTimeout = 15_000
            connection.readTimeout = 15_000
            connection.doOutput = true
            connection.setRequestProperty("Content-Type", "application/x-www-form-urlencoded")
            connection.outputStream.use {
                it.write(Pkce.form(listOf("client_id" to BuildConfig.LOGTO_APP_ID) + params).toByteArray())
            }
            val status = connection.responseCode
            if (status != 200) {
                AnyDashLog.error("anydash_token_failed", "status" to status)
                // An invalid grant ends the session; transient failures keep it.
                if (status == 400) signOutIfCurrent(generation)
                throw AuthException("Sign-in expired. Sign in again.")
            }
            return JSONObject(connection.inputStream.use { it.readBytes() }.toString(Charsets.UTF_8))
        } catch (error: IOException) {
            AnyDashLog.error("anydash_token_unreachable")
            throw AuthException("The network request failed.")
        } finally {
            connection.disconnect()
        }
    }

    companion object {
        const val REDIRECT = "anydash://auth/callback"
    }
}

internal fun base64(bytes: ByteArray): String = Base64.encodeToString(bytes, Base64.NO_WRAP)
internal fun unbase64(text: String): ByteArray = Base64.decode(text, Base64.DEFAULT)
