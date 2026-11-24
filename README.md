# π EXPLORER

Berechnet die Nachkommastellen von π selbst (Chudnovsky + Binary Splitting, nichts vorgespeichert),
zeigt sie als Raster und findet darin Zahlen, Geburtstage, Wörter und gemalte Formen.
Eine Codebasis für **Windows (.exe)** und **Android (.apk)** mit Tauri 2.

## Aufbau

```
crates/pi-core/        Rust-Kern ohne UI (läuft auf Desktop und Android)
  src/chudnovsky.rs      π-Berechnung, Fortschritt, Abbruch, Dezimalumwandlung (Teile und herrsche)
  src/search.rs          Ziffernsuche (memchr::memmem, parallel, alle Treffer inkl. überlappender), Wort/Datum
  src/shape.rs           Formensuche im Schwarz-Weiß-Raster mit Toleranz
  src/stats.rs           Ziffernhäufigkeit, längste Wiederholung
  src/estimate.rs        Zeit-/RAM-Schätzung mit kurzer Gerätemessung
  examples/bench.rs      Benchmark
  tests/reference.rs     Abgleich mit externer Referenz (nur im Test geladen)
src-tauri/             Tauri-App: Befehle, Hintergrund-Threads, Fortschritt per Channel
src/                   Oberfläche (TypeScript, Vite, WebGL2)
  renderer.ts            Vollbild-Shader: zeichnet nur sichtbare Zellen, Ziffern in R8UI-Array-Textur
  camera.ts              Zoom/Pan, gefederte Kamerafahrten (van-Wijk-Kurve)
  main.ts                Startbildschirm, Suche, Formensuche, HUD, Statistik
scripts/build-apk.ps1  APK-Build inkl. Signierung
scripts/cdp.mjs        Testhilfe: steuert die laufende App über das DevTools-Protokoll
```

Positionen sind 1-basiert ab der ersten Nachkommastelle (die „1“ nach „3.“ ist Position 1).

## Rechenmodi

- **FAST**: alles im RAM, am schnellsten, braucht aber ≈ 27 Byte pro Stelle (100 Mio. ≈ 2,7 GB).
- **SPLIT** (`crates/pi-core/src/split.rs`): fest begrenztes RAM-Budget (512 MB … 8 GB), nur im
  Speicher, keine Dateien. Große Multiplikationen werden in Blöcke zerlegt, P/Q/T der Reihe auf die
  nötige Genauigkeit gekürzt, Division/Wurzel per Newton und die Dezimalumwandlung blockweise.
  ≈ 2,5 Byte pro Stelle plus fester Anteil, dafür langsamer. Gemessen: 100 Mio. Stellen mit 512 MB
  in 91 s (Spitze 421 MB), 250 Mio. mit 1 GB in 5,6 min. Grenze ≈ Budget × 0,62 / 2,6 Stellen.
- **SPLIT mit Hilfsdateien** (`crates/pi-core/src/ooc.rs`): Passen die Stellen nicht mehr ins
  RAM-Budget, legt die App die großen Zwischenzahlen als temporäre Dateien ab und rechnet blockweise
  darauf (Karatsuba-Multiplikation auf Platte; die oberen Stufen parallel, solange die RAM-Blöcke
  ≥ 1 Mio. Limbs bleiben). Die fertigen Ziffern liegen dann ebenfalls in einer Datei (1 Byte/Stelle);
  Ansicht, Suche, Formensuche und Statistik lesen fensterweise daraus.
  - Ort wählbar (App-Speicher oder ein Laufwerk, dort Ordner `PiExplorerTemp`).
  - Platzbedarf in der Spitze ≈ 5,5–6 Byte pro Stelle (10 Mrd. Stellen ≈ 60 GB).
  - Zwischenzahlen werden gelöscht, sobald sie nicht mehr gebraucht werden; die Ziffern-Datei bei
    der nächsten Rechnung oder beim Beenden; Reste nach einem Absturz beim nächsten Start.
  - Gemessen: 200 Mio. Stellen mit 256 MB RAM in 14 min (Platte 0,9 GB + 0,2 GB Ziffern).
- Bei mehr Stellen, als die Grafikkarte halten soll (PC 200 Mio., Handy 48 Mio.), liegt nur ein
  Fenster auf der GPU; es wird beim Verschieben und bei Treffern nachgeladen.

## Rechnen im Hintergrund

- **Android**: Vordergrund-Dienst (`PiComputeService.kt`) mit Wakelock und Fortschritts-Benachrichtigung –
  rechnet bei ausgeschaltetem Bildschirm, im Hintergrund und auch nach dem Wegwischen aus den
  „Zuletzt verwendet“ weiter. Beim Öffnen hängt sich die Oberfläche an die laufende oder fertige
  Rechnung an (`MainActivity.kt` → `nativeRecreateWindow`). Ist die App beim Fertigwerden nicht sichtbar,
  kommt „π is ready“.
- **PC**: Minimiert rechnet sie normal weiter; Fenster schließen während einer Rechnung → Symbol im
  Infobereich (Klick öffnet wieder), Windows geht solange nicht in den Standby.
- Wird der Prozess hart beendet (Task-Manager, „Beenden erzwingen“), ist die Rechnung weg – die
  Hilfsdateien sind nur Zwischenstände der laufenden Rechnung, kein Spielstand zum Fortsetzen.

## Zahlen eingeben

`750k` · `250m` · `1.5b` · `2 mrd` · `1e9` · `1,000,000` – die Liste steht auch im Startmenü unter
ABBREVIATIONS: k/tsd = Tausend, m/mio/mil = Million, b/bn/mrd/g = Milliarde, eN = 10^N.

## Was du installieren musst

| Für | Was | Hinweis |
| --- | --- | --- |
| alles | **Rust** (rustup, stable) | <https://rustup.rs> |
| alles | **Node.js 20+** | danach `npm install` im Projektordner |
| EXE | **Visual Studio Build Tools** mit „Desktopentwicklung mit C++“ | liefert den MSVC-Linker |
| EXE | **WebView2** | ist bei Windows 11 schon dabei |
| APK | **Android SDK** (Android Studio oder cmdline-tools) | Platform 34+, Build-Tools, Platform-Tools |
| APK | **Android NDK** | SDK Manager → SDK Tools → NDK (Side by side) |
| APK | **JDK 17 oder 21** | z. B. Eclipse Temurin 21 |
| APK | Rust-Ziele | `rustup target add aarch64-linux-android x86_64-linux-android` |

Optional: Windows-**Entwicklermodus** (Einstellungen → System → Für Entwickler). Ohne ihn darf Tauri keinen
Symlink anlegen; das Skript kopiert die Bibliothek dann einfach und baut direkt mit Gradle weiter.

## Befehle

```
npm install
npm run tauri dev          # Desktop-App mit Live-Neuladen
npm run test:core          # Unit-Tests (erste 100 Stellen, Feynman-Punkt, Suche, Formen, Statistik)
npm run test:reference     # 10 Mio. Stellen + Stichproben aus 100 Mio. gegen externe Referenz
npm run bench              # Benchmark 1 Mio. und 10 Mio. Stellen
npm run build:exe          # target/release/pi-explorer.exe + Installer (target/release/bundle/nsis)
npm run build:apk          # release/PiExplorer-aarch64.apk (Handy)
npm run build:apk:emulator # release/PiExplorer-x86_64.apk (Emulator)
```

Der APK-Signierschlüssel liegt in `%USERPROFILE%\PiExplorerKeys` (außerhalb des Projekts). Gut aufbewahren:
Ohne ihn lassen sich Updates nicht über die installierte App spielen.

## Installieren

**Windows:** `Pi Explorer_1.0.0_x64-setup.exe` ausführen (installiert nur für den aktuellen Benutzer, keine
Admin-Rechte). Alternativ `pi-explorer.exe` direkt starten. SmartScreen warnt bei unsignierten Programmen:
„Weitere Informationen“ → „Trotzdem ausführen“.

**Android:** `PiExplorer-aarch64.apk` aufs Handy kopieren, antippen, „Installation aus dieser Quelle zulassen“
bestätigen. Oder per USB: `adb install -r PiExplorer-aarch64.apk`.

## Bedienung

`1–4` Modi · `/` Suche · `S` Formen · `F` alles zeigen · `R` zurück zum Anfang · `H` Statistik ·
`←/→` vorheriger/nächster Treffer · `[ ]` Zeilenbreite (mit Shift ±10) · Mausrad zoomt zur Maus ·
Ziehen verschiebt · Doppelklick zoomt hinein. Am Handy: Pinch-Zoom, Wischen, Doppeltippen.

## Hinweise

- `malachite` steht unter LGPL-3.0. Für private Nutzung egal; bei Weitergabe einer EXE/APK müsste man
  Nutzern ermöglichen, die Bibliothek auszutauschen (z. B. Quellcode beilegen).
- `rug`/GMP als Desktop-Option ist nicht eingebaut: GMP lässt sich mit der MSVC-Toolchain nicht ohne MSYS2
  bauen, und malachite ist hier schon schnell genug (10 Mio. Stellen ≈ 1,5 s).

## Lizenz

MIT – siehe [LICENSE](LICENSE).

Für eigene APK-Builds brauchst du einen eigenen Signierschlüssel: Lege `src-tauri/gen/android/keystore.properties` mit `keyAlias`, `password` und `storeFile` an (diese Datei wird nicht hochgeladen).
