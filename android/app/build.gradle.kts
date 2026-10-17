plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "app.redline"
    compileSdk = 37

    defaultConfig {
        applicationId = "app.redline"
        minSdk = 24
        targetSdk = 37
        versionCode = 1
        versionName = "0.1.0"

        // youtubedl-android ships prebuilt python/ffmpeg per ABI; these two cover phones + emulator.
        // The Rust core (UniFFI .so files) will need the same ABI set.
        ndk {
            abiFilters += listOf("x86_64", "arm64-v8a")
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
    }

    packaging {
        jniLibs {
            // The library runs python/ffmpeg out of nativeLibraryDir, so the .so files
            // must be extracted to disk at install time (matches extractNativeLibs="true").
            useLegacyPackaging = true
        }
    }
}

dependencies {
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.compose.ui.tooling.preview)
    debugImplementation(libs.androidx.compose.ui.tooling)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.kotlinx.coroutines.android)

    implementation(libs.youtubedl.library)
    implementation(libs.youtubedl.ffmpeg)
}

// Keep the latest debug APK at android/apk/ so it's easy to find and sideload.
val copyDebugApk = tasks.register<Copy>("copyDebugApk") {
    from(layout.buildDirectory.dir("outputs/apk/debug")) {
        include("*.apk")
        rename { "redline-debug.apk" }
    }
    into(rootProject.layout.projectDirectory.dir("apk"))
}

tasks.configureEach {
    if (name == "assembleDebug") finalizedBy(copyDebugApk)
}
