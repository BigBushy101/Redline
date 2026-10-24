<#
.SYNOPSIS
  Builds the signed Android APK and the Windows exe into dist/.
.DESCRIPTION
  Reads the version from Cargo.toml and checks it against versionName in
  android/app/build.gradle.kts. The Android release is signed with the key described by
  ~/.redline-signing/keystore.properties (or the file named by REDLINE_SIGNING).
.PARAMETER Publish
  After building, create the GitHub release v<version> with gh and upload both files.
#>
param([switch]$Publish)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

# --- Version: Cargo.toml [package] must match the Android versionName ---
$inPackage = $false
$version = $null
foreach ($line in Get-Content (Join-Path $root 'Cargo.toml')) {
    if ($line -match '^\s*\[(.+)\]\s*$') { $inPackage = ($Matches[1] -eq 'package'); continue }
    if ($inPackage -and $line -match '^\s*version\s*=\s*"([^"]+)"') { $version = $Matches[1]; break }
}
if (-not $version) { throw 'No version found in the [package] section of Cargo.toml.' }

$gradleFile = Join-Path $root 'android\app\build.gradle.kts'
$m = Select-String -Path $gradleFile -Pattern 'versionName\s*=\s*"([^"]+)"' | Select-Object -First 1
if (-not $m) { throw 'No versionName found in android/app/build.gradle.kts.' }
$androidVersion = $m.Matches[0].Groups[1].Value
if ($androidVersion -ne $version) {
    throw "Version mismatch: Cargo.toml says $version but versionName is $androidVersion."
}
Write-Host "Redline $version"

# --- Signing key: refuse to ship an unsigned APK ---
if ($env:REDLINE_SIGNING) { $signing = $env:REDLINE_SIGNING }
else { $signing = Join-Path $env:USERPROFILE '.redline-signing\keystore.properties' }
if (-not (Test-Path -LiteralPath $signing -PathType Leaf)) {
    throw "Signing properties not found at $signing. Set REDLINE_SIGNING or see the Releases section of the README."
}

# --- Environment for the Android build (only what isn't already set) ---
if (-not $env:ANDROID_HOME) { $env:ANDROID_HOME = Join-Path $env:LOCALAPPDATA 'Android\Sdk' }
if (-not $env:JAVA_HOME) { $env:JAVA_HOME = 'C:\Program Files\Android\Android Studio\jbr' }
if (-not $env:ANDROID_NDK_HOME -and -not $env:ANDROID_NDK_ROOT) {
    $ndkRoot = Join-Path $env:ANDROID_HOME 'ndk'
    $ndk = $null
    if (Test-Path $ndkRoot) {
        $ndk = Get-ChildItem $ndkRoot -Directory | Sort-Object { [version]($_.Name -replace '[^0-9.]', '') } | Select-Object -Last 1
    }
    if ($ndk) { $env:ANDROID_NDK_HOME = $ndk.FullName }
}
# Gradle's daemon fails with "Unable to establish loopback connection" on some Windows setups
# unless JNI unix sockets use a short, long-name (not 8.3) directory.
if (-not $env:JAVA_TOOL_OPTIONS) {
    $sockDir = Join-Path $env:LOCALAPPDATA 'Temp\redline-gradle'
    New-Item -ItemType Directory -Force $sockDir | Out-Null
    $env:JAVA_TOOL_OPTIONS = '"-Djdk.net.unixdomain.tmpdir=' + $sockDir + '"'
}

# --- Android ---
Write-Host 'Building the Android release...'
Push-Location (Join-Path $root 'android')
try {
    & .\gradlew.bat assembleRelease
    if ($LASTEXITCODE -ne 0) { throw 'Android build failed.' }
} finally { Pop-Location }
$apk = Join-Path $root "android\apk\redline-$version.apk"
if (-not (Test-Path $apk)) { throw "Expected $apk after the Android build." }

# --- Windows ---
Write-Host 'Building the Windows release...'
Push-Location $root
try {
    & cargo build --release
    if ($LASTEXITCODE -ne 0) { throw 'Windows build failed.' }
} finally { Pop-Location }
$builtExe = Join-Path $root 'target\release\redline.exe'
if ($env:CARGO_TARGET_DIR) { $builtExe = Join-Path $env:CARGO_TARGET_DIR 'release\redline.exe' }

# --- Collect ---
$dist = Join-Path $root 'dist'
New-Item -ItemType Directory -Force $dist | Out-Null
$apkOut = Join-Path $dist "Redline-$version-android.apk"
$exeOut = Join-Path $dist "Redline-$version-windows-x64.exe"
Copy-Item $apk $apkOut -Force
Copy-Item $builtExe $exeOut -Force
Write-Host "Built:"
Write-Host "  $apkOut"
Write-Host "  $exeOut"

# --- Publish ---
if ($Publish) {
    & gh release create "v$version" $apkOut $exeOut --title "Redline $version" --generate-notes
    if ($LASTEXITCODE -ne 0) { throw 'gh release create failed.' }
}
