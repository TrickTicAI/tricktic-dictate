package com.metrix.writer

/** Constant false is folded out of release callers. No fixture text, state or insertion exists here. */
internal object EditorFixture {
    const val enabled = false
    @Suppress("UNUSED_PARAMETER") fun start(service: WriterInputService) = Unit
}
