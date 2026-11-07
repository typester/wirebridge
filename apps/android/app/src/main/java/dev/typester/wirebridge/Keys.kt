package dev.typester.wirebridge

import android.content.Context
import uniffi.wirebridge_core.generatePrivateKey
import uniffi.wirebridge_core.publicKeyFromPrivate

/**
 * This phone's own key pair, made the first time it is needed and kept in the app's private storage. Its private key
 * never leaves the phone. Each client's key pair is the client's own ([Clients]).
 */
object Keys {
    private const val PREFS = "keys"
    private const val PHONE_PRIVATE_KEY = "phonePrivateKey"

    fun phonePrivateKey(context: Context): String = key(context, PHONE_PRIVATE_KEY)

    fun phonePublicKey(context: Context): String = publicKeyFromPrivate(phonePrivateKey(context))

    @Synchronized
    private fun key(context: Context, name: String): String {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        prefs.getString(name, null)?.let { return it }
        val key = generatePrivateKey()
        prefs.edit().putString(name, key).commit()
        return key
    }
}
