@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.content.ComponentName
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.test.AndroidTestCase
import com.google.firebase.FirebaseApp
import com.metrix.writer.account.FirebaseGoogleIdentity

/** Optional Google sign-in (MW-OUATH-4): what the dev build adds, and what a guest never starts. */
class AccountBoundaryTest : AndroidTestCase() {
    fun testGuestNeverStartsFirebase() {
        val prefs = context.getSharedPreferences("writer-account", 0)
        if (prefs.getBoolean("firebase_session", false)) return // A signed-in fixture; guest behavior is not observable.
        val identity = FirebaseGoogleIdentity(context)
        assertNull(identity.restore())
        assertTrue(FirebaseApp.getApps(context).none { it.name == "writer-account" })
        assertTrue("no default Firebase app is created at startup", FirebaseApp.getApps(context).isEmpty())
        identity.signOutLocal()
        assertTrue(FirebaseApp.getApps(context).isEmpty())
    }

    fun testSignInConfigurationIsCompleteOrAbsent() {
        val values = listOf(BuildConfig.FIREBASE_PROJECT_ID, BuildConfig.FIREBASE_APP_ID, BuildConfig.FIREBASE_API_KEY, BuildConfig.FIREBASE_WEB_CLIENT_ID)
        assertTrue(values.all { it.isEmpty() } || values.none { it.isEmpty() })
        if (values.none { it.isEmpty() }) {
            assertEquals("metrix-writer", BuildConfig.FIREBASE_PROJECT_ID)
            assertTrue(BuildConfig.FIREBASE_APP_ID.startsWith("1:575628797662:android:"))
            assertTrue(BuildConfig.FIREBASE_WEB_CLIENT_ID.endsWith(".apps.googleusercontent.com"))
            assertTrue(com.metrix.writer.ui.WriterAccounts.get(context).configured)
        }
    }

    fun testUnusedFirebaseEntrypointsAreRemoved() {
        val pm = context.packageManager
        for (name in listOf("com.google.firebase.auth.internal.GenericIdpActivity", "com.google.firebase.auth.internal.RecaptchaActivity")) {
            try { pm.getActivityInfo(ComponentName(context, name), 0); fail("$name must not be declared") } catch (_: PackageManager.NameNotFoundException) {}
        }
        assertNull(pm.resolveContentProvider("${context.packageName}.firebaseinitprovider", 0))
        for (scheme in listOf("genericidp", "recaptcha")) {
            val intent = Intent(Intent.ACTION_VIEW, Uri.parse("$scheme://firebase.auth/${context.packageName}")).setPackage(context.packageName)
            assertTrue(scheme, pm.queryIntentActivities(intent, 0).isEmpty())
        }
    }
}
