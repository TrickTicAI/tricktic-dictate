// AGP's built-in Kotlin supports a newer KGP through its buildscript classpath.
// Keep the Compose compiler plugin at the same version.
buildscript {
    configurations.classpath { resolutionStrategy.activateDependencyLocking() }
    dependencies {
        classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:2.4.20")
        constraints {
            // Additional plugin-classpath findings from the complete MW-SEC-8 scan.
            classpath("org.bitbucket.b_c:jose4j:0.9.6")
            classpath("org.jdom:jdom2:2.0.6.1")
            classpath("org.apache.commons:commons-lang3:3.20.0")
            classpath("org.apache.httpcomponents:httpclient:4.5.14")
            classpath("org.bouncycastle:bcprov-jdk18on:1.86")
            classpath("org.bouncycastle:bcpkix-jdk18on:1.86")
            classpath("org.bouncycastle:bcutil-jdk18on:1.86")
        }
    }
}
plugins {
    id("com.android.application") version "9.4.1" apply false
    id("org.jetbrains.kotlin.plugin.compose") version "2.4.20" apply false
}
allprojects {
    dependencyLocking { lockAllConfigurations() }
    // AGP 9.4.1 still pins old libraries in the isolated lint worker. Keep these
    // compatible library families aligned; never override app/test runtime graphs.
    // Advisory mapping and validation: docs/security/android-tooling.md.
    configurations.matching { it.name == "androidLintTool" }.configureEach {
        listOf(
            "org.apache.commons:commons-lang3:3.20.0",
            "org.apache.httpcomponents:httpclient:4.5.14",
            "org.bouncycastle:bcprov-jdk18on:1.86",
            "org.bouncycastle:bcpkix-jdk18on:1.86",
            "org.bouncycastle:bcutil-jdk18on:1.86",
        ).forEach { coordinate ->
            project.dependencies.constraints.add(name, coordinate) {
                because("MW-SEC-8: patch host tooling without changing APK dependencies")
            }
        }
    }
}
