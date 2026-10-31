@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.AndroidTestCase
import java.io.File
import java.io.IOException
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.UUID

/** Fabricated parser inputs only; no speech, real model or private user data. */
class PhoneModelSecurityTest : AndroidTestCase() {
    fun testMalformedModelsRejectWithoutChangingInputs() {
        fun ints(vararg values: Int): ByteArray = ByteBuffer.allocate(values.size * 4)
            .order(ByteOrder.LITTLE_ENDIAN).apply { values.forEach { putInt(it) } }.array()
        val magic = ints(0x67676d6c)
        val params = intArrayOf(51865, 1500, 384, 6, 4, 448, 384, 6, 4, 80, 1)
        val header = magic + ints(*params)
        val filters = header + ints(80, 201) + ByteArray(80 * 201 * 4)
        val tensors = filters + ints(0)
        val tensorName = "encoder.positional_embedding".toByteArray()
        val cases = linkedMapOf(
            "empty" to byteArrayOf(), "bad-magic" to "bad model".toByteArray(),
            "magic-only" to magic, "partial-hparams" to header.copyOf(31),
            "missing-filters" to header,
            "overflow-filters" to (header + ints(Int.MAX_VALUE, Int.MAX_VALUE)),
            "truncated-filters" to (header + ints(80, 201)),
            "zero-heads" to (magic + ints(*params.copyOf().apply { this[3] = 0 })),
            "negative-vocabulary" to (filters + ints(-1)),
            "oversized-token" to (filters + ints(1, 1024 * 1024 + 1)),
            "truncated-token" to (filters + ints(1, 4) + "ab".toByteArray()),
            "missing-tensors" to tensors, "partial-tensor-header" to (tensors + byteArrayOf(1)),
            "negative-tensor-type" to (tensors + ints(2, 1, -1)),
            "huge-tensor-type" to (tensors + ints(2, 1, Int.MAX_VALUE)),
            "invalid-tensor-dimensions" to (tensors + ints(2, 1, 0, -1, 384)),
            "truncated-tensor-data" to (tensors + ints(2, tensorName.size, 0, 384, 1500) + tensorName + ByteArray(4)),
        )
        val root = File(context.cacheDir, "invalid-models-${UUID.randomUUID()}").apply { mkdirs() }
        val native = PhoneNative()
        try {
            for ((name, bytes) in cases) {
                val model = File(root, "$name.bin").apply { writeBytes(bytes) }
                try { native.load(model.path); fail("Malformed model accepted: $name") }
                catch (_: IOException) { /* Expected: loader errors cross JNI without killing the app. */ }
                finally { native.unload() }
                assertTrue("Input unchanged: $name", bytes.contentEquals(model.readBytes()))
            }
        } finally { native.unload(); root.deleteRecursively() }
    }
}
