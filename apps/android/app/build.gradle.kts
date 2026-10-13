import java.util.Properties

plugins { id("com.android.application"); id("org.jetbrains.kotlin.plugin.compose") }
android {
    useLibrary("android.test.runner")
    useLibrary("android.test.base")
    namespace = "com.metrix.writer"
    ndkVersion = "28.2.13676358"
    compileSdk { version = release(37) { minorApiLevel = 0 } }
    buildToolsVersion = "36.0.0"
    defaultConfig {
        manifestPlaceholders["writerInputLabel"] = "@string/editor_service_label"
        buildConfigField("boolean", "PREPARATION_PROBE_ENABLED", "false")
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
        externalNativeBuild { cmake {
            arguments += listOf("-DWHISPER_SOURCE=" + providers.environmentVariable("WRITER_WHISPER_SOURCE").getOrElse("/missing/set-WRITER_WHISPER_SOURCE"), "-DANDROID_STL=c++_static")
            // No build-machine paths in __FILE__ strings or debug symbols (MW-RELEASE-14).
            // clang uses the longest matching prefix, so the checkout beats the home folder.
            val neutral = listOfNotNull(
                System.getProperty("user.home") to "~",
                rootDir.resolve("../..").canonicalPath to "/writer",
                providers.environmentVariable("WRITER_WHISPER_SOURCE").orNull?.let { file(it).canonicalPath to "/writer/whisper-source" },
            ).map { (path, name) -> "-ffile-prefix-map=$path=$name" }
            cFlags += neutral
            cppFlags += neutral
        } }
        minSdk = 33
        targetSdk = 36
        testInstrumentationRunner = providers.gradleProperty("writerTestRunner").getOrElse("android.test.InstrumentationTestRunner")
    }
    // Release signing is opt-in per flavor. No key/password is stored in the repository.
    fun signing(name: String, variable: String) = providers.environmentVariable(variable).orNull?.let { path ->
        val values = Properties().apply { file(path).inputStream().use { load(it) } }
        signingConfigs.create(name) {
            storeFile = file(values.getProperty("storeFile"))
            storePassword = values.getProperty("storePassword")
            keyAlias = values.getProperty("keyAlias")
            keyPassword = values.getProperty("keyPassword")
        }
    }
    val privateRelease = signing("privateRelease", "WRITER_ANDROID_SIGNING_PROPERTIES")
    val playUpload = signing("playUpload", "WRITER_ANDROID_PLAY_SIGNING_PROPERTIES")
    // Play versionCode = major * 1000000 + minor * 1000 + patch of the tracked play version.
    val playVersion = providers.gradleProperty("writerPlayVersionName").get()
    val playCode = Regex("""(\d+)\.(\d{1,3})\.(\d{1,3})""").matchEntire(playVersion)?.destructured
        ?.let { (major, minor, patch) -> major.toInt() * 1_000_000 + minor.toInt() * 1_000 + patch.toInt() }
        ?: error("writerPlayVersionName must be MAJOR.MINOR.PATCH")
    flavorDimensions += "distribution"
    productFlavors {
        // The maintainer's private build: unchanged package and signer, self-updates from the private feed.
        create("dev") {
            dimension = "distribution"
            applicationId = "com.metrix.writer"
            versionCode = providers.gradleProperty("writerVersionCode").getOrElse("19").toInt().also { require(it > 0) }
            versionName = providers.gradleProperty("writerVersionName").getOrElse("0.5.0")
            resValue("string", "app_name", "TrickTic Dictate Dev")
            privateRelease?.let { signingConfig = it }
        }
        // The Google Play app: no self-update code, install permission or sideload guidance.
        create("play") {
            dimension = "distribution"
            applicationId = "com.tricktic.dictate"
            versionCode = playCode
            versionName = playVersion
            resValue("string", "app_name", "TrickTic Dictate")
            playUpload?.let { signingConfig = it }
        }
    }
    if (providers.gradleProperty("writerReleaseTest").orNull == "true") testBuildType = "release"
    externalNativeBuild { cmake { path = file("src/main/cpp/CMakeLists.txt"); version = "3.22.1" } }
    buildTypes {
        debug {
            applicationIdSuffix = ".debug"; resValue("string", "app_name", "TrickTic Dictate debug")
            buildConfigField("boolean", "PREPARATION_PROBE_ENABLED", (providers.gradleProperty("writerPreparationProbe").orNull == "true").toString())
            externalNativeBuild { cmake {
                arguments += "-DWRITER_PREPARATION_PROBE=" + if (providers.gradleProperty("writerPreparationProbe").orNull == "true") "ON" else "OFF"
            } }
        }
        release {
            isDebuggable = false
            // Owner-phone security acceptance: a separately named, non-debuggable release beside the installed app.
            if (providers.gradleProperty("writerPhoneSecurity").orNull == "true") {
                applicationIdSuffix = ".securityphone"
                manifestPlaceholders["writerInputLabel"] = "TrickTic Dictate security test insertion"
                resValue("string", "app_name", "TrickTic Dictate security test")
            }
        }
    }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
    buildFeatures { compose = true; buildConfig = true; resValues = true }
    lint { abortOnError = true }
}

// An explicitly supplied public test CA is compiled into debug only, for the emulator host.
val testCa = providers.environmentVariable("WRITER_ANDROID_TEST_CA")
abstract class WriterGeneratedFiles : DefaultTask() {
    @get:OutputDirectory abstract val outputDirectory: DirectoryProperty
}
val debugNetwork = tasks.register<WriterGeneratedFiles>("generateDebugNetworkSecurity") {
    outputDirectory.set(layout.buildDirectory.dir("generated/writerDebugNetwork"))
    inputs.property("testCaPath", testCa.orElse(""))
    testCa.orNull?.let { inputs.file(it) }
    val output = layout.buildDirectory.dir("generated/writerDebugNetwork")
    outputs.dir(output)
    doLast {
        val root = output.get().asFile
        root.deleteRecursively(); root.resolve("xml").mkdirs()
        var trust = ""
        testCa.orNull?.let {
            root.resolve("raw").mkdirs()
            file(it).copyTo(root.resolve("raw/writer_test_ca.pem"))
            trust = "<trust-anchors><certificates src=\"system\"/><certificates src=\"@raw/writer_test_ca\"/></trust-anchors>"
        }
        root.resolve("xml/network_security_config.xml").writeText("""
            <network-security-config>
              <base-config cleartextTrafficPermitted="false" />
              <domain-config cleartextTrafficPermitted="true">
                <domain includeSubdomains="false">10.0.2.2</domain>$trust
              </domain-config>
            </network-security-config>
        """.trimIndent())
    }
}
androidComponents.onVariants(androidComponents.selector().withBuildType("debug")) { variant ->
    variant.sources.res?.addGeneratedSourceDirectory(debugNetwork) { it.outputDirectory }
}
tasks.matching { it.name == "preDebugBuild" }.configureEach { dependsOn(debugNetwork) }

// One checked-in qualification catalog supplies app downloads and runtime identity.
val phoneAssets = tasks.register<WriterGeneratedFiles>("generatePhoneCatalog") {
    outputDirectory.set(layout.buildDirectory.dir("generated/phoneAssets"))
    inputs.file("../benchmark/assets.json")
    val sourceLicense = providers.environmentVariable("WRITER_WHISPER_SOURCE").map { "$it/LICENSE" }
    inputs.file(sourceLicense)
    doLast {
        val root = outputDirectory.get().asFile.apply { mkdirs() }
        file("../benchmark/assets.json").copyTo(root.resolve("phone-models.json"), overwrite = true)
        file(sourceLicense.get()).copyTo(root.resolve("whisper-LICENSE.txt"), overwrite = true)
    }
}
androidComponents.onVariants { variant ->
    variant.sources.assets?.addGeneratedSourceDirectory(phoneAssets) { it.outputDirectory }
}
tasks.matching { it.name == "preBuild" }.configureEach { dependsOn(phoneAssets) }

// Instrumentation tests read the shared account-sync vectors (MW-OUATH-1) byte for byte, so the
// Kotlin adapter is checked against the same files as the JavaScript oracle. Test APKs only.
val syncFixtures = tasks.register<WriterGeneratedFiles>("generateSyncFixtures") {
    outputDirectory.set(layout.buildDirectory.dir("generated/syncFixtures"))
    val source = file("../../../docs/testing/account-sync")
    val names = listOf("wire.json", "conflicts.json")
    inputs.files(names.map { source.resolve(it) })
    doLast {
        val root = outputDirectory.get().asFile.resolve("account-sync").apply { mkdirs() }
        names.forEach { source.resolve(it).copyTo(root.resolve(it), overwrite = true) }
    }
}
val vocabularyFixtures = tasks.register<WriterGeneratedFiles>("generateVocabularyFixtures") {
    outputDirectory.set(layout.buildDirectory.dir("generated/vocabularyFixtures"))
    val source = file("../../../tests/fixtures/vocabulary/parity.json")
    inputs.file(source)
    doLast {
        val root = outputDirectory.get().asFile.apply { mkdirs() }
        source.copyTo(root.resolve("vocabulary-parity.json"), overwrite = true)
    }
}
androidComponents.onVariants { variant ->
    variant.deviceTests.values.forEach { test -> test.sources.assets?.addGeneratedSourceDirectory(syncFixtures) { it.outputDirectory } }
    variant.deviceTests.values.forEach { test -> test.sources.assets?.addGeneratedSourceDirectory(vocabularyFixtures) { it.outputDirectory } }
}

// The in-app licenses screen lists each variant's resolved runtime libraries, so the dev
// build's sign-in libraries never appear in the Play build. A new group fails the build
// until its license is classified in WriterLicenseNotices and its text is in assets/licenses.
abstract class WriterLicenseNotices : DefaultTask() {
    @get:Input abstract val coordinates: ListProperty<String>
    @get:OutputDirectory abstract val outputDirectory: DirectoryProperty
    @TaskAction fun write() {
        val rows = coordinates.get().filterNot { it.substringBeforeLast(':').endsWith("-bom") }.sorted().map { coordinate ->
            val license = LICENSES.filter { coordinate.startsWith(it.first) }.maxByOrNull { it.first.length }?.second
                ?: error("Classify the license of $coordinate for the in-app licenses screen")
            "$coordinate\t$license"
        }
        check(rows.isNotEmpty()) { "No runtime dependencies resolved for the licenses screen" }
        val root = outputDirectory.get().asFile.resolve("licenses").apply { deleteRecursively(); mkdirs() }
        root.resolve("dependencies.tsv").writeText(rows.joinToString("\n", postfix = "\n"))
    }
    companion object {
        // Longest matching prefix wins. Google's SDK terms are not open-source licenses; the
        // screen names them and where to read them.
        val LICENSES = listOf("androidx.", "org.jetbrains.kotlin", "org.jetbrains.kotlinx", "org.jetbrains:", "com.google.guava:", "org.jspecify:",
            "com.google.errorprone:", "com.google.firebase:", "javax.inject:").map { it to "Apache-2.0" } + listOf(
            "com.google.firebase:firebase-auth:" to "Android-SDK-License", "com.google.firebase:firebase-auth-interop:" to "Android-SDK-License",
            "com.google.android.gms:" to "Android-SDK-License", "com.google.android.recaptcha:" to "Android-SDK-License",
            "com.google.android.libraries.identity.googleid:" to "Android-SDK-License",
            "com.google.android.play:core-common:" to "Play-Core-Terms", "com.google.android.play:integrity:" to "Play-Integrity-Terms")
    }
}
androidComponents.onVariants { variant ->
    val notices = tasks.register<WriterLicenseNotices>("generate${variant.name.replaceFirstChar { it.uppercase() }}LicenseNotices") {
        outputDirectory.set(layout.buildDirectory.dir("generated/licenseNotices/${variant.name}"))
        coordinates.set(variant.runtimeConfiguration.incoming.resolutionResult.rootComponent.map { root ->
            val visited = mutableSetOf<org.gradle.api.artifacts.component.ComponentIdentifier>()
            val queue = ArrayDeque(listOf(root))
            val found = mutableSetOf<String>()
            while (queue.isNotEmpty()) {
                val component = queue.removeFirst()
                if (!visited.add(component.id)) continue
                (component.id as? org.gradle.api.artifacts.component.ModuleComponentIdentifier)?.let { found += "${it.group}:${it.module}:${it.version}" }
                component.dependencies.filterIsInstance<org.gradle.api.artifacts.result.ResolvedDependencyResult>().forEach { queue += it.selected }
            }
            found.toList()
        })
    }
    variant.sources.assets?.addGeneratedSourceDirectory(notices) { it.outputDirectory }
}

// Optional Google sign-in (MW-OUATH-4): dev variants read Firebase's public app identifiers
// from the owner's private google-services.json named by WRITER_ANDROID_FIREBASE_CONFIG.
// Nothing is tracked or printed. Without it, or without the variant's package, sign-in
// reports that this build is not set up. The Play build has no sign-in code at all.
val firebaseConfig = providers.environmentVariable("WRITER_ANDROID_FIREBASE_CONFIG")
val accountService = providers.environmentVariable("WRITER_ACCOUNT_SERVICE_URL")
androidComponents.onVariants(androidComponents.selector().withFlavor("distribution" to "dev")) { variant ->
    val packageName = variant.applicationId.get()
    val values = firebaseConfig.orNull?.let { path ->
        @Suppress("UNCHECKED_CAST") fun Any?.map() = this as? Map<String, Any?> ?: emptyMap()
        @Suppress("UNCHECKED_CAST") fun Any?.list() = (this as? List<Any?>).orEmpty().map { it.map() }
        val json = groovy.json.JsonSlurper().parse(file(path)).map()
        val client = json["client"].list().find { it["client_info"].map()["android_client_info"].map()["package_name"] == packageName }
        if (client == null) { logger.warn("Firebase configuration has no Android app for $packageName; sign-in stays not set up."); return@let null }
        mapOf(
            "PROJECT_ID" to (json["project_info"].map()["project_id"] to Regex("[a-z][a-z0-9-]{4,28}[a-z0-9]")),
            "APP_ID" to (client["client_info"].map()["mobilesdk_app_id"] to Regex("""1:\d+:android:[0-9a-f]+""")),
            "API_KEY" to (client["api_key"].list().firstOrNull()?.get("current_key") to Regex("[A-Za-z0-9_-]{30,60}")),
            "WEB_CLIENT_ID" to (client["oauth_client"].list().find { it["client_type"] == 3 }?.get("client_id") to Regex("""\d+-[a-z0-9]+\.apps\.googleusercontent\.com""")),
        ).mapValues { (key, entry) ->
            val (value, pattern) = entry
            (value as? String)?.takeIf { pattern.matches(it) } ?: error("Firebase configuration for $packageName has no valid ${key.lowercase()}")
        }
    }.orEmpty()
    listOf("PROJECT_ID", "APP_ID", "API_KEY", "WEB_CLIENT_ID").forEach { key ->
        variant.buildConfigFields?.put("FIREBASE_$key", com.android.build.api.variant.BuildConfigField("String", "\"${values[key].orEmpty()}\"", null))
    }
    // The trusted account cleanup service (MW-OUATH-8). HTTPS only; a debug build may also use the
    // emulator host loopback 10.0.2.2. Without it, deletion and session revocation report not set up.
    val service = accountService.orNull.orEmpty().trimEnd('/')
    val allowed = Regex("""https://[a-z0-9.-]+(:\d{1,5})?(/[A-Za-z0-9._~-]+)*""")
    val emulator = Regex("""http://10\.0\.2\.2:\d{1,5}(/[A-Za-z0-9._~-]+)*""")
    check(service.isEmpty() || allowed.matches(service) || (variant.buildType == "debug" && emulator.matches(service))) {
        "WRITER_ACCOUNT_SERVICE_URL must be an https:// base URL"
    }
    variant.buildConfigFields?.put("WRITER_ACCOUNT_SERVICE_URL", com.android.build.api.variant.BuildConfigField("String", "\"$service\"", null))
}

val verifyPhoneRuntime = tasks.register("verifyPhoneRuntime") {
    doLast {
        val source = providers.environmentVariable("WRITER_WHISPER_SOURCE").orNull
            ?: error("Set WRITER_WHISPER_SOURCE to the clean pinned whisper.cpp checkout (Android README)")
        fun git(vararg args: String): String {
            val process = ProcessBuilder(listOf("git", "-C", source) + args).redirectErrorStream(true).start()
            val result = process.inputStream.bufferedReader().readText().trim()
            check(process.waitFor() == 0) { "Cannot verify phone runtime checkout" }
            return result
        }
        check(git("rev-parse", "HEAD") == "927cfce34f31707e17f2bff35c349632fb9e2c3a" && git("status", "--porcelain").isEmpty()) {
            "Phone runtime must be the clean pinned whisper.cpp v1.9.4 revision"
        }
    }
}
tasks.matching { it.name == "preBuild" || it.name.startsWith("configureCMake") || it.name.startsWith("buildCMake") }
    .configureEach { dependsOn(verifyPhoneRuntime) }


// release.py inspects Play bundles with the Gradle-verified bundletool that AGP itself locks.
tasks.register("printBundletoolClasspath") {
    val classpath = rootProject.buildscript.configurations.getByName("classpath")
    doLast { println(classpath.files.joinToString(File.pathSeparator)) }
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2026.08.00")
    implementation(composeBom)
    androidTestImplementation(composeBom)
    implementation("androidx.activity:activity-compose:1.11.0")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.9.4")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.9.4")
    implementation("androidx.navigation3:navigation3-runtime:1.1.7")
    implementation("androidx.navigation3:navigation3-ui:1.1.7")
    // Optional Google sign-in (MW-OUATH-4) ships only in the dev build until MW-RELEASE-12.
    // Versions match the MW-OUATH-2 live qualification.
    "devImplementation"("com.google.firebase:firebase-auth:24.2.0")
    "devImplementation"("androidx.credentials:credentials:1.3.0")
    "devImplementation"("androidx.credentials:credentials-play-services-auth:1.3.0")
    "devImplementation"("com.google.android.libraries.identity.googleid:googleid:1.1.1")
    debugImplementation("androidx.compose.ui:ui-tooling")
    androidTestImplementation("androidx.compose.ui:ui-test-junit4")
    androidTestImplementation("androidx.test:runner:1.7.0")
    androidTestImplementation("androidx.test:rules:1.7.0")
    androidTestImplementation("androidx.test.ext:junit:1.3.0")
}

// Byte-identical MW-105 meaning vectors are test-only assets, never shipped in the app.
val cleanupFixtures = tasks.register<WriterGeneratedFiles>("generateCleanupFixtures") {
    outputDirectory.set(layout.buildDirectory.dir("generated/cleanupFixtures"))
    val source = file("../../../tests/fixtures/cleanup/meaning.json")
    inputs.file(source)
    doLast {
        val root = outputDirectory.get().asFile.resolve("cleanup").apply { mkdirs() }
        source.copyTo(root.resolve("meaning.json"), overwrite = true)
    }
}
androidComponents.onVariants { variant ->
    variant.deviceTests.values.forEach { test -> test.sources.assets?.addGeneratedSourceDirectory(cleanupFixtures) { it.outputDirectory } }
}
