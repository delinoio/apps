// SPDX-License-Identifier: Apache-2.0
package io.delino.anydash

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import android.util.Log
import org.json.JSONObject
import java.security.KeyStore
import java.security.MessageDigest
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** Structured logs with safe metadata only: never tokens, provider data or usage values. */
internal object AnyDashLog {
    fun info(event: String, vararg fields: Pair<String, Any?>) = Log.i("AnyDash", json(event, fields))
    fun error(event: String, vararg fields: Pair<String, Any?>) = Log.e("AnyDash", json(event, fields))
    private fun json(event: String, fields: Array<out Pair<String, Any?>>) =
        JSONObject().apply {
            put("event", event)
            put("platform", "android")
            for ((key, value) in fields) put(key, value ?: JSONObject.NULL)
        }.toString()
}

internal fun sha256Hex(text: String): String =
    MessageDigest.getInstance("SHA-256").digest(text.toByteArray(Charsets.UTF_8)).joinToString("") { "%02x".format(it) }

/**
 * Session secrets (the Logto refresh token, the per-user cache key and a
 * pending sign-in) encrypted at rest with a nonexportable Android Keystore key.
 * Backups are disabled in the manifest. The widget provider runs in the same
 * app process and reads the same store.
 */
internal class SessionStore(context: Context) {
    private val preferences = context.getSharedPreferences("anydash-session", Context.MODE_PRIVATE)
    private val key: SecretKey by lazy {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(ALIAS, null) as? SecretKey) ?: KeyGenerator.getInstance(
            KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore",
        ).apply {
            init(
                KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                    .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                    .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                    .build(),
            )
        }.generateKey()
    }

    @Synchronized fun put(name: String, value: JSONObject) {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.ENCRYPT_MODE, key) }
        cipher.updateAAD(name.toByteArray(Charsets.UTF_8))
        val encrypted = cipher.doFinal(value.toString().toByteArray(Charsets.UTF_8))
        check(preferences.edit().putString(name, Base64.encodeToString(cipher.iv + encrypted, Base64.NO_WRAP)).commit())
    }

    @Synchronized fun get(name: String): JSONObject? {
        val encoded = preferences.getString(name, null) ?: return null
        return try {
            val bytes = Base64.decode(encoded, Base64.NO_WRAP)
            require(bytes.size >= 28)
            val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply {
                init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, bytes.copyOfRange(0, 12)))
            }
            cipher.updateAAD(name.toByteArray(Charsets.UTF_8))
            JSONObject(cipher.doFinal(bytes.copyOfRange(12, bytes.size)).toString(Charsets.UTF_8))
        } catch (_: Exception) {
            // A lost Keystore key (for example after a device restore) ends the session.
            AnyDashLog.error("anydash_session_unreadable")
            remove(name)
            null
        }
    }

    @Synchronized fun remove(name: String) { check(preferences.edit().remove(name).commit()) }
    @Synchronized fun clear() { check(preferences.edit().clear().commit()) }

    companion object {
        private const val ALIAS = "io.delino.anydash.session.v1"
        const val SESSION = "session"
        const val PENDING = "pending-sign-in"
    }
}
