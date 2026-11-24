package app.piexplorer

import android.graphics.Color
import android.os.Bundle
import android.webkit.WebView
import androidx.activity.SystemBarStyle
import androidx.activity.enableEdgeToEdge

class MainActivity : TauriActivity() {
  companion object {
    /** Gab es in diesem Prozess schon eine Activity, und wie viele leben gerade? */
    private var everCreated = false
    private var alive = 0

    /** Rust: Hauptfenster neu anlegen (siehe lib.rs). */
    @JvmStatic external fun nativeRecreateWindow()
  }

  override fun onCreate(savedInstanceState: Bundle?) {
    // App ist immer dunkel: helle Symbole in Status- und Navigationsleiste
    enableEdgeToEdge(
      statusBarStyle = SystemBarStyle.dark(Color.TRANSPARENT),
      navigationBarStyle = SystemBarStyle.dark(Color.TRANSPARENT)
    )
    // Wieder geöffnet, während der Prozess (z. B. wegen einer Hintergrundrechnung) weiterlief?
    val reopened = everCreated && alive == 0
    everCreated = true
    alive++
    super.onCreate(savedInstanceState)
    if (reopened) {
      try {
        nativeRecreateWindow()
      } catch (e: UnsatisfiedLinkError) {
        // Bibliothek nicht geladen: dann ist das ohnehin ein frischer Prozess
      }
    }
  }

  override fun onDestroy() {
    alive--
    super.onDestroy()
  }

  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)
    // window.PiAndroid für den Hintergrunddienst
    webView.addJavascriptInterface(PiBridge(this), "PiAndroid")
  }
}
