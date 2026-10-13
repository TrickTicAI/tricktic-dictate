package com.metrix.writer

import androidx.activity.compose.setContent
import androidx.compose.ui.graphics.asAndroidBitmap
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.metrix.writer.account.AccountSession
import com.metrix.writer.account.IdentityUser
import com.metrix.writer.sync.TranscriptSync
import com.metrix.writer.ui.Page
import com.metrix.writer.ui.TranscriptConsent
import com.metrix.writer.ui.WriterTheme
import kotlinx.coroutines.*
import org.junit.*
import org.junit.runner.RunWith
import java.io.File

@RunWith(AndroidJUnit4::class)
class TranscriptConsentUiTest {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

    @Test fun disclosureAndUncheckedConsentGateTheFirstUpload() {
        val context = compose.activity
        val sandbox = PortableSandbox(context)
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
        val cloud = FakeCloud()
        val session = AccountSession(SyncIdentity(IdentityUser("synthetic-consent", null, null)), scope)
        val engine = TranscriptSync(context, session, { cloud }, scope)
        try {
            compose.runOnUiThread { context.setContent { WriterTheme("light") { Page { TranscriptConsent(engine) } } } }
            compose.onNodeWithText("Firebase storage is encrypted in transit and at rest, but service administrators can read it. It is not end-to-end encrypted.").performScrollTo().assertIsDisplayed()
            compose.onNodeWithTag("transcripts-consent").assertIsOff()
            compose.onNodeWithTag("transcripts-enable").performScrollTo().assertIsNotEnabled()
            Assert.assertTrue(cloud.documents.isEmpty())
            compose.onNodeWithTag("transcripts-consent").performScrollTo().performClick()
            compose.onNodeWithTag("transcripts-enable").performScrollTo().assertIsEnabled()
            Assert.assertTrue("checking consent alone uploads nothing", cloud.documents.isEmpty())
            File(context.cacheDir, "transcript-consent.png").outputStream().use {
                compose.onRoot().captureToImage().asAndroidBitmap().compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it)
            }
            compose.onNodeWithTag("transcripts-enable").performClick()
            compose.waitUntil(5000) { engine.status.value.enabled }
            Assert.assertTrue(cloud.consent("synthetic-consent").getJSONObject("transcripts").getBoolean("enabled"))
            Assert.assertEquals("future_only", cloud.consent("synthetic-consent").getJSONObject("transcripts").getJSONObject("history").getString("mode"))
        } finally { scope.cancel(); sandbox.restore() }
    }
}
