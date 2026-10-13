package com.metrix.writer

import android.app.Instrumentation
import android.app.UiAutomation
import android.content.Context
import android.content.pm.ApplicationInfo
import android.os.Bundle
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.security.MessageDigest
import java.util.UUID

/** Separate same-signer test APK. Never delivered or exposed by the production APK. */
class UpgradeRunner : Instrumentation() {
    override fun onCreate(arguments: Bundle?) {
        super.onCreate(arguments)
        Thread {
            val result = Bundle()
            try {
                getUiAutomation(UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES)
                val app = targetContext
                check(app.packageName == "com.metrix.writer")
                check(app.applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE == 0)
                check(!android.security.NetworkSecurityPolicy.getInstance().isCleartextTrafficPermitted("10.0.2.2"))
                check(app.packageManager.getPackageInfo(app.packageName, 0).longVersionCode == arguments!!.getString("version")!!.toLong())
                val baseline = File(app.noBackupFilesDir, "upgrade-baseline.json")
                if (arguments.getString("phase") == "seed") {
                    check(!baseline.exists()) { "Upgrade fixture already exists" }
                    val store = SessionStore(File(app.filesDir, "recordings"))
                    check(store.restore().isEmpty()) { "Use an empty isolated release install" }
                    val item = store.create("mixed")
                    store.audio(item).appendBytes(ByteArray(32000) { (it % 101).toByte() })
                    store.finishCapture(item, null)
                    item.getJSONArray("attempts").put(JSONObject().put("id", UUID.randomUUID().toString())
                        .put("status", JSONObject().put("state", "succeeded").put("result", JSONObject().put("transcript", JSONObject().put("raw_text", "Upgrade fixture שלום English 🌍")))))
                    store.save(item)
                    store.prepare(item, ORIGIN, "gemini", "fixture-model", "mixed")
                    val client = MobileConnection(app)
                    client.saveCredential(ORIGIN, TOKEN); client.select(ORIGIN, "gemini/fixture-model", "mixed")
                    CompanionPreferences(app).apply {
                        saveDocument(JSONObject().put("hints", JSONArray().put(JSONObject().put("text", "שדרוג 🌍").put("enabled", true)))
                            .put("corrections", JSONArray()).put("snippets", JSONArray()))
                        save("off", "dictionary")
                    }
                    val ledger = DeliveryStore(File(app.filesDir, "deliveries"))
                    val token = UUID.randomUUID().toString()
                    ledger.create(item.getString("id"), token, false)
                    ledger.save(ledger.load(token).put("state", "recovery").put("text", "Saved fixture שלום"))
                    val cloud = DirectCloud(app)
                    cloud.saveKey(CLOUD_KEY); cloud.saveSelection(DirectCloud.DEFAULT_MODEL, "mixed")
                    val direct = store.create("mixed")
                    store.audio(direct).appendBytes(ByteArray(16000) { (it % 79).toByte() })
                    store.finishCapture(direct, null); cloud.prepare(store, direct)
                    val hashes = JSONObject()
                    for (directory in listOf(File(app.filesDir, "recordings"), File(app.filesDir, "deliveries"), app.noBackupFilesDir,
                        File(app.applicationInfo.dataDir, "shared_prefs"))) {
                        for (file in directory.listFiles().orEmpty().filter { it.isFile }) {
                            // The updater's own bookkeeping legitimately changes while it performs
                            // the update under test; this baseline guards user-data retention.
                            if (file.name == "writer-updates.xml") continue
                            if (derivedHistoryFile(file.relativeTo(File(app.applicationInfo.dataDir)).path)) continue
                            hashes.put(file.relativeTo(File(app.applicationInfo.dataDir)).path, hash(file))
                        }
                    }
                    baseline.writeText(JSONObject().put("hashes", hashes).put("session", item.getString("id")).put("cloud", cloud.snapshot()).put("direct_session", direct.getString("id")).toString())
                } else if (arguments.getString("phase") == "in-app-update") {
                    val candidatePath = arguments.getString("candidate_apk") ?: error("Missing candidate_apk")
                    val candidateApk = File(candidatePath)
                    check(candidateApk.isFile) { "Candidate APK does not exist: $candidatePath" }
                    val targetVersion = arguments.getString("target_version") ?: error("Missing target_version")
                    val targetCode = arguments.getString("target_code")!!.toInt()
                    val targetSha256 = hash(candidateApk)

                    val info = UpdateInfo(
                        version = targetVersion,
                        versionCode = targetCode,
                        packageName = app.packageName,
                        signerSha256 = "e35e0ff5447132ced8c1228783a5f7cccf782af0f33af77e152f74e0a02f10fb",
                        // Staged mode never downloads; keep the metadata within the release URL contract.
                        url = "https://updates.tricktic.com/writer/releases/staged/writer.apk",
                        sha256 = targetSha256,
                        bytes = candidateApk.length(),
                        notes = "In-app self-update verification",
                        pubDate = "2026-09-15T12:00:00Z"
                    )

                    val updater = AndroidUpdater.getInstance(app)
                    val sessionId = updater.commitSession(info, candidateApk)
                    result.putInt("session_id", sessionId)
                    result.putString("result", "PASS")
                    finish(0, result)
                    return@Thread
                } else if (arguments.getString("phase") == "hosted-update") {
                    val feedUrl = arguments.getString("feed_url") ?: error("Missing feed_url")
                    val targetCode = arguments.getString("target_code")!!.toInt()
                    val updater = AndroidUpdater.getInstance(app)
                    updater.setFeedUrlOverride(feedUrl.takeUnless { it == AndroidUpdater.DEFAULT_FEED_URL })
                    updater.checkForUpdate(isAutomatic = false)
                    val deadline = System.currentTimeMillis() + 180000
                    var downloadStarted = false
                    var downloaded: File? = null
                    var info: UpdateInfo? = null
                    while (System.currentTimeMillis() < deadline) {
                        when (val state = updater.state.value) {
                            is UpdateState.Available -> {
                                check(state.info.versionCode == targetCode) { "Feed offered ${state.info.versionCode}, expected $targetCode" }
                                if (!downloadStarted) {
                                    downloadStarted = true
                                    updater.downloadUpdate(state.info)
                                }
                            }
                            is UpdateState.Downloaded -> {
                                check(state.info.versionCode == targetCode) { "Downloaded ${state.info.versionCode}, expected $targetCode" }
                                info = state.info
                                downloaded = state.apkFile
                                break
                            }
                            is UpdateState.Error -> error("Hosted update failed: ${state.message}")
                            else -> {}
                        }
                        Thread.sleep(500)
                    }
                    val apk = downloaded ?: error("Timed out waiting for hosted download from $feedUrl")
                    val update = info ?: error("Missing update info after hosted download")
                    check(apk.isFile && apk.length() == update.bytes) { "Downloaded APK missing or truncated" }
                    val sessionId = updater.commitSession(update, apk)
                    result.putInt("session_id", sessionId)
                    result.putString("feed_url", feedUrl)
                    result.putString("downloaded_apk", apk.absolutePath)
                    result.putString("result", "PASS")
                    finish(0, result)
                    return@Thread
                } else if (arguments.getString("phase") == "download-only") {
                    val feedUrl = arguments.getString("feed_url") ?: error("Missing feed_url")
                    val targetCode = arguments.getString("target_code")!!.toInt()
                    val updater = AndroidUpdater.getInstance(app)
                    updater.setFeedUrlOverride(feedUrl.takeUnless { it == AndroidUpdater.DEFAULT_FEED_URL })
                    updater.checkForUpdate(isAutomatic = false)
                    val deadline = System.currentTimeMillis() + 180000
                    var downloadStarted = false
                    while (System.currentTimeMillis() < deadline) {
                        when (val state = updater.state.value) {
                            is UpdateState.Available -> {
                                check(state.info.versionCode == targetCode) { "Feed offered ${state.info.versionCode}, expected $targetCode" }
                                if (!downloadStarted) {
                                    downloadStarted = true
                                    updater.downloadUpdate(state.info)
                                }
                            }
                            is UpdateState.Downloaded -> {
                                check(state.info.versionCode == targetCode) { "Downloaded ${state.info.versionCode}, expected $targetCode" }
                                check(state.apkFile.isFile && state.apkFile.length() == state.info.bytes) { "Downloaded APK missing or truncated" }
                                result.putString("downloaded_apk", state.apkFile.absolutePath)
                                result.putString("result", "PASS")
                                finish(0, result)
                                return@Thread
                            }
                            is UpdateState.Error -> error("Hosted download failed: ${state.message}")
                            else -> {}
                        }
                        Thread.sleep(500)
                    }
                    error("Timed out waiting for hosted download from $feedUrl")
                } else if (arguments.getString("phase") == "download-cancel") {
                    val feedUrl = arguments.getString("feed_url") ?: error("Missing feed_url")
                    val updater = AndroidUpdater.getInstance(app)
                    updater.setFeedUrlOverride(feedUrl.takeUnless { it == AndroidUpdater.DEFAULT_FEED_URL })
                    updater.checkForUpdate(isAutomatic = false)
                    val deadline = System.currentTimeMillis() + 180000
                    var downloadStarted = false
                    while (System.currentTimeMillis() < deadline) {
                        when (val state = updater.state.value) {
                            is UpdateState.Available -> {
                                if (!downloadStarted) {
                                    downloadStarted = true
                                    updater.downloadUpdate(state.info)
                                    Thread.sleep(300)
                                    updater.cancelDownload()
                                }
                            }
                            is UpdateState.Downloaded -> {
                                // Won the race: download finished before the cancel landed.
                                result.putString("outcome", "downloaded")
                                result.putString("result", "PASS")
                                finish(0, result)
                                return@Thread
                            }
                            is UpdateState.Error -> error("Cancel probe failed: ${state.message}")
                            else -> {}
                        }
                        if (downloadStarted && updater.state.value is UpdateState.Available) {
                            result.putString("outcome", "cancelled")
                            result.putString("result", "PASS")
                            finish(0, result)
                            return@Thread
                        }
                        Thread.sleep(200)
                    }
                    error("Timed out waiting for cancel probe to settle")
                } else if (arguments.getString("phase") == "state") {
                    val updater = AndroidUpdater.getInstance(app)
                    val state = updater.state.value
                    result.putString("update_state", state.javaClass.simpleName)
                    result.putString("result", "PASS")
                    finish(0, result)
                    return@Thread
                } else {
                    val saved = JSONObject(baseline.readText())
                    val hashes = saved.getJSONObject("hashes")
                    // Older baselines hashed this derived SQLite projection too. Opening
                    // History can rewrite it without changing any authoritative sidecar.
                    val retainedPaths = hashes.keys().asSequence().filterNot(::derivedHistoryFile).toList()
                    for (path in retainedPaths) check(hash(File(app.applicationInfo.dataDir, path)) == hashes.getString(path)) { "Retained file changed: $path" }
                    val store = SessionStore(File(app.filesDir, "recordings"))
                    val restored = store.restore()
                    val indexed = store.page(limit = 100).records
                    check(indexed.map { it.getString("id") }.toSet() == restored.map { it.getString("id") }.toSet()) { "History index lost a retained recording" }
                    for (record in indexed) check(record.toString() == store.load(record.getString("id")).toString()) { "History index differs from retained sidecar" }
                    android.database.sqlite.SQLiteDatabase.openDatabase(File(app.filesDir, "recordings/history-index.sqlite3").path, null,
                        android.database.sqlite.SQLiteDatabase.OPEN_READONLY).use { db ->
                        db.rawQuery("PRAGMA quick_check", null).use { rows -> check(rows.moveToFirst() && rows.getString(0) == "ok") { "History index integrity failed" } }
                    }
                    val item = restored.single { it.getString("id") == saved.getString("session") }
                    check(item.has("pending") && item.getJSONArray("attempts").length() == 1)
                    check(CompanionPreferences(app).delivery() == "dictionary")
                    check(CompanionPreferences(app).vocabulary().getJSONArray("hints").getString(0) == "שדרוג 🌍")
                    val client = MobileConnection(app)
                    check(client.endpoint() == ORIGIN && client.selection() == "gemini/fixture-model" && client.language() == "mixed")
                    client.open(ORIGIN, "/api/v1/mobile/connection", "GET").apply {
                        check(getRequestProperty("Authorization") == "Bearer $TOKEN") { "Credential did not decrypt after upgrade" }
                        disconnect() // No network or real credential used.
                    }
                    val cloud = DirectCloud(app)
                    check(cloud.model() == DirectCloud.DEFAULT_MODEL && cloud.language() == "mixed")
                    val direct = store.load(saved.getString("direct_session"))
                    check(direct.has("pending")) { "Saved direct request is missing" }
                    // Historical fixtures save only the credential snapshot; newer
                    // pending requests also retain cleanup policy. The file hashes
                    // above still require the complete saved request to be unchanged.
                    val retainedCloud = direct.getJSONObject("pending_cloud")
                    val expectedCloud = saved.getJSONObject("cloud")
                    for (field in expectedCloud.keys()) check(retainedCloud.get(field) == expectedCloud.get(field)) {
                        "Direct credential snapshot changed: $field"
                    }
                    check(cloud.key(direct.getJSONObject("pending_cloud")) == CLOUD_KEY) { "Direct provider key did not survive upgrade" }
                    val expectedState = arguments.getString("expected_state") ?: "Idle"
                    val updaterState = AndroidUpdater.getInstance(app).state.value
                    check(updaterState.javaClass.simpleName == expectedState) { "Updater state ${updaterState.javaClass.simpleName}, expected $expectedState" }
                    result.putInt("retained_files", retainedPaths.size)
                    result.putInt("indexed_recordings", indexed.size)
                }
                check(!CaptureService.isActive()) { "Microphone restarted without a tap" }
                result.putString("result", "PASS")
                finish(0, result)
            } catch (error: Throwable) {
                // Assertions contain only fixed fixture/path labels, never captured app content.
                result.putString("result", "FAIL: ${error.javaClass.simpleName}: ${error.message}")
                finish(1, result)
            }
        }.start()
    }
    private fun hash(file: File) = MessageDigest.getInstance("SHA-256").digest(file.readBytes()).joinToString("") { "%02x".format(it) }
    private fun derivedHistoryFile(path: String) = path in setOf("files/recordings/history-index.sqlite3",
        "files/recordings/history-index.sqlite3-journal", "files/recordings/history-index.sqlite3-wal", "files/recordings/history-index.sqlite3-shm")
    companion object {
        private const val CLOUD_KEY = "writer-upgrade-fixture-key-not-a-real-secret-0001"
        private const val ORIGIN = "https://upgrade-fixture.invalid"
        private const val TOKEN = "mw1.00000000-0000-4000-8000-000000000062.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    }
}
