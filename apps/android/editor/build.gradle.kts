plugins { id("com.android.application") }
android {
    useLibrary("android.test.runner")
    useLibrary("android.test.base")
    namespace = "com.metrix.writer.editor"
    compileSdk { version = release(37) { minorApiLevel = 0 } }
    buildToolsVersion = "36.0.0"
    defaultConfig { applicationId = "com.metrix.writer.editor"; minSdk = 33; targetSdk = 36; versionCode = 1; versionName = "1"; testInstrumentationRunner = "android.test.InstrumentationTestRunner" }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
}
