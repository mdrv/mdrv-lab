// App module build.gradle.kts for mdrv-lab.
//
// Packages the pre-compiled Rust native library (libmdrv_lab.so) into an
// APK hosted by Android's NativeActivity (via GpuiActivity).
//
// Build steps:
//   cd /g/mdrv-lab
//   cargo build --lib --target aarch64-linux-android --release
//   cp target/aarch64-linux-android/release/libmdrv_lab.so \
//       android/gradle/app/src/main/jniLibs/arm64-v8a/
//   cd android/gradle && ./gradlew assembleDebug

plugins {
    id("com.android.application")
}

android {
    namespace = "id.mdrv.lab"
    compileSdk = 34

    defaultConfig {
        applicationId = "id.mdrv.lab"
        minSdk = 26          // Vulkan 1.0 is mandatory from API 26+
        targetSdk = 34
        versionCode = 1
        versionName = "0.1.0"

        ndk {
            abiFilters += listOf("arm64-v8a")
        }

        // Must match the cdylib output name (without lib/.so).
        manifestPlaceholders["nativeLibraryName"] = "mdrv_lab"
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro"
            )
        }
        debug {
            isDebuggable = true
            isJniDebuggable = true
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_1_8
        targetCompatibility = JavaVersion.VERSION_1_8
    }

    sourceSets {
        getByName("main") {
            jniLibs.srcDirs("src/main/jniLibs")
        }
    }

    packaging {
        jniLibs {
            keepDebugSymbols += listOf("*/arm64-v8a/libmdrv_lab.so")
        }
    }

    lint {
        abortOnError = false
        checkReleaseBuilds = false
    }
}

dependencies {
    // AndroidX SplashScreen compat (used by GpuiActivity to hold splash until native init)
    implementation("androidx.core:core-splashscreen:1.0.1")
    // AndroidX core (NotificationCompat etc. used by gpui-mobile helpers)
    implementation("androidx.core:core:1.12.0")
}
