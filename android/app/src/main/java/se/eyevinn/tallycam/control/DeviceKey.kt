package se.eyevinn.tallycam.control

import android.content.pm.PackageManager
import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.security.keystore.StrongBoxUnavailableException
import android.util.Log
import se.eyevinn.tallycam.core.DeviceKeyIdentity
import se.eyevinn.tallycam.core.deviceKeyAuthMessage
import se.eyevinn.tallycam.core.deviceKeyAuthPayload
import se.eyevinn.tallycam.core.deviceKeyIdentity
import java.math.BigInteger
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.PrivateKey
import java.security.Signature
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec

/**
 * The phone's device key (docs/PROTOCOL.md, "Device keys"): a P-256 key in
 * the Android Keystore (StrongBox where the phone has one), never exported
 * and never backed up. The id, the bytes to sign and the `auth` message come
 * from the shared core, as on iOS. A reinstall creates a new key, and so a
 * new camera.
 */
class DeviceKey private constructor(
    val identity: DeviceKeyIdentity,
    private val privateKey: PrivateKey,
) {
    val deviceId: String get() = identity.deviceId

    /** The `auth` message (JSON text) answering a `challenge`. */
    fun authMessage(server: String, nonce: String): String {
        val payload = deviceKeyAuthPayload(server, nonce, identity.deviceId)
        val der = Signature.getInstance("SHA256withECDSA").run {
            initSign(privateKey)
            update(payload)
            sign()
        }
        return deviceKeyAuthMessage(derToRaw(der))
    }

    companion object {
        private const val TAG = "DeviceKey"
        private const val ALIAS = "tallycam-device-key-v1"

        @Volatile private var cached: DeviceKey? = null

        /** The stored key, created on first use; null if the Keystore fails. */
        fun current(context: Context): DeviceKey? {
            cached?.let { return it }
            return synchronized(this) {
                cached ?: runCatching { loadOrCreate(context) }
                    .onFailure { Log.e(TAG, "No device key", it) }
                    .getOrNull()
                    ?.also { cached = it }
            }
        }

        private fun loadOrCreate(context: Context): DeviceKey {
            val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
            if (!store.containsAlias(ALIAS)) {
                create(context)
            }
            val entry = store.getEntry(ALIAS, null) as KeyStore.PrivateKeyEntry
            val public = entry.certificate.publicKey as ECPublicKey
            return DeviceKey(deviceKeyIdentity(x963(public)), entry.privateKey)
        }

        private fun create(context: Context) {
            val strongBox = context.packageManager.hasSystemFeature(PackageManager.FEATURE_STRONGBOX_KEYSTORE)
            fun generate(withStrongBox: Boolean) {
                val spec = KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_SIGN)
                    .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
                    .setDigests(KeyProperties.DIGEST_SHA256)
                    .setIsStrongBoxBacked(withStrongBox)
                    .build()
                KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, "AndroidKeyStore").run {
                    initialize(spec)
                    generateKeyPair()
                }
            }
            try {
                generate(strongBox)
            } catch (e: StrongBoxUnavailableException) {
                generate(false)
            }
            Log.i(TAG, "Created device key${if (strongBox) " (StrongBox)" else ""}")
        }

        /** SEC1 uncompressed: 0x04, x and y as 32 bytes each. */
        fun x963(key: ECPublicKey): ByteArray =
            byteArrayOf(4) + fixed32(key.w.affineX) + fixed32(key.w.affineY)

        private fun fixed32(n: BigInteger): ByteArray {
            val b = n.toByteArray()
            return when {
                b.size == 32 -> b
                b.size > 32 -> b.copyOfRange(b.size - 32, b.size)
                else -> ByteArray(32 - b.size) + b
            }
        }

        /** An ECDSA signature from DER (SEQUENCE of two INTEGERs) to raw r || s. */
        fun derToRaw(der: ByteArray): ByteArray {
            var i = 2
            if (der[1].toInt() and 0x80 != 0) i += der[1].toInt() and 0x7F
            fun integer(): BigInteger {
                require(der[i].toInt() == 0x02) { "not a DER integer" }
                val len = der[i + 1].toInt() and 0xFF
                val value = BigInteger(1, der.copyOfRange(i + 2, i + 2 + len))
                i += 2 + len
                return value
            }
            val r = integer()
            val s = integer()
            return fixed32(r) + fixed32(s)
        }
    }
}
