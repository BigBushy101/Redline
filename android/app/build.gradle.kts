import org.gradle.process.ExecOperations
import java.util.Properties
import javax.inject.Inject

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
}

/** ABIs the APK carries; youtubedl-android and the Rust core are both built for these. */
val rustAbis = listOf("x86_64", "arm64-v8a")

android {
    namespace = "app.redline"
    compileSdk = 37

    defaultConfig {
        applicationId = "app.redline"
        minSdk = 26 // cpal plays through AAudio, which exists only from API 26
        targetSdk = 37
        versionCode = 5
        versionName = "0.1.4"

        // youtubedl-android ships prebuilt python/ffmpeg per ABI; these two cover phones + emulator.
        // The Rust core is built for the same set (see rustAbis below).
        ndk {
            abiFilters += rustAbis
        }
    }

    // Release signing: the key lives outside the repo. Point REDLINE_SIGNING at its
    // keystore.properties (storeFile, storePassword, keyAlias, keyPassword), or keep it at
    // ~/.redline-signing/keystore.properties. Without it, the release APK is built unsigned.
    val signingProps = (System.getenv("REDLINE_SIGNING")?.let(::File)
        ?: File(System.getProperty("user.home"), ".redline-signing/keystore.properties"))
        .takeIf { it.isFile }
        ?.let { f -> Properties().apply { f.inputStream().use { load(it) } } }
    signingConfigs {
        if (signingProps != null) {
            create("release") {
                storeFile = File(signingProps.getProperty("storeFile"))
                storePassword = signingProps.getProperty("storePassword")
                keyAlias = signingProps.getProperty("keyAlias")
                keyPassword = signingProps.getProperty("keyPassword")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.findByName("release")
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
            // That also keeps them compressed in the APK; the alternative stores them
            // uncompressed and runs nothing.
            useLegacyPackaging = true
        }
    }
}

dependencies {
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.compose.material.icons.extended)
    implementation(libs.androidx.compose.ui.tooling.preview)
    debugImplementation(libs.androidx.compose.ui.tooling)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.kotlinx.coroutines.android)
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.media)
    implementation(libs.coil.compose)
    implementation(libs.coil.network.okhttp)

    implementation(libs.youtubedl.library)
    implementation(libs.youtubedl.ffmpeg)

    // UniFFI's Kotlin loads libredline_mobile.so through JNA; the aar carries JNA's own .so files.
    implementation(libs.jna) { artifact { type = "aar" } }
}

// Keep the latest debug APK at android/apk/ so it's easy to find and sideload.
val copyDebugApk = tasks.register<Copy>("copyDebugApk") {
    from(layout.buildDirectory.dir("outputs/apk/debug")) {
        include("*.apk")
        rename { "redline-debug.apk" }
    }
    into(rootProject.layout.projectDirectory.dir("apk"))
}

// Same for the signed release APK: android/apk/redline-<versionName>.apk.
val copyReleaseApk = tasks.register<Copy>("copyReleaseApk") {
    val apkName = "redline-${android.defaultConfig.versionName}.apk"
    from(layout.buildDirectory.dir("outputs/apk/release")) {
        include("*.apk")
        rename { apkName }
    }
    into(rootProject.layout.projectDirectory.dir("apk"))
}

tasks.configureEach {
    if (name == "assembleDebug") finalizedBy(copyDebugApk)
    if (name == "assembleRelease") finalizedBy(copyReleaseApk)
}

// ---- The Rust core (redline-mobile, through UniFFI) ----------------------------------------
//
// Two tasks per variant, both run cargo from the repo root:
//  * uniffiBindings<Variant>: builds redline-mobile for this computer and generates the Kotlin
//    bindings from that library (they're the same for every ABI) into a generated source dir.
//  * cargoNdk<Variant>: cross-compiles libredline_mobile.so for rustAbis with cargo-ndk into a
//    generated jniLibs dir. Needs the Android NDK; -Predline.skipRust=true skips it, which still
//    compiles the app but leaves the core out of the APK.

val rustRepoRoot: Directory = rootProject.layout.projectDirectory.dir("..")
val skipRust: Provider<Boolean> = providers.gradleProperty("redline.skipRust").map { it.toBoolean() }.orElse(false)

/** What a Rust rebuild depends on, so Gradle knows when to call cargo again. */
fun rustSources() = fileTree(rustRepoRoot) {
    include("Cargo.toml", "Cargo.lock", "redline-core/**", "redline-mobile/**")
    exclude("**/target/**")
}

/** Cross-compiles the core with cargo-ndk into outputDir/<abi>/libredline_mobile.so. */
abstract class CargoNdkTask : DefaultTask() {
    @get:Inject abstract val exec: ExecOperations

    @get:InputFiles @get:PathSensitive(PathSensitivity.RELATIVE)
    abstract val sources: ConfigurableFileCollection

    @get:Input abstract val abis: ListProperty<String>
    @get:Input abstract val release: Property<Boolean>
    @get:Input abstract val minSdk: Property<Int>
    @get:Input abstract val skip: Property<Boolean>

    /** ANDROID_NDK_HOME (or ANDROID_NDK_ROOT) when set. */
    @get:Input @get:Optional abstract val ndkHome: Property<String>

    /** The SDK, whose ndk/<version> folders are the fallback. */
    @get:Internal abstract val sdkDir: DirectoryProperty

    @get:Internal abstract val repoRoot: DirectoryProperty

    @get:OutputDirectory abstract val outputDir: DirectoryProperty

    @TaskAction
    fun build() {
        val out = outputDir.get().asFile
        out.deleteRecursively()
        out.mkdirs()
        if (skip.get()) {
            logger.warn("redline.skipRust is set: the APK won't contain libredline_mobile.so, so the Rust core can't load.")
            return
        }
        val ndk = findNdk() ?: throw GradleException(
            listOf(
                "The Android NDK isn't installed, so the Rust core (redline-mobile) can't be built.",
                "  Install it in Android Studio (SDK Manager > SDK Tools > NDK (Side by side)),",
                "  or set ANDROID_NDK_HOME to an NDK folder.",
                "  Looked in: ${ndkHome.orNull ?: "ANDROID_NDK_HOME/ANDROID_NDK_ROOT (not set)"} and ${sdkDir.get().asFile.resolve("ndk")}",
                "  To build the app without the core for now, pass -Predline.skipRust=true.",
            ).joinToString("\n")
        )
        logger.lifecycle("Building redline-mobile for ${abis.get().joinToString()} with the NDK at $ndk")
        exec.exec {
            workingDir = repoRoot.get().asFile
            environment("ANDROID_NDK_HOME", ndk.absolutePath)
            commandLine(
                buildList {
                    addAll(listOf("cargo", "ndk"))
                    abis.get().forEach { addAll(listOf("-t", it)) }
                    addAll(listOf("-P", minSdk.get().toString(), "-o", out.absolutePath, "build", "-p", "redline-mobile"))
                    // Debug builds use the workspace's android-dev profile: dev without the
                    // debuginfo that made the library ~150 MB per ABI (see Cargo.toml).
                    addAll(if (release.get()) listOf("--release") else listOf("--profile", "android-dev"))
                }
            )
        }
    }

    /** The configured NDK, else the newest one the SDK Manager installed. */
    private fun findNdk(): File? {
        ndkHome.orNull?.let { File(it) }?.takeIf { it.isDirectory }?.let { return it }
        val sdk = sdkDir.get().asFile
        val installed = sdk.resolve("ndk").listFiles { f -> f.isDirectory && f.resolve("source.properties").isFile }
        val newest = installed?.maxByOrNull { v ->
            v.name.split('.').map { it.toLongOrNull() ?: 0L }.fold(0L) { acc, n -> acc * 100_000 + n }
        }
        return newest ?: sdk.resolve("ndk-bundle").takeIf { it.resolve("source.properties").isFile }
    }
}

/** Builds redline-mobile for this computer and generates its Kotlin bindings (library mode). */
abstract class UniffiBindingsTask : DefaultTask() {
    @get:Inject abstract val exec: ExecOperations

    @get:InputFiles @get:PathSensitive(PathSensitivity.RELATIVE)
    abstract val sources: ConfigurableFileCollection

    @get:Internal abstract val repoRoot: DirectoryProperty

    /** CARGO_TARGET_DIR when set; else <repo>/target. */
    @get:Input @get:Optional abstract val cargoTargetDir: Property<String>

    @get:OutputDirectory abstract val outputDir: DirectoryProperty

    @TaskAction
    fun generate() {
        val root = repoRoot.get().asFile
        val out = outputDir.get().asFile
        out.deleteRecursively()
        out.mkdirs()
        // The bindgen binary is behind the crate's `cli` feature; building the library with the
        // same features keeps cargo from rebuilding it between the two steps.
        val crate = listOf("-p", "redline-mobile", "--features", "cli")
        exec.exec {
            workingDir = root
            commandLine(listOf("cargo", "build") + crate)
        }
        val os = System.getProperty("os.name").lowercase()
        val libName = when {
            os.contains("windows") -> "redline_mobile.dll"
            os.contains("mac") -> "libredline_mobile.dylib"
            else -> "libredline_mobile.so"
        }
        val targetDir = cargoTargetDir.orNull?.let { File(it) }?.let { if (it.isAbsolute) it else root.resolve(it) }
            ?: root.resolve("target")
        val library = targetDir.resolve("debug").resolve(libName)
        exec.exec {
            workingDir = root
            commandLine(
                listOf("cargo", "run", "-q") + crate + listOf(
                    "--bin", "uniffi-bindgen", "--",
                    "generate", "--library", library.absolutePath,
                    "--language", "kotlin", "--no-format", "--out-dir", out.absolutePath,
                )
            )
        }
    }
}

androidComponents {
    onVariants { variant ->
        val name = variant.name.replaceFirstChar { it.uppercase() }

        val bindings = tasks.register<UniffiBindingsTask>("uniffiBindings$name") {
            group = "redline"
            description = "Generates the Kotlin bindings for redline-mobile."
            sources.from(rustSources())
            repoRoot.set(rustRepoRoot)
            cargoTargetDir.set(providers.environmentVariable("CARGO_TARGET_DIR"))
        }
        variant.sources.kotlin?.addGeneratedSourceDirectory(bindings, UniffiBindingsTask::outputDir)

        val rust = tasks.register<CargoNdkTask>("cargoNdk$name") {
            group = "redline"
            description = "Builds libredline_mobile.so for ${rustAbis.joinToString()} with cargo-ndk."
            sources.from(rustSources())
            abis.set(rustAbis)
            release.set(variant.buildType == "release")
            minSdk.set(variant.minSdk.apiLevel)
            skip.set(skipRust)
            ndkHome.set(
                providers.environmentVariable("ANDROID_NDK_HOME")
                    .orElse(providers.environmentVariable("ANDROID_NDK_ROOT"))
            )
            sdkDir.set(androidComponents.sdkComponents.sdkDirectory)
            repoRoot.set(rustRepoRoot)
        }
        variant.sources.jniLibs?.addGeneratedSourceDirectory(rust, CargoNdkTask::outputDir)
    }
}
