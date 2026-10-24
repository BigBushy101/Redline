# Redline

Redline is a media downloader, converter and music player. It downloads with yt-dlp, converts and joins files with ffmpeg, and keeps a music library. The Windows app is written in Rust with egui; the Android app is Kotlin Compose on top of the same Rust core.

## Features

| Feature | Windows | Android |
| --- | :---: | :---: |
| Download media with yt-dlp | yes | yes |
| Convert and join files with ffmpeg (images too) | yes | yes |
| Music library: Saved links by platform, Playlists, Local files | yes | yes |
| Player: queue, shuffle, speed, EQ, A-B loop | yes | yes |
| Export and import saved links | yes | yes |
| Flag dead links before play and find replacement uploads | yes | yes |
| Windows media keys and media overlay | yes | no |

The Android app gets yt-dlp and ffmpeg from the youtubedl-android libraries, and updates yt-dlp at launch.

## Requirements

Windows app:
- Rust toolchain (edition 2024, so a recent stable release)
- yt-dlp, ffmpeg and Deno are downloaded by Redline itself into its data folder (`bin/`); nothing to install by hand.

Android app, in addition:
- JDK 17
- Android SDK (compileSdk 37, minSdk 26) and the NDK, found through `ANDROID_NDK_HOME` / `ANDROID_NDK_ROOT` or the SDK's `ndk/` folder
- `cargo-ndk`, and the Rust targets for `x86_64` and `arm64-v8a` Android

## Build and run

Windows, from the repo root:

    cargo run --release

Android, from `android/`:

    gradlew assembleDebug      # debug APK, android/apk/redline-debug.apk
    gradlew assembleRelease    # signed release APK, android/apk/redline-<version>.apk

Gradle builds `redline-mobile` for the host to generate the Kotlin bindings with UniFFI, then cross-compiles `libredline_mobile.so` for `x86_64` and `arm64-v8a` with `cargo ndk`. The APK is copied to `android/apk/redline-debug.apk`. Passing `-Predline.skipRust=true` skips the Rust build (the APK then has no core).

## Releases

`scripts/release.ps1` builds the signed Android APK (`assembleRelease`) and the Windows exe (`cargo build --release`) and collects them in `dist/` as `Redline-<version>-android.apk` and `Redline-<version>-windows-x64.exe`. It first checks that the version in `Cargo.toml` matches `versionName` in `android/app/build.gradle.kts`. Run it from PowerShell:

    scripts\release.ps1             # build both into dist/
    scripts\release.ps1 -Publish    # also run `gh release create v<version>` with both files

The release APK is signed with a key that is not in this repo. Put a `keystore.properties` file (keys `storeFile`, `storePassword`, `keyAlias`, `keyPassword`) at `~/.redline-signing/keystore.properties`, or point the `REDLINE_SIGNING` environment variable at it. Without it Gradle builds an unsigned APK and the script refuses to run.

Back up the keystore and its passwords somewhere safe. Android only installs an update over an existing install if it is signed with the same key, so if the key is lost, users have to uninstall Redline (losing its data) before they can install a newer version.

Note: the egui version (0.36) is pinned to match egui-phosphor 0.14; don't bump one without the other.

## Project layout

- `src/` - the Windows app (egui)
- `redline-core/` - shared Rust logic: tools (yt-dlp, ffmpeg), job queue, library, saved links, player, link checking
- `redline-mobile/` - UniFFI bindings exposing the core to Kotlin
- `android/` - the Android app (Kotlin, Compose) and its Gradle build
- `assets/` - bundled fonts
