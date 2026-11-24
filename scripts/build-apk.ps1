# Baut die Android-APK.
#   npm run build:apk            -> Handy (arm64, z. B. Galaxy Z Fold 7)
#   npm run build:apk:emulator   -> Emulator (x86_64)
# Hinweis: Datei bewusst nur ASCII, weil Windows PowerShell 5.1 UTF-8 ohne BOM falsch liest.
param([switch]$Emulator)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

# --- Werkzeuge finden -------------------------------------------------------
if (-not $env:JAVA_HOME -or -not (Test-Path "$env:JAVA_HOME\bin\java.exe")) {
    $jdk = Get-ChildItem 'C:\Program Files\Eclipse Adoptium', 'C:\Program Files\Java', 'C:\Program Files\Android\Android Studio' -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match 'jdk-(17|21)|jbr' } | Select-Object -First 1
    if (-not $jdk) { throw 'Kein JDK 17/21 gefunden. Bitte Temurin 21 installieren und JAVA_HOME setzen.' }
    $env:JAVA_HOME = $jdk.FullName
}
if (-not $env:ANDROID_HOME) { $env:ANDROID_HOME = "$env:LOCALAPPDATA\Android\Sdk" }
if (-not (Test-Path $env:ANDROID_HOME)) { throw "Android SDK nicht gefunden: $env:ANDROID_HOME" }
if (-not $env:NDK_HOME) {
    $ndk = Get-ChildItem "$env:ANDROID_HOME\ndk" -Directory -ErrorAction SilentlyContinue | Sort-Object Name -Descending | Select-Object -First 1
    if (-not $ndk) { throw 'Android NDK fehlt (SDK Manager > SDK Tools > NDK).' }
    $env:NDK_HOME = $ndk.FullName
}
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:JAVA_HOME\bin;$env:PATH"
Write-Host "JAVA_HOME=$env:JAVA_HOME"
Write-Host "ANDROID_HOME=$env:ANDROID_HOME"
Write-Host "NDK_HOME=$env:NDK_HOME"

# --- Android-Projekt einmalig erzeugen -------------------------------------
$gen = "$root\src-tauri\gen\android"
if (-not (Test-Path "$gen\app\build.gradle.kts")) {
    npx tauri android init --ci
    if ($LASTEXITCODE) { throw 'tauri android init fehlgeschlagen' }
}

# --- Signierung in Gradle eintragen (einmalig) -----------------------------
$gradle = "$gen\app\build.gradle.kts"
$g = Get-Content $gradle -Raw
if ($g -notmatch 'keystore.properties') {
    if ($g -notmatch 'import java.io.FileInputStream') { $g = "import java.io.FileInputStream`r`n" + $g }
    $signing = @'
    signingConfigs {
        create("release") {
            val keystorePropertiesFile = rootProject.file("keystore.properties")
            val keystoreProperties = Properties()
            if (keystorePropertiesFile.exists()) {
                keystoreProperties.load(FileInputStream(keystorePropertiesFile))
            }
            keyAlias = keystoreProperties["keyAlias"] as String
            keyPassword = keystoreProperties["password"] as String
            storeFile = file(keystoreProperties["storeFile"] as String)
            storePassword = keystoreProperties["password"] as String
        }
    }
'@
    $g = $g -replace '(?m)^(android \{\r?\n)', "`$1$signing`r`n"
    $g = $g -replace '(getByName\("release"\) \{\r?\n)', "`$1            signingConfig = signingConfigs.getByName(`"release`")`r`n"
    Set-Content -Path $gradle -Value $g -Encoding ascii
    Write-Host 'Signierung in build.gradle.kts eingetragen'
}

# --- Signierschluessel (ausserhalb des Projekts, nie einchecken) -----------
$keyDir = "$env:USERPROFILE\PiExplorerKeys"
if (-not (Test-Path "$keyDir\pi-explorer.jks")) {
    New-Item -ItemType Directory -Force $keyDir | Out-Null
    $pw = -join ((48..57) + (65..90) + (97..122) | Get-Random -Count 24 | ForEach-Object { [char]$_ })
    # keytool meldet Fortschritt auf stderr; das ist kein Fehler
    $old = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    & "$env:JAVA_HOME\bin\keytool.exe" -genkeypair -keystore "$keyDir\pi-explorer.jks" -storetype JKS -keyalg RSA -keysize 2048 `
        -validity 10000 -alias piexplorer -storepass $pw -keypass $pw -dname 'CN=Pi Explorer, O=Private, C=DE' 2>&1 | Out-Null
    $ErrorActionPreference = $old
    if (-not (Test-Path "$keyDir\pi-explorer.jks")) { throw 'keytool konnte keinen Schluessel erzeugen' }
    "password=$pw`nkeyAlias=piexplorer`nstoreFile=$($keyDir -replace '\\','/')/pi-explorer.jks" | Set-Content -Encoding ascii "$keyDir\keystore.properties"
    Write-Host "Neuer Schluessel: $keyDir\pi-explorer.jks (gut aufbewahren - ohne ihn keine Updates!)"
}
Copy-Item "$keyDir\keystore.properties" "$gen\keystore.properties" -Force

# --- Bauen -----------------------------------------------------------------
$target = if ($Emulator) { 'x86_64' } else { 'aarch64' }
$arch = if ($Emulator) { 'x86_64' } else { 'arm64' }
$abi = if ($Emulator) { 'x86_64' } else { 'arm64-v8a' }
$old = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
npx tauri android build --apk --target $target 2>&1 | Tee-Object -Variable buildOut | ForEach-Object { "$_" }
$code = $LASTEXITCODE
$ErrorActionPreference = $old
$symlinkFail = ($buildOut | Out-String) -match 'symbolic link'
if ($code -and -not $symlinkFail) { throw 'APK-Build fehlgeschlagen (siehe Ausgabe oben)' }

if ($code) {
    # Ohne Windows-Entwicklermodus darf Tauri keinen Symlink auf die .so anlegen.
    # Die Rust-Bibliothek ist dann aber schon fertig: kopieren und Gradle direkt starten.
    $so = "$root\target\$target-linux-android\release\libpi_explorer_lib.so"
    if (-not (Test-Path $so)) { throw "Rust-Bibliothek fehlt: $so" }
    Write-Host 'Symlink nicht erlaubt - kopiere die Bibliothek und baue mit Gradle direkt'
    $jni = "$gen\app\src\main\jniLibs\$abi"
    New-Item -ItemType Directory -Force $jni | Out-Null
    Remove-Item "$jni\libpi_explorer_lib.so" -Force -ErrorAction SilentlyContinue
    Copy-Item $so $jni -Force
    $flavor = $arch.Substring(0, 1).ToUpper() + $arch.Substring(1)
    Push-Location $gen
    $ErrorActionPreference = 'Continue'
    & .\gradlew.bat "assemble${flavor}Release" "-PabiList=$abi" "-ParchList=$arch" "-PtargetList=$target" -x "rustBuild${flavor}Release" --console=plain
    $code = $LASTEXITCODE
    $ErrorActionPreference = $old
    Pop-Location
    if ($code) { throw 'Gradle-Build fehlgeschlagen' }
}

$apk = Get-ChildItem "$gen\app\build\outputs\apk" -Recurse -Filter *.apk |
    Where-Object { $_.FullName -match $arch -or $_.FullName -match 'universal' } |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
New-Item -ItemType Directory -Force "$root\release" | Out-Null
$out = "$root\release\PiExplorer-$target.apk"
Copy-Item $apk.FullName $out -Force
Write-Host ("APK: {0} ({1} MB)" -f $out, [math]::Round((Get-Item $out).Length / 1MB, 1))
