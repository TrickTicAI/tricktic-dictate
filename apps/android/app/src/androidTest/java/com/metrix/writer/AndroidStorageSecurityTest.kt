@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.content.pm.ApplicationInfo
import android.security.NetworkSecurityPolicy
import android.test.AndroidTestCase
import org.xmlpull.v1.XmlPullParser
import java.io.File
import java.security.KeyStore
import java.util.UUID

class AndroidStorageSecurityTest : AndroidTestCase() {
    fun testPackagedBackupAndTransferRulesExcludePrivateStores() {
        assertEquals(0, context.applicationInfo.flags and ApplicationInfo.FLAG_ALLOW_BACKUP)
        val rules = context.resources.getXml(R.xml.data_extraction_rules)
        val excluded = mutableMapOf<String, MutableSet<String>>()
        var section = ""
        rules.use {
            while (it.eventType != XmlPullParser.END_DOCUMENT) {
                if (it.eventType == XmlPullParser.START_TAG) when (it.name) {
                    "cloud-backup", "device-transfer" -> section = it.name
                    "exclude" -> {
                        assertEquals(".", it.getAttributeValue(null, "path"))
                        excluded.getOrPut(section) { mutableSetOf() }.add(it.getAttributeValue(null, "domain"))
                    }
                }
                it.next()
            }
        }
        for (kind in listOf("cloud-backup", "device-transfer"))
            assertEquals(kind, setOf("root", "file", "database", "sharedpref", "external"), excluded[kind])
    }
    fun testNetworkPolicyHasNoReleaseCleartextOrTestTrust() {
        val policy = NetworkSecurityPolicy.getInstance()
        assertFalse(policy.isCleartextTrafficPermitted("generativelanguage.googleapis.com"))
        assertFalse(policy.isCleartextTrafficPermitted("api.openai.com"))
        if (!BuildConfig.DEBUG) {
            assertFalse(policy.isCleartextTrafficPermitted("10.0.2.2"))
            assertEquals(0, context.resources.getIdentifier("writer_test_ca", "raw", context.packageName))
            context.resources.getXml(R.xml.network_security_config).use {
                val tags = mutableListOf<String>()
                while (it.eventType != XmlPullParser.END_DOCUMENT) {
                    if (it.eventType == XmlPullParser.START_TAG) {
                        tags.add(it.name)
                        if (it.name == "base-config") assertEquals("false", it.getAttributeValue(null, "cleartextTrafficPermitted"))
                    }
                    it.next()
                }
                assertEquals(listOf("network-security-config", "base-config"), tags)
            }
        }
    }
    fun testKeystoreLossAndCiphertextTamperingRequireExplicitNewCredential() {
        assertTrue("Never delete a real phone key", android.os.Build.HARDWARE.contains("ranchu") || android.os.Build.HARDWARE.contains("goldfish"))
        val cloud = DirectCloud(context, "openai")
        assertFalse("Use an unconfigured fixture app", cloud.configured())
        val directory = File(context.cacheDir, "key-failure-${UUID.randomUUID()}")
        val store = SessionStore(directory)
        val item = store.create("mixed")
        store.audio(item).appendBytes(ByteArray(64) { 17 }); store.finishCapture(item, null)
        item.getJSONArray("attempts").put(org.json.JSONObject().put("raw_text", "synthetic original"))
        store.save(item)
        val beforeAudio = store.audio(item).readBytes()
        val beforeItem = store.load(item.getString("id")).toString()
        val keys = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        try {
            cloud.saveKey("synthetic-key-never-contact-a-provider")
            val old = cloud.snapshot()
            val file = File(context.noBackupFilesDir, "openai-${old.getString("credential_id")}.credential")
            val original = file.readBytes()
            assertFalse(String(original).contains("synthetic-key"))
            file.writeBytes(original.copyOf().apply { this[lastIndex] = (this[lastIndex].toInt() xor 1).toByte() })
            fun unavailable() {
                try { cloud.key(old); fail("An unreadable credential was accepted") }
                catch (error: CloudFailure) {
                    assertTrue(error.message!!.contains("start a new saved-audio retry"))
                    assertFalse(error.message!!.contains("synthetic-key"))
                }
                assertTrue(beforeAudio.contentEquals(store.audio(item).readBytes()))
                assertEquals(beforeItem, store.load(item.getString("id")).toString())
            }
            unavailable()
            file.writeBytes(original)
            keys.deleteEntry("writer.direct.openai.v1")
            unavailable()
            cloud.saveKey("synthetic-replacement-never-contact-provider")
            assertEquals("synthetic-replacement-never-contact-provider", cloud.key(cloud.snapshot()))
            unavailable() // A replacement must never silently rebind an older request.
        } finally { cloud.forget(); directory.deleteRecursively() }
    }
}
