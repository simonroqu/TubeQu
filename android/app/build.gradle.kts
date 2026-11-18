plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

// qu-core liefert auch die Android-Bausteine (android/src: Oberfläche, Player; android/build-rust.sh). Ordner so, wie
// Cargo ihn auflöst: gleicher Stand wie der Rust-Teil (Cargo.lock), lokal ggf. der Patch auf ~/Work/quCore.
val quCore: String = run {
    val json = providers.exec {
        commandLine("cargo", "metadata", "--format-version", "1", "--manifest-path", rootDir.resolve("../Cargo.toml").path)
    }.standardOutput.asText.get()
    @Suppress("UNCHECKED_CAST")
    val packages = (groovy.json.JsonSlurper().parseText(json) as Map<String, Any>)["packages"] as List<Map<String, Any>>
    File(packages.first { it["name"] == "qu-core" }["manifest_path"] as String).parent
}

android {
    namespace = "dev.roqu.tubequ"
    compileSdk = 37
    defaultConfig {
        applicationId = "dev.roqu.tubequ"
        minSdk = 26
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
        // Wie build-rust.sh: Handy (arm64) und Waydroid/Emulator (x86_64).
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }
    buildFeatures { compose = true }
    // NewPipeExtractor nutzt Java-APIs, die es auf älterem Android erst durch Desugaring gibt (wie in der NewPipe-App).
    compileOptions { isCoreLibraryDesugaringEnabled = true }
    sourceSets["main"].kotlin.srcDir("$quCore/android/src")
}

// Rust-Teil (tubequ-ffi) vor jedem Build: .so in jniLibs, Kotlin-Anbindung in java/uniffi.
val buildRust by tasks.registering(Exec::class) {
    workingDir = rootDir
    commandLine("sh", "$quCore/android/build-rust.sh", "tubequ-ffi")
}
tasks.named("preBuild") { dependsOn(buildRust) }

dependencies {
    val compose = platform("androidx.compose:compose-bom:2026.09.00")
    implementation(compose)
    implementation("androidx.compose.material3:material3")
    implementation("androidx.activity:activity-compose:1.13.0")
    implementation("io.coil-kt.coil3:coil-compose:3.6.3")
    implementation("io.coil-kt.coil3:coil-network-okhttp:3.6.3")
    implementation("androidx.media3:media3-exoplayer:1.11.1")
    implementation("androidx.media3:media3-exoplayer-hls:1.11.1")
    implementation("androidx.media3:media3-exoplayer-dash:1.11.1")
    implementation("androidx.media3:media3-ui:1.11.1")
    // YouTube-Abspieladressen (rustypipe kann seit 2026-11 nicht mehr abspielen, yt-dlp gibt es auf Android nicht)
    implementation("com.github.TeamNewPipe:NewPipeExtractor:v0.27.1")
    implementation("com.squareup.okhttp3:okhttp:5.5.1")
    coreLibraryDesugaring("com.android.tools:desugar_jdk_libs_nio:2.1.5")
    // Für die UniFFI-Anbindung
    implementation("net.java.dev.jna:jna:5.19.1@aar")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.11.0")
}
