package app.piexplorer

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.webkit.JavascriptInterface
import androidx.core.content.ContextCompat

/** `window.PiAndroid` in der Oberfläche: Hintergrunddienst starten/stoppen. */
class PiBridge(private val activity: MainActivity) {
    @JavascriptInterface
    fun start(statusFile: String) {
        val i = Intent(activity, PiComputeService::class.java).putExtra(PiComputeService.EXTRA_STATUS, statusFile)
        ContextCompat.startForegroundService(activity, i)
    }

    /** Die Oberfläche hat das Ergebnis gesehen: Dienst ohne „fertig“-Meldung beenden. */
    @JavascriptInterface
    fun stop() {
        activity.stopService(Intent(activity, PiComputeService::class.java))
    }

    @JavascriptInterface
    fun requestNotifications() {
        if (Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(activity, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            activity.runOnUiThread {
                activity.requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS)) { }
            }
        }
    }
}
