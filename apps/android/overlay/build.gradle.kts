plugins { id("com.android.application") }
android {
    namespace = "com.metrix.writer.securityoverlay"
    compileSdk { version = release(37) { minorApiLevel = 0 } }
    buildToolsVersion = "36.0.0"
    defaultConfig { applicationId = "com.metrix.writer.securityoverlay"; minSdk = 33; targetSdk = 36; versionCode = 1; versionName = "1" }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
}
