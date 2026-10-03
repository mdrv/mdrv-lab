// Root build.gradle.kts for mdrv-lab.
//
// Rust native library is compiled separately and placed into
// app/src/main/jniLibs/<abi>/ before building the APK.

buildscript {
    repositories {
        google()
        mavenCentral()
    }
    dependencies {
        classpath("com.android.tools.build:gradle:9.1.0")
        classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:1.9.22")
    }
}

tasks.register("clean", Delete::class) {
    delete(rootProject.layout.buildDirectory)
}
