package com.metrix.writer

import android.content.Context
import android.content.Intent
import android.content.SharedPreferences
import android.content.pm.PackageInfo
import android.content.pm.PackageInstaller
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.provider.Settings
import android.util.Log
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.net.HttpURLConnection
import java.io.ByteArrayOutputStream
import java.io.OutputStream
import java.util.UUID
import java.security.MessageDigest
import java.util.concurrent.atomic.AtomicBoolean

data class UpdateInfo(
    val version: String,
    val versionCode: Int,
    val packageName: String,
    val signerSha256: String,
    val url: String,
    val sha256: String,
    val bytes: Long,
    val notes: String,
    val pubDate: String
) {
    fun toJson(): JSONObject = JSONObject().apply {
        put("version", version)
        put("version_code", versionCode)
        put("package", packageName)
        put("signer_sha256", signerSha256)
        put("url", url)
        put("sha256", sha256)
        put("bytes", bytes)
        put("notes", notes)
        put("pub_date", pubDate)
    }

    companion object {
        fun fromJson(json: JSONObject): UpdateInfo = UpdateInfo(
            version = json.getString("version"),
            versionCode = json.getInt("version_code"),
            packageName = json.getString("package"),
            signerSha256 = json.getString("signer_sha256"),
            url = json.getString("url"),
            sha256 = json.getString("sha256"),
            bytes = json.getLong("bytes"),
            notes = json.optString("notes", ""),
            pubDate = json.optString("pub_date", "")
        )
    }
}

sealed class UpdateState {
    abstract val lastCheckedMs: Long?

    data class Idle(override val lastCheckedMs: Long? = null, val message: String? = null) : UpdateState()
    data class Checking(override val lastCheckedMs: Long? = null) : UpdateState()
    data class Available(val info: UpdateInfo, override val lastCheckedMs: Long) : UpdateState()
    data class Downloading(val info: UpdateInfo, val downloadedBytes: Long, val totalBytes: Long, val percent: Int, override val lastCheckedMs: Long? = null) : UpdateState()
    data class Downloaded(val info: UpdateInfo, val apkFile: File, override val lastCheckedMs: Long? = null) : UpdateState()
    data class Installing(val info: UpdateInfo, val sessionId: Int, val pendingUserAction: Boolean, override val lastCheckedMs: Long? = null) : UpdateState()
    data class Error(val message: String, val info: UpdateInfo? = null, val canRetry: Boolean = true, override val lastCheckedMs: Long? = null) : UpdateState()
}

class AndroidUpdater private constructor(val context: Context) {
    private val prefs: SharedPreferences = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
    private val scope = CoroutineScope(Dispatchers.Main + SupervisorJob())
    private val _state = MutableStateFlow<UpdateState>(UpdateState.Idle())
    val state: StateFlow<UpdateState> = _state.asStateFlow()

    private var currentDownloadJob: Job? = null
    private var checkJob: Job? = null
    private val downloadCancelled = AtomicBoolean(false)
    private val installInFlight = AtomicBoolean(false)
    private val updatesDir = File(context.filesDir, "updates")

    init {
        updatesDir.mkdirs()
        reconcileState()
    }

    private fun lastChecked(): Long? =
        if (prefs.contains(PREF_LAST_CHECKED_MS)) prefs.getLong(PREF_LAST_CHECKED_MS, 0L) else null

    fun reconcileState() {
        val lastChecked = lastChecked()
        val installer = context.packageManager.packageInstaller
        val savedId = prefs.getInt(PREF_INSTALL_SESSION, -1)
        // A kill between createSession and persisting its id must not leak staging storage.
        installer.mySessions.filter { it.appPackageName == EXPECTED_PACKAGE && it.sessionId != savedId }
            .forEach { runCatching { installer.abandonSession(it.sessionId) } }
        val savedJson = prefs.getString(PREF_AVAILABLE_UPDATE, null)
        val downloadedPath = prefs.getString(PREF_DOWNLOADED_APK_PATH, null)

        if (savedJson != null) {
            try {
                val info = parseAndValidateManifest(UpdateTransfer.manifestJson(savedJson))
                // If the app has been updated to or beyond this version, the update succeeded!
                if (BuildConfig.VERSION_CODE >= info.versionCode) {
                    clearInstallSession()
                    clearSavedUpdate()
                    cleanObsoleteFiles(null)
                    _state.value = UpdateState.Idle(lastChecked, "Updated to version ${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE})")
                    return
                }

                val activeId = prefs.getInt(PREF_INSTALL_SESSION, -1)
                val active = context.packageManager.packageInstaller.mySessions.firstOrNull { it.sessionId == activeId }
                if (active != null && active.appPackageName == EXPECTED_PACKAGE && active.isSealed) {
                    installInFlight.set(true)
                    _state.value = UpdateState.Installing(info, activeId, false, lastChecked)
                    return
                }
                if (active != null) context.packageManager.packageInstaller.abandonSession(activeId)
                clearInstallSession()

                // Check if downloaded APK exists and is valid
                if (downloadedPath != null) {
                    val apkFile = File(updatesDir, "writer-${info.versionCode}.apk")
                    if (apkFile.exists() && apkFile.length() == info.bytes) {
                        try {
                            verifyApkFile(apkFile, info)
                            cleanObsoleteFiles(apkFile)
                            _state.value = UpdateState.Downloaded(info, apkFile, lastChecked)
                            return
                        } catch (e: Exception) {
                            Log.w(TAG, "Cached APK failed verification: ${e.message}", e)
                            apkFile.delete()
                            prefs.edit().remove(PREF_DOWNLOADED_APK_PATH).apply()
                        }
                    }
                }

                cleanObsoleteFiles(null)
                prefs.edit().remove(PREF_DOWNLOADED_APK_PATH).apply()
                _state.value = UpdateState.Available(info, lastChecked ?: System.currentTimeMillis())
                return
            } catch (e: Exception) {
                Log.w(TAG, "Failed to parse saved update: ${e.message}", e)
                clearSavedUpdate()
            }
        }

        clearInstallSession()
        cleanObsoleteFiles(null)
        _state.value = UpdateState.Idle(lastChecked)
    }

    private fun cleanObsoleteFiles(keepFile: File?) {
        try {
            updatesDir.listFiles()?.forEach { file ->
                if (keepFile == null || file.absolutePath != keepFile.absolutePath) {
                    file.delete()
                }
            }
        } catch (e: Exception) {
            Log.w(TAG, "Failed to clean updates dir: ${e.message}")
        }
    }

    private fun clearSavedUpdate() {
        prefs.edit()
            .remove(PREF_AVAILABLE_UPDATE)
            .remove(PREF_DOWNLOADED_APK_PATH)
            .apply()
    }

    fun getFeedUrl(): String {
        if (!BuildConfig.DEBUG) return DEFAULT_FEED_URL
        return prefs.getString(PREF_FEED_URL_OVERRIDE, null)?.takeIf { it.isNotBlank() } ?: DEFAULT_FEED_URL
    }

    fun setFeedUrlOverride(url: String?) {
        if (url.isNullOrBlank()) {
            prefs.edit().remove(PREF_FEED_URL_OVERRIDE).apply()
        } else {
            check(BuildConfig.DEBUG) { "Feed overrides are debug-only" }
            UpdateTransfer.url(url, feed = true)
            prefs.edit().putString(PREF_FEED_URL_OVERRIDE, url).apply()
        }
    }

    fun checkOnLaunch() {
        val lastChecked = lastChecked()
        val now = System.currentTimeMillis()
        if (lastChecked == null || (now - lastChecked) > AUTO_CHECK_INTERVAL_MS) {
            checkForUpdate(isAutomatic = true)
        }
    }

    fun checkForUpdate(isAutomatic: Boolean = false) {
        if (_state.value is UpdateState.Checking || _state.value is UpdateState.Downloading || _state.value is UpdateState.Installing) {
            return
        }
        if (checkJob?.isActive == true || installInFlight.get()) {
            return
        }
        val previousInfo = (_state.value as? UpdateState.Available)?.info
            ?: (_state.value as? UpdateState.Error)?.info

        if (!isAutomatic) {
            _state.value = UpdateState.Checking(lastChecked())
        }

        checkJob = scope.launch {
            try {
                val info = withContext(Dispatchers.IO) {
                    fetchAndValidateFeed(getFeedUrl())
                }
                val now = System.currentTimeMillis()
                // Fetching is over before publishing an actionable offer to callers.
                checkJob = null
                prefs.edit().putLong(PREF_LAST_CHECKED_MS, now).apply()

                if (info != null && info.versionCode > BuildConfig.VERSION_CODE) {
                    prefs.edit().putString(PREF_AVAILABLE_UPDATE, info.toJson().toString()).apply()
                    // Check if already downloaded
                    val targetApk = File(updatesDir, "writer-${info.versionCode}.apk")
                    if (targetApk.exists() && targetApk.length() == info.bytes) {
                        try {
                            verifyApkFile(targetApk, info)
                            prefs.edit().putString(PREF_DOWNLOADED_APK_PATH, targetApk.absolutePath).apply()
                            _state.value = UpdateState.Downloaded(info, targetApk, now)
                            return@launch
                        } catch (_: Exception) {
                            targetApk.delete()
                        }
                    }
                    cleanObsoleteFiles(null)
                    prefs.edit().remove(PREF_DOWNLOADED_APK_PATH).apply()
                    _state.value = UpdateState.Available(info, now)
                } else {
                    clearInstallSession()
                    clearSavedUpdate()
                    cleanObsoleteFiles(null)
                    _state.value = UpdateState.Idle(now, "$PRODUCT_NAME is up to date.")
                }
            } catch (e: Exception) {
                Log.w(TAG, "Update check failed: ${e.message}")
                if (!isAutomatic) {
                    _state.value = UpdateState.Error(
                        message = e.message ?: "Failed to check for updates. Check connection and try again.",
                        info = previousInfo,
                        lastCheckedMs = lastChecked()
                    )
                }
            }
        }
    }

    private fun fetchAndValidateFeed(feedUrl: String): UpdateInfo? {
        val conn = UpdateTransfer.open(feedUrl, feed = true)

        try {
            val code = conn.responseCode
            if (code == HttpURLConnection.HTTP_NOT_FOUND) {
                return null
            }
            if (code != HttpURLConnection.HTTP_OK) {
                throw IllegalStateException("Server returned HTTP $code")
            }

            UpdateTransfer.response(conn, UpdateTransfer.MAX_FEED_BYTES)
            val bytes = ByteArrayOutputStream()
            val declaredLength = conn.contentLengthLong
            val limit = if (declaredLength >= 0) declaredLength else UpdateTransfer.MAX_FEED_BYTES
            conn.inputStream.use { UpdateTransfer.copy(it, bytes, limit, exact = declaredLength >= 0) }
            val json = UpdateTransfer.manifestJson(bytes.toString("UTF-8"))
            return parseAndValidateManifest(json)
        } finally {
            conn.disconnect()
        }
    }

    fun parseAndValidateManifest(json: JSONObject): UpdateInfo {
        require(json.length() <= 9) { "Too many release feed fields" }
        val bounds = mapOf("package" to 128, "signer_sha256" to 64, "version" to 80,
            "url" to 2048, "sha256" to 64, "notes" to 16_384, "pub_date" to 64)
        bounds.forEach { (key, max) ->
            if (json.has(key)) require(json.get(key) is String && json.getString(key).length <= max) {
                "Invalid release feed field: $key"
            }
        }
        for (key in listOf("version_code", "bytes")) {
            require(json.get(key) is Int || json.get(key) is Long) { "Invalid numeric feed field: $key" }
        }
        require(json.getLong("version_code") in 1..Int.MAX_VALUE.toLong()) { "Invalid version code" }
        val pkg = json.getString("package")
        if (pkg != EXPECTED_PACKAGE) {
            throw SecurityException("Invalid feed: package '$pkg' does not match expected '$EXPECTED_PACKAGE'")
        }

        val signer = json.getString("signer_sha256")
        if (!signer.equals(RETAINED_SIGNER_SHA256, ignoreCase = true)) {
            throw SecurityException("Invalid feed: signer digest does not match retained release key")
        }

        val versionCode = json.getInt("version_code")
        val versionName = json.getString("version")
        val url = json.getString("url")
        if (url.isBlank()) throw IllegalArgumentException("Missing artifact URL in release feed")

        require(versionName.isNotBlank()) { "Missing release version" }
        UpdateTransfer.url(url)
        val sha256 = json.getString("sha256")
        if (!sha256.matches(Regex("^[0-9a-fA-F]{64}$"))) {
            throw IllegalArgumentException("Invalid sha256 in release feed")
        }

        val bytes = json.getLong("bytes")
        if (bytes !in 1..UpdateTransfer.MAX_APK_BYTES) throw IllegalArgumentException("Invalid byte length in release feed")

        val notes = json.optString("notes", "")
        val pubDate = json.optString("pub_date", "")

        return UpdateInfo(
            version = versionName,
            versionCode = versionCode,
            packageName = pkg,
            signerSha256 = signer,
            url = url,
            sha256 = sha256.lowercase(),
            bytes = bytes,
            notes = notes,
            pubDate = pubDate
        )
    }

    fun downloadUpdate(info: UpdateInfo) {
        if (currentDownloadJob?.isActive == true || checkJob?.isActive == true || installInFlight.get()) return
        parseAndValidateManifest(info.toJson())
        require(info.versionCode > BuildConfig.VERSION_CODE) { "Update is not newer" }

        prefs.edit().putString(PREF_AVAILABLE_UPDATE, info.toJson().toString()).apply()
        downloadCancelled.set(false)
        val checkedAt = lastChecked()
        _state.value = UpdateState.Downloading(info, 0L, info.bytes, 0, checkedAt)

        currentDownloadJob = scope.launch {
            val targetFile = File(updatesDir, "writer-${info.versionCode}.apk")
            val tempFile = File(updatesDir, "writer-${info.versionCode}.apk.tmp")

            try {
                withContext(Dispatchers.IO) {
                    if (tempFile.exists()) tempFile.delete()
                    if (targetFile.exists()) targetFile.delete()

                    cleanObsoleteFiles(null)
                    UpdateTransfer.space(updatesDir.usableSpace, info.bytes * 2)
                    val conn = UpdateTransfer.open(info.url)

                    try {
                        UpdateTransfer.response(conn, info.bytes, exact = true)
                        var lastUpdate = 0L
                        val actualHash = conn.inputStream.use { input ->
                            FileOutputStream(tempFile).use { output ->
                                UpdateTransfer.copy(input, output, info.bytes,
                                    checkActive = {
                                        if (downloadCancelled.get()) throw CancellationException("Download cancelled")
                                        ensureActive()
                                    },
                                    beforeWrite = { UpdateTransfer.space(updatesDir.usableSpace, it.toLong()) },
                                    progress = { downloaded ->
                                        val now = System.currentTimeMillis()
                                        if (now - lastUpdate > 100 || downloaded == info.bytes) {
                                            lastUpdate = now
                                            _state.value = UpdateState.Downloading(info, downloaded, info.bytes,
                                                ((downloaded * 100) / info.bytes).toInt(), checkedAt)
                                        }
                                    })
                            }
                        }
                        UpdateTransfer.hashMatches(actualHash, info.sha256)

                        if (!tempFile.renameTo(targetFile)) {
                            throw IllegalStateException("Failed to move temporary update file into place")
                        }

                        verifyApkFile(targetFile, info)
                    } finally {
                        conn.disconnect()
                    }
                }

                currentDownloadJob = null
                prefs.edit().putString(PREF_DOWNLOADED_APK_PATH, targetFile.absolutePath).apply()
                _state.value = UpdateState.Downloaded(info, targetFile, checkedAt)
            } catch (e: CancellationException) {
                tempFile.delete()
                targetFile.delete()
                prefs.edit().remove(PREF_DOWNLOADED_APK_PATH).apply()
                _state.value = UpdateState.Available(info, checkedAt ?: System.currentTimeMillis())
            } catch (e: Exception) {
                Log.e(TAG, "Download failed: ${e.message}", e)
                tempFile.delete()
                targetFile.delete()
                prefs.edit().remove(PREF_DOWNLOADED_APK_PATH).apply()
                _state.value = UpdateState.Error(
                    message = "Download failed: ${e.message ?: "network error"}",
                    info = info,
                    lastCheckedMs = checkedAt
                )
            }
        }
    }

    fun cancelDownload() {
        downloadCancelled.set(true)
        currentDownloadJob?.cancel()
    }

    fun verifyApkFile(apkFile: File, expectedInfo: UpdateInfo) {
        if (!apkFile.exists() || apkFile.length() == 0L) {
            throw IllegalArgumentException("APK file does not exist or is empty")
        }

        parseAndValidateManifest(expectedInfo.toJson())
        check(apkFile.length() == expectedInfo.bytes) { "APK file size mismatch" }
        apkFile.inputStream().use {
            UpdateTransfer.hashMatches(UpdateTransfer.copy(it, OutputStream.nullOutputStream(), expectedInfo.bytes), expectedInfo.sha256)
        }
        val pm = context.packageManager
        val pkgInfo: PackageInfo = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            pm.getPackageArchiveInfo(
                apkFile.absolutePath,
                PackageManager.PackageInfoFlags.of(PackageManager.GET_SIGNING_CERTIFICATES.toLong())
            )
        } else {
            @Suppress("DEPRECATION")
            pm.getPackageArchiveInfo(apkFile.absolutePath, PackageManager.GET_SIGNING_CERTIFICATES)
        } ?: throw SecurityException("Cannot parse APK package archive info")

        if (pkgInfo.packageName != EXPECTED_PACKAGE) {
            throw SecurityException("APK package '${pkgInfo.packageName}' does not match expected '$EXPECTED_PACKAGE'")
        }

        val apkVersionCode = pkgInfo.longVersionCode
        if (apkVersionCode != expectedInfo.versionCode.toLong()) {
            throw SecurityException("APK version code $apkVersionCode does not match feed ${expectedInfo.versionCode}")
        }

        if (apkVersionCode <= BuildConfig.VERSION_CODE) {
            throw SecurityException("APK version code $apkVersionCode is not newer than currently installed ${BuildConfig.VERSION_CODE}")
        }

        if (pkgInfo.versionName != expectedInfo.version) throw SecurityException("APK version name does not match feed")

        val minSdk = pkgInfo.applicationInfo?.minSdkVersion ?: 0
        if (minSdk > Build.VERSION.SDK_INT) {
            throw IllegalStateException("APK requires Android API $minSdk but this device runs API ${Build.VERSION.SDK_INT}")
        }

        val signingInfo = pkgInfo.signingInfo ?: throw SecurityException("APK contains no signing certificates")
        if (signingInfo.hasMultipleSigners()) {
            throw SecurityException("Multiple APK signers are not permitted")
        }

        val signers = signingInfo.apkContentsSigners
        if (signers.isNullOrEmpty()) {
            throw SecurityException("No signer certificate found in APK")
        }

        val certBytes = signers[0].toByteArray()
        val certDigest = MessageDigest.getInstance("SHA-256").digest(certBytes).joinToString("") { "%02x".format(it) }

        if (!certDigest.equals(expectedInfo.signerSha256, ignoreCase = true)) {
            throw SecurityException("APK signer certificate does not match feed ($certDigest vs ${expectedInfo.signerSha256})")
        }
        if (!certDigest.equals(RETAINED_SIGNER_SHA256, ignoreCase = true)) {
            throw SecurityException("APK signer certificate does not match retained release key")
        }
    }

    fun canRequestPackageInstalls(): Boolean {
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            context.packageManager.canRequestPackageInstalls()
        } else {
            true
        }
    }

    fun createRequestInstallPermissionIntent(): Intent {
        return Intent(
            Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
            Uri.parse("package:${context.packageName}")
        ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    }

    @Synchronized
    fun commitSession(info: UpdateInfo, apkFile: File): Int {
        if (CaptureService.isActive()) {
            throw IllegalStateException("Recording is in progress. Stop or finish recording before updating.")
        }
        if (SessionManager.get(context).isBusy()) {
            throw IllegalStateException("Audio transcription or text delivery is in progress. Wait for it to complete.")
        }

        if (!canRequestPackageInstalls()) {
            throw SecurityException("$PRODUCT_NAME needs permission to install updates.")
        }

        check(checkJob?.isActive != true && currentDownloadJob?.isActive != true) { "An update transfer is still running" }
        check(prefs.getInt(PREF_INSTALL_SESSION, -1) == -1) { "An update install is already active" }
        try {
            verifyApkFile(apkFile, info)
        } catch (e: Exception) {
            rejectCachedArtifact(apkFile)
            throw e
        }
        UpdateTransfer.space(updatesDir.usableSpace, info.bytes)
        val installer = context.packageManager.packageInstaller
        val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL).apply {
            setAppPackageName(EXPECTED_PACKAGE)
            setSize(info.bytes)
            setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_NOT_REQUIRED)
        }
        val sessionId = installer.createSession(params)
        try {
            val nonce = UUID.randomUUID().toString()
            check(prefs.edit().putInt(PREF_INSTALL_SESSION, sessionId)
                .putString(PREF_INSTALL_NONCE, nonce)
                .putString(PREF_AVAILABLE_UPDATE, info.toJson().toString()).commit()) {
                "Cannot persist update install session"
            }
            installer.openSession(sessionId).use { session ->
                session.openWrite("writer_apk", 0, info.bytes).use { out ->
                    val hash = apkFile.inputStream().use { UpdateTransfer.copy(it, out, info.bytes) }
                    UpdateTransfer.hashMatches(hash, info.sha256)
                    session.fsync(out)
                }
                // Check both the archive identity and exact bytes again before sealing the session.
                verifyApkFile(apkFile, info)
                check(!CaptureService.isActive() && !SessionManager.get(context).isBusy()) {
                    "Dictation started while preparing the update. Try installing after it finishes."
                }
                val pendingIntent = installStatusCallback(sessionId, nonce)
                _state.value = UpdateState.Installing(info, sessionId, false, lastChecked())
                session.commit(pendingIntent.intentSender)
            }
        } catch (e: Exception) {
            runCatching { installer.abandonSession(sessionId) }
            clearInstallSession()
            rejectCachedArtifact(apkFile)
            throw e
        }
        return sessionId
    }

    internal fun installStatusCallback(sessionId: Int, nonce: String): android.app.PendingIntent =
        android.app.PendingIntent.getBroadcast(context, sessionId, installStatusIntent(sessionId, nonce),
            android.app.PendingIntent.FLAG_CANCEL_CURRENT or android.app.PendingIntent.FLAG_MUTABLE)

    internal fun installStatusIntent(sessionId: Int, nonce: String) = Intent(context, InstallReceiver::class.java).apply {
        action = "${context.packageName}.INSTALL_STATUS"
        `package` = context.packageName
        data = Uri.parse("writer-update://install/$sessionId/$nonce")
        putExtra(InstallReceiver.EXTRA_SESSION_ID, sessionId)
    }

    internal fun acceptsInstallStatus(intent: Intent): Boolean {
        val id = intent.getIntExtra(PackageInstaller.EXTRA_SESSION_ID, -1)
        val nonce = prefs.getString(PREF_INSTALL_NONCE, null) ?: return false
        return isActiveSession(id) && intent.getIntExtra(InstallReceiver.EXTRA_SESSION_ID, -1) == id &&
            intent.action == "${context.packageName}.INSTALL_STATUS" &&
            intent.data == installStatusIntent(id, nonce).data &&
            intent.component == installStatusIntent(id, nonce).component && intent.`package` == context.packageName
    }

    private fun isActiveSession(id: Int) = id >= 0 && prefs.getInt(PREF_INSTALL_SESSION, -1) == id

    private fun clearInstallSession() {
        prefs.edit().remove(PREF_INSTALL_SESSION).remove(PREF_INSTALL_NONCE).commit()
        installInFlight.set(false)
    }

    private fun rejectCachedArtifact(file: File) {
        // Only our update directory is disposable, never audio or caller-owned files.
        if (file.canonicalFile.parentFile == updatesDir.canonicalFile) file.delete()
        prefs.edit().remove(PREF_DOWNLOADED_APK_PATH).apply()
    }

    fun installUpdate(info: UpdateInfo, apkFile: File) {
        if (!installInFlight.compareAndSet(false, true)) {
            return
        }
        scope.launch {
            try {
                withContext(Dispatchers.IO) {
                    commitSession(info, apkFile)
                }
            } catch (e: Exception) {
                installInFlight.set(false)
                Log.e(TAG, "Install initiation failed: ${e.message}", e)
                _state.value = UpdateState.Error("Installation failed: ${e.message}", info, lastCheckedMs = lastChecked())
            }
        }
    }

    fun onPendingUserAction(sessionId: Int, confirmIntent: Intent?) {
        if (!isActiveSession(sessionId)) return
        val current = _state.value
        if (current is UpdateState.Installing) {
            _state.value = current.copy(pendingUserAction = true)
        }
    }

    fun onInstallSuccess(sessionId: Int) {
        if (!isActiveSession(sessionId)) return
        val current = _state.value
        val info = when (current) {
            is UpdateState.Installing -> current.info
            is UpdateState.Downloaded -> current.info
            else -> null
        }
        clearInstallSession()
        clearSavedUpdate()
        cleanObsoleteFiles(null)
        _state.value = UpdateState.Idle(
            System.currentTimeMillis(),
            "Update ${info?.version ?: ""} installed successfully."
        )
    }

    fun onInstallAborted(sessionId: Int, message: String?) {
        if (!isActiveSession(sessionId)) return
        val current = _state.value
        val info = when (current) {
            is UpdateState.Installing -> current.info
            is UpdateState.Downloaded -> current.info
            else -> null
        }
        clearInstallSession()
        val targetApk = info?.let { File(updatesDir, "writer-${it.versionCode}.apk") }
        if (info != null && targetApk != null && runCatching { verifyApkFile(targetApk, info) }.isSuccess) {
            _state.value = UpdateState.Downloaded(info, targetApk, lastChecked())
        } else {
            targetApk?.let { rejectCachedArtifact(it) }
            _state.value = UpdateState.Error("Installation cancelled by user.", info, lastCheckedMs = lastChecked())
        }
    }

    fun onInstallFailed(sessionId: Int, status: Int, message: String?) {
        if (!isActiveSession(sessionId)) return
        val current = _state.value
        val info = when (current) {
            is UpdateState.Installing -> current.info
            is UpdateState.Downloaded -> current.info
            else -> null
        }
        val explanation = when (status) {
            PackageInstaller.STATUS_FAILURE_BLOCKED -> "Installation blocked by device security policy."
            PackageInstaller.STATUS_FAILURE_CONFLICT -> "Package conflict with existing installation."
            PackageInstaller.STATUS_FAILURE_INCOMPATIBLE -> "Update package is incompatible with this device."
            PackageInstaller.STATUS_FAILURE_INVALID -> "Update package is invalid."
            PackageInstaller.STATUS_FAILURE_STORAGE -> "Insufficient storage space to install update."
            else -> message ?: "Installation failed (system status $status)"
        }
        clearInstallSession()
        cleanObsoleteFiles(null)
        prefs.edit().remove(PREF_DOWNLOADED_APK_PATH).apply()
        _state.value = UpdateState.Error(explanation, info, lastCheckedMs = lastChecked())
    }

    companion object {
        const val TAG = "AndroidUpdater"
        const val DEFAULT_FEED_URL = "https://updates.tricktic.com/writer/channels/android-stable.json"
        const val RETAINED_SIGNER_SHA256 = "e35e0ff5447132ced8c1228783a5f7cccf782af0f33af77e152f74e0a02f10fb"
        const val EXPECTED_PACKAGE = "com.metrix.writer"

        internal const val PREFS_NAME = "writer-updates"
        internal const val PREF_LAST_CHECKED_MS = "last_checked_ms"
        internal const val PREF_AVAILABLE_UPDATE = "available_update_json"
        internal const val PREF_DOWNLOADED_APK_PATH = "downloaded_apk_path"
        internal const val PREF_INSTALL_SESSION = "install_session"
        internal const val PREF_INSTALL_NONCE = "install_nonce"
        private const val PREF_FEED_URL_OVERRIDE = "feed_url_override"
        private const val AUTO_CHECK_INTERVAL_MS = 4 * 60 * 60 * 1000L // 4 hours

        @Volatile private var instance: AndroidUpdater? = null
        fun getInstance(context: Context): AndroidUpdater {
            return instance ?: synchronized(this) {
                instance ?: AndroidUpdater(context.applicationContext).also { instance = it }
            }
        }
    }
}
