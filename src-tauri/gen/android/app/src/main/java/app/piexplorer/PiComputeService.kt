package app.piexplorer

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager
import java.io.File
import java.text.NumberFormat
import java.util.Locale

/**
 * Hält die App am Leben, solange π gerechnet wird – auch bei ausgeschaltetem Bildschirm
 * oder wenn die App im Hintergrund ist. Gerechnet wird im Rust-Teil derselben App;
 * dieser Dienst sorgt nur dafür, dass Android den Prozess nicht anhält.
 * Den Fortschritt liest er aus der Statusdatei, die der Rust-Teil schreibt.
 */
class PiComputeService : Service() {
    companion object {
        const val CHANNEL = "compute"
        const val CHANNEL_DONE = "done"
        const val ID = 31415
        const val ID_DONE = 31416
        const val EXTRA_STATUS = "status"
        const val EXTRA_QUIET = "quiet"
    }

    private var wakeLock: PowerManager.WakeLock? = null
    private val handler = Handler(Looper.getMainLooper())
    private var statusPath: String? = null
    private var missing = 0

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.getBooleanExtra(EXTRA_QUIET, false) == true) {
            // Oberfläche hat das Ergebnis schon gesehen: still beenden
            stopSelf()
            return START_NOT_STICKY
        }
        intent?.getStringExtra(EXTRA_STATUS)?.let { statusPath = it }
        createChannels()
        val n = progressNotification("Starting…", 0.0)
        if (Build.VERSION.SDK_INT >= 34) {
            startForeground(ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        } else {
            startForeground(ID, n)
        }
        if (wakeLock == null) {
            val pm = getSystemService(POWER_SERVICE) as PowerManager
            wakeLock = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "piexplorer:compute").apply {
                setReferenceCounted(false)
                acquire(48L * 60 * 60 * 1000)
            }
        }
        missing = 0
        handler.removeCallbacks(tick)
        handler.post(tick)
        return START_NOT_STICKY
    }

    private val tick = object : Runnable {
        override fun run() {
            val st = readStatus()
            when (st["state"]) {
                "running" -> {
                    val f = st["fraction"]?.toDoubleOrNull() ?: 0.0
                    val phase = st["phase"] ?: ""
                    val digits = st["digits"]?.toLongOrNull() ?: 0
                    val text = "${fmt(digits)} digits · $phase ${(f * 100).toInt()} % · ${clock(st["elapsed"])}"
                    nm().notify(ID, progressNotification(text, f))
                    handler.postDelayed(this, 2000)
                }
                "done" -> {
                    val digits = st["digits"]?.toLongOrNull() ?: 0
                    notifyDone("π is ready: ${fmt(digits)} digits", "Tap to explore them · ${clock(st["elapsed"])}")
                    stopSelf()
                }
                "error" -> {
                    notifyDone("π computation failed", "Probably not enough memory – try fewer digits")
                    stopSelf()
                }
                null -> {
                    // Datei noch nicht da: kurz warten, dann aufgeben
                    if (++missing > 15) stopSelf() else handler.postDelayed(this, 1000)
                }
                else -> stopSelf()
            }
        }
    }

    private fun readStatus(): Map<String, String> {
        val p = statusPath ?: return emptyMap()
        return try {
            File(p).readLines().mapNotNull { line ->
                val i = line.indexOf('=')
                if (i > 0) line.substring(0, i) to line.substring(i + 1) else null
            }.toMap()
        } catch (e: Exception) {
            emptyMap()
        }
    }

    private fun fmt(n: Long) = NumberFormat.getIntegerInstance(Locale.US).format(n)

    private fun clock(s: String?): String {
        val t = s?.toDoubleOrNull()?.toLong() ?: 0
        return if (t >= 3600) "%d:%02d:%02d".format(t / 3600, (t / 60) % 60, t % 60) else "%d:%02d".format(t / 60, t % 60)
    }

    private fun nm() = getSystemService(NOTIFICATION_SERVICE) as NotificationManager

    private fun openApp(): PendingIntent {
        val i = Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
        return PendingIntent.getActivity(this, 0, i, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
    }

    private fun createChannels() {
        if (Build.VERSION.SDK_INT >= 26) {
            nm().createNotificationChannel(
                NotificationChannel(CHANNEL, "Computing π", NotificationManager.IMPORTANCE_LOW).apply {
                    description = "Progress while π is computed in the background"
                    setShowBadge(false)
                }
            )
            nm().createNotificationChannel(
                NotificationChannel(CHANNEL_DONE, "π ready", NotificationManager.IMPORTANCE_DEFAULT)
            )
        }
    }

    private fun progressNotification(text: String, f: Double): Notification {
        val b = if (Build.VERSION.SDK_INT >= 26) Notification.Builder(this, CHANNEL) else Notification.Builder(this)
        // Android 12+ zeigt Dienst-Meldungen sonst erst nach 10 Sekunden
        if (Build.VERSION.SDK_INT >= 31) b.setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
        return b.setSmallIcon(R.drawable.ic_stat_pi)
            .setContentTitle("Computing π")
            .setContentText(text)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setProgress(1000, (f * 1000).toInt().coerceIn(0, 1000), false)
            .setContentIntent(openApp())
            .build()
    }

    private fun notifyDone(title: String, text: String) {
        val b = if (Build.VERSION.SDK_INT >= 26) Notification.Builder(this, CHANNEL_DONE) else Notification.Builder(this)
        nm().notify(
            ID_DONE,
            b.setSmallIcon(R.drawable.ic_stat_pi)
                .setContentTitle(title)
                .setContentText(text)
                .setAutoCancel(true)
                .setContentIntent(openApp())
                .build()
        )
    }

    override fun onDestroy() {
        handler.removeCallbacks(tick)
        wakeLock?.let { if (it.isHeld) it.release() }
        wakeLock = null
        if (Build.VERSION.SDK_INT >= 24) stopForeground(STOP_FOREGROUND_REMOVE) else @Suppress("DEPRECATION") stopForeground(true)
        super.onDestroy()
    }
}
