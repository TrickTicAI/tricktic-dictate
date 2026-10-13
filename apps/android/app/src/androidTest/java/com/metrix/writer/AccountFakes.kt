package com.metrix.writer

import android.content.Context
import com.metrix.writer.account.*
import kotlinx.coroutines.CompletableDeferred
import java.util.concurrent.CopyOnWriteArrayList

/** A scripted provider for the account session tests. Nothing reaches Google or Firebase. */
internal class FakeIdentity(override val projectId: String? = "metrix-writer", var restored: IdentityUser? = null) : GoogleIdentity {
    val calls = CopyOnWriteArrayList<String>()
    @Volatile var credential = CompletableDeferred<String>()
    @Volatile var exchange = CompletableDeferred<IdentityUser>()
    @Volatile var signedIn: IdentityUser? = restored
    @Volatile var refresh: () -> String = { "firebase-id-token" }
    /** `auth_time` of the next fresh token; defaults to a sign-in that just happened. */
    @Volatile var authTime: () -> Long = { System.currentTimeMillis() / 1000 }
    override fun restore(): IdentityUser? { calls += "restore"; return restored }
    override suspend fun googleIdToken(context: Context): String { calls += "credential"; return credential.await() }
    override suspend fun signIn(googleIdToken: String): IdentityUser {
        calls += "signIn"; return exchange.await().also { signedIn = it }
    }
    override suspend fun reauthenticate(googleIdToken: String, uid: String): IdentityUser {
        calls += "reauthenticate"; return exchange.await()
    }
    override suspend fun idToken(uid: String, forceRefresh: Boolean): String { calls += "idToken:$forceRefresh"; return refresh() }
    override suspend fun freshIdToken(uid: String): FreshToken { calls += "freshIdToken"; return FreshToken(refresh(), authTime()) }
    override fun signOutLocal() { calls += "signOutLocal"; signedIn = null }
    override suspend fun clearCredentialState() { calls += "clearCredentialState" }
}

/** Records every hook call as "<hook>:<uid>[:detail]" into [log], optionally failing I/O hooks. */
internal class FakeParticipant(
    private val log: MutableList<String>,
    var held: LocalCopySummary = LocalCopySummary.NONE,
    var failIo: Boolean = false,
    private val observe: () -> String = { "" },
) : AccountParticipant {
    override fun suspend(account: AccountKey) { log += "suspend:${account.uid}" }
    override fun localCopy(account: AccountKey) = held
    override fun keepLocalCopy(account: AccountKey) { log += "keep:${account.uid}${observe()}"; if (failIo) error("disk full") }
    override fun removeLocalCopy(account: AccountKey) { log += "remove:${account.uid}${observe()}"; if (failIo) error("disk full") }
    override fun applyFence(account: AccountKey, fence: CloudFence) { log += "fence:${account.uid}:${fence.category}:${fence.generation}:${fence.everywhere}" }
}
