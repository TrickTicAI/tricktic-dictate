package com.metrix.writer

/** Test-only diagnostics/reference; no recording-sized arrays or range lists in production. */
internal fun SilenceNative.quietFrames(audio: String, ceiling: Double): ByteArray? {
    val output = java.io.ByteArrayOutputStream()
    return if (scan(audio, ceiling, SilenceBatch { flags, count -> output.write(flags, 0, count); true })) output.toByteArray() else null
}

internal fun SilencePreparation.removed(flags: ByteArray): List<LongRange> {
    val frame = 640L
    val pad = 16_000L
    val result = mutableListOf<LongRange>()
    var start = -1
    fun close(end: Int) {
        if (start >= 0 && end - start >= 250)
            result += (start * frame + pad) until (end * frame - pad)
        start = -1
    }
    flags.forEachIndexed { i, value -> if (value.toInt() == 1) { if (start < 0) start = i } else close(i) }
    close(flags.size)
    return result
}
