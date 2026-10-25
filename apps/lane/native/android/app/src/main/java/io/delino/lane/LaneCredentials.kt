// SPDX-License-Identifier: Apache-2.0
package io.delino.lane

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import org.json.JSONObject
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** Secrets are encrypted at rest with a nonexportable installation key; backups are disabled. */
internal class LaneCredentials(context: Context) {
    private val preferences = context.getSharedPreferences("lane-credentials", Context.MODE_PRIVATE)
    private val key: SecretKey by lazy {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(ALIAS, null) as? SecretKey) ?: KeyGenerator.getInstance(
            KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply {
            init(KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).build())
        }.generateKey()
    }
    @Synchronized fun put(name: String, value: JSONObject) {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.ENCRYPT_MODE, key) }
        cipher.updateAAD(name.toByteArray(Charsets.UTF_8))
        val encrypted = cipher.doFinal(value.toString().toByteArray(Charsets.UTF_8))
        val encoded = Base64.encodeToString(cipher.iv + encrypted, Base64.NO_WRAP)
        check(preferences.edit().putString(name, encoded).commit())
    }
    @Synchronized fun get(name: String): JSONObject? {
        val encoded = preferences.getString(name, null) ?: return null
        val bytes = Base64.decode(encoded, Base64.NO_WRAP)
        require(bytes.size >= 28)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply {
            init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, bytes.copyOfRange(0, 12)))
        }
        cipher.updateAAD(name.toByteArray(Charsets.UTF_8))
        return JSONObject(cipher.doFinal(bytes.copyOfRange(12, bytes.size)).toString(Charsets.UTF_8))
    }
    @Synchronized fun remove(name: String) { check(preferences.edit().remove(name).commit()) }
    @Synchronized fun spaces(): Set<String> = preferences.all.keys.filter { LaneLinks.validId(it) }.toSet()
    companion object { private const val ALIAS = "io.delino.lane.credentials.v1" }
}
