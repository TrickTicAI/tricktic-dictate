@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.test.InstrumentationTestCase
import com.metrix.writer.sync.Planner
import com.metrix.writer.sync.StrictJson
import kotlinx.coroutines.runBlocking
import org.json.JSONArray
import org.json.JSONObject

/**
 * The Kotlin planner against every shared conflicts.json vector, seeded the way
 * packages/contracts/account-sync.firestore.fixtures.mjs seeds the JavaScript planner.
 */
class SyncPlannerTest : InstrumentationTestCase() {
    private fun accountDocument(authority: JSONObject, uid: String, category: String): JSONObject {
        val account = Planner.initialAccount(uid)
        account.put("account_epoch", authority.getLong("account_epoch")).put("account_state", authority.getString("account_state"))
        account.getJSONObject("consent").put("account_epoch", authority.getLong("account_epoch"))
        val toggle = JSONObject().put("enabled", authority.getBoolean("enabled")).put("generation", authority.getLong("generation"))
        if (category == "transcripts") toggle.put("history", if (authority.getBoolean("enabled"))
            JSONObject().put("mode", "future_only").put("enrollment_id", authority.getString("history_enrollment_id")) else JSONObject().put("mode", "off"))
        account.getJSONObject("consent").put(category, toggle)
        return account
    }

    private fun seed(initial: JSONObject, uid: String, category: String): MutableMap<String, JSONObject> {
        val documents = mutableMapOf(Planner.account(uid) to accountDocument(initial, uid, category))
        val records = initial.getJSONArray("records").let { array -> (0 until array.length()).map { array.getJSONObject(it) } }
        for (record in records) {
            documents[Planner.record(uid, record.getString("id"))] = StrictJson.copy(record).put("operation_id", "ffffffff-ffff-4fff-bfff-000000000000")
                .put("consent_generation", initial.getLong("generation"))
            val data = record.getJSONObject("content").optJSONObject("data")
            if (data?.getString("kind") == "original") {
                documents[Planner.attempt(uid, data.getString("attempt_id"))] = JSONObject().put("original_record_id", record.getString("id"))
                documents[Planner.family(uid, record.getString("id"))] = JSONObject().put("deleted", record.getJSONObject("content").getString("state") == "deleted")
                    .put("member_ids", JSONArray(records.filter { it.getJSONObject("content").optJSONObject("data")?.optString("original_record_id") == record.getString("id") }.map { it.getString("id") }))
            }
        }
        return documents
    }

    fun testPlannerMatchesEverySharedVector() = runBlocking {
        val scenarios = SyncFixtures.json(instrumentation, "conflicts.json") as JSONArray
        assertEquals(22, scenarios.length())
        for (i in 0 until scenarios.length()) {
            val scenario = scenarios.getJSONObject(i)
            val name = scenario.getString("name")
            val initial = scenario.getJSONObject("initial")
            val steps = scenario.getJSONArray("steps")
            val first = steps.getJSONObject(0).getJSONObject("operation")
            val uid = first.getString("uid")
            val category = first.getString("category")
            val documents = seed(initial, uid, category)
            val authority = StrictJson.copy(initial)
            for (s in 0 until steps.length()) {
                val step = steps.getJSONObject(s)
                step.optJSONObject("authority_patch")?.let { patch ->
                    patch.keys().forEach { authority.put(it, patch.get(it)) }
                    documents[Planner.account(uid)] = accountDocument(authority, uid, category)
                }
                val authUid = initial.opt("uid") as? String
                val plan = Planner.plan(authUid, step.getJSONObject("operation"), initial.getBoolean("history_enrolled")) { path -> documents[path]?.let(StrictJson::copy) }
                val expected = step.getJSONObject("expected")
                assertEquals("$name step $s", expected.getString("status"), plan.status)
                if (expected.has("revision")) assertEquals("$name step $s revision", expected.getLong("revision"), plan.revision)
                if (plan.status != "acknowledged") assertTrue("$name step $s writes nothing", plan.writes.isEmpty())
                plan.writes.forEach { documents[it.path] = it.data }
            }
            if (name == "delete-original-fences-descendants") {
                val live = documents.filterKeys { it.startsWith("users/$uid/records/") }.values.filter { it.getJSONObject("content").getString("state") == "live" }
                assertTrue("every family member is tombstoned", live.isEmpty())
            }
        }
    }

    fun testConsentChangesAdvanceGenerationsByOne() {
        val account = Planner.initialAccount("synthetic-user-a").getJSONObject("consent")
        val on = Planner.nextConsent(account, "dictionary", true)
        assertEquals(2L, on.getLong("revision")); assertEquals(2L, on.getJSONObject("dictionary").getLong("generation"))
        assertEquals(1L, on.getJSONObject("settings").getLong("generation"))
        assertEquals(2L, Planner.nextConsent(on, "dictionary", true).getJSONObject("dictionary").getLong("generation"))
        assertEquals(3L, Planner.nextConsent(on, "dictionary", false).getJSONObject("dictionary").getLong("generation"))
    }

    fun testPathsRefuseUidsAndIdsThatEscapeTheUserRoot() {
        for (uid in listOf("", "a/b", "..", ".", "__admin__", "x".repeat(129))) {
            try { Planner.account(uid); fail(uid) } catch (_: IllegalArgumentException) {}
        }
        try { Planner.record("synthetic-user-a", "../receipts/x"); fail() } catch (_: IllegalArgumentException) {}
        assertEquals("users/synthetic-user-a/records/00000000-0000-4000-8000-000000000001", Planner.record("synthetic-user-a", "00000000-0000-4000-8000-000000000001"))
    }
}
