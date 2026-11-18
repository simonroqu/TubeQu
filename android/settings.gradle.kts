pluginManagement {
    repositories { google(); mavenCentral(); gradlePluginPortal() }
}
dependencyResolutionManagement {
    // JitPack: NewPipeExtractor (Abspiel-Adressen von YouTube, siehe MainActivity.kt)
    repositories { google(); mavenCentral(); maven("https://jitpack.io") }
}
rootProject.name = "TubeQu"
include(":app")
