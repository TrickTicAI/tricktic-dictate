package com.metrix.writer

import android.test.InstrumentationTestCase
import com.metrix.writer.DeliveryStore.Companion.markOutcome
import java.io.File
import java.util.UUID

/** Ledger outcomes, legacy-record compatibility and the copy-only recovery path. */
class DeliveryStoreTest : InstrumentationTestCase() {
    private lateinit var directory: File
    private lateinit var store: DeliveryStore
    override fun setUp() {
        super.setUp()
        directory = File(instrumentation.targetContext.cacheDir, "delivery-store-test-${UUID.randomUUID()}")
        store = DeliveryStore(directory)
    }
    override fun tearDown() { directory.deleteRecursively(); super.tearDown() }
    private fun created(): String = UUID.randomUUID().toString().also { store.create(UUID.randomUUID().toString(), it, false) }

    fun testLegacyRecordsDeriveOutcomeFromState() {
        val expected = mapOf("delivered" to DeliveryOutcome.INSERTED_VERIFIED, "unconfirmed" to DeliveryOutcome.UNCONFIRMED,
            "recovery" to DeliveryOutcome.SAVED_NOT_INSERTED, "awaiting_result" to DeliveryOutcome.AWAITING)
        for ((state, outcome) in expected) {
            val token = created()
            // Records written before outcome/reason_code existed carry only state and free-text reason.
            store.save(store.load(token).put("state", state).put("reason", "legacy"))
            assertEquals(state, outcome, DeliveryOutcome.of(store.load(token)))
            assertEquals("", store.load(token).optString("reason_code"))
        }
    }

    fun testMarkOutcomeKeepsLegacyStateForExistingReaders() {
        val expected = mapOf(DeliveryOutcome.INSERTED_VERIFIED to "delivered", DeliveryOutcome.UNCONFIRMED to "unconfirmed",
            DeliveryOutcome.SAVED_NOT_INSERTED to "recovery", DeliveryOutcome.UNSUPPORTED to "recovery")
        for ((outcome, state) in expected) {
            val token = created()
            store.save(store.load(token).put("text", "saved").markOutcome(outcome, "code_${outcome.code}", "Reason"))
            val item = store.load(token)
            assertEquals(state, item.getString("state")); assertEquals(outcome, DeliveryOutcome.of(item))
            assertEquals("code_${outcome.code}", item.getString("reason_code"))
        }
    }

    fun testRecoveryCopiesWithoutChangingOutcomeOrIssuingAnEdit() {
        val token = created()
        assertNull("Nothing saved yet", store.recover(token))
        store.save(store.load(token).put("text", "Uncertain שלום").markOutcome(DeliveryOutcome.UNCONFIRMED, "readback_mismatch", "Check"))
        assertEquals("Uncertain שלום", store.recover(token))
        val item = store.load(token)
        assertEquals(DeliveryOutcome.UNCONFIRMED, DeliveryOutcome.of(item)); assertEquals("readback_mismatch", item.getString("reason_code"))
        assertTrue(item.getLong("recovered_at_ms") > 0)
    }
}
