package com.metrix.writer.account

import org.json.JSONObject
import java.io.File
import java.security.MessageDigest

/*
 * Account lifecycle hooks v1 (MW-OUATH-8, docs/testing/account-sync/account-lifecycle.md).
 * Sync adapters (MW-OUATH-7/10) register an [AccountParticipant]; they never handle sign-out
 * themselves. Everything they keep for an account lives in its [AccountWorkspace], never in
 * the guest stores, and unsynced originals stay in their original store.
 */

/** What happens to an account's downloaded copies when it signs out, switches or is deleted. */
enum class LocalCopy(val wire: String) { LEAVE("leave"), KEEP("keep"), REMOVE("remove") }

data class LocalCopySummary(val items: Int, val unsynced: Int) {
    val empty get() = items == 0 && unsynced == 0
    operator fun plus(other: LocalCopySummary) = LocalCopySummary(items + other.items, unsynced + other.unsynced)
    companion object { val NONE = LocalCopySummary(0, 0) }
}

/** A cloud fence for one category: a cleanup receipt, a `resets` entry or an account epoch change. */
data class CloudFence(val category: String, val generation: Long, val everywhere: Boolean)

interface AccountParticipant {
    /** Stop listeners and outbox dispatch for [account] now. Queued work stays in the workspace. No I/O. */
    fun suspend(account: AccountKey)
    fun localCopy(account: AccountKey): LocalCopySummary
    /** Copy portable content into the guest store without account metadata, consent or queued operations. */
    fun keepLocalCopy(account: AccountKey)
    /** Remove this UID's downloaded copies only. No cloud tombstones; never unsynced originals. */
    fun removeLocalCopy(account: AccountKey)
    /** Disable the category's sync; for [CloudFence.everywhere] also remove its downloaded copies. */
    fun applyFence(account: AccountKey, fence: CloudFence)
}

/** filesDir/accounts/<projectId>/<first 32 hex of sha256(uid)>/: the file system never shows the UID. */
class AccountWorkspace(private val filesDir: File) {
    fun directory(account: AccountKey): File {
        require(Regex("[a-z][a-z0-9-]{4,28}[a-z0-9]").matches(account.projectId)) { "invalid project" }
        val digest = MessageDigest.getInstance("SHA-256").digest(account.uid.toByteArray(Charsets.UTF_8))
        return File(filesDir, "accounts/${account.projectId}/${digest.joinToString("") { "%02x".format(it) }.take(32)}")
    }
}

/** The non-content fields of `users/{uid}` that account controls need. */
data class RemoteAccount(val epoch: Long, val state: String, val consentRevision: Long, val resets: Map<String, Long>) {
    companion object {
        /** An account that never turned on sync has no document: the all-off initial account. */
        val INITIAL = RemoteAccount(1, "active", 1, emptyMap())
        val CATEGORIES = listOf("settings", "dictionary", "transcripts")

        /** Parses a Firestore REST document. Throws on a shape this client does not understand. */
        fun fromFirestore(document: JSONObject): RemoteAccount {
            val fields = document.getJSONObject("fields")
            fun JSONObject.long(name: String) = getJSONObject(name).getString("integerValue").toLong()
            val consent = fields.getJSONObject("consent").getJSONObject("mapValue").getJSONObject("fields")
            val resets = fields.optJSONObject("resets")?.optJSONObject("mapValue")?.optJSONObject("fields")
            return RemoteAccount(
                fields.long("account_epoch"), fields.getJSONObject("account_state").getString("stringValue"), consent.long("revision"),
                CATEGORIES.mapNotNull { category ->
                    resets?.optJSONObject(category)?.getJSONObject("mapValue")?.getJSONObject("fields")?.let { category to it.long("generation") }
                }.toMap(),
            )
        }
    }

    /** `everywhere` resets this device has not reconciled yet; the sync adapter applies them on reconnect. */
    fun pendingResets(reconciled: Map<String, Long>): List<CloudFence> =
        resets.filter { (category, generation) -> generation > (reconciled[category] ?: 0) }.map { (category, generation) -> CloudFence(category, generation, true) }
}
