package com.openwhisper.android.update

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.util.Log
import com.openwhisper.android.BuildConfig
import com.openwhisper.android.MainActivity
import com.openwhisper.android.R
import java.util.concurrent.Executors

object UpdateNotifier {
    private val executor = Executors.newSingleThreadExecutor { task ->
        Thread(task, "openwhisper-update-notifier").apply { isDaemon = true }
    }
    private var dailyCheck: DailyUpdateCheck? = null

    fun checkIfDue(context: Context) {
        dailyCheck(context.applicationContext).runIfDue()
    }

    @Synchronized
    private fun dailyCheck(application: Context): DailyUpdateCheck =
        dailyCheck ?: run {
            val preferences = application.getSharedPreferences(PREFERENCES, 0)
            DailyUpdateCheck(
                currentTimeMillis = System::currentTimeMillis,
                lastAttemptMillis = { preferences.getLong(LAST_CHECK, 0) },
                recordAttemptMillis = { preferences.edit().putLong(LAST_CHECK, it).apply() },
                findUpdate = { AndroidUpdateClient().findUpdate(BuildConfig.VERSION_CODE) },
                showUpdate = { showNotification(application, it) },
                runInBackground = executor::execute,
                logFailure = { Log.w(LOG_TAG, it) },
            ).also { dailyCheck = it }
        }

    fun fromIntent(intent: Intent?): UpdateManifest? {
        if (intent?.getBooleanExtra(EXTRA_UPDATE, false) != true) return null
        return runCatching {
            UpdateManifest(
                versionCode = intent.getIntExtra(EXTRA_VERSION_CODE, 0),
                versionName = intent.getStringExtra(EXTRA_VERSION_NAME).orEmpty(),
                apkUrl = intent.getStringExtra(EXTRA_APK_URL).orEmpty(),
                sha256 = intent.getStringExtra(EXTRA_SHA256).orEmpty(),
                releaseNotes = intent.getStringExtra(EXTRA_RELEASE_NOTES).orEmpty(),
            ).also { update ->
                require(update.versionCode > BuildConfig.VERSION_CODE)
                require(update.apkUrl.startsWith("https://"))
                require(update.sha256.matches(Regex("[0-9a-f]{64}")))
            }
        }.getOrNull()
    }

    private fun showNotification(context: Context, update: UpdateManifest) {
        if (
            Build.VERSION.SDK_INT >= 33 &&
            context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) !=
            PackageManager.PERMISSION_GRANTED
        ) return
        val manager = context.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(
                CHANNEL,
                context.getString(R.string.update_notification_channel),
                NotificationManager.IMPORTANCE_DEFAULT,
            ),
        )
        val launch = PendingIntent.getActivity(
            context,
            update.versionCode,
            Intent(context, MainActivity::class.java).apply {
                putExtra(EXTRA_UPDATE, true)
                putExtra(EXTRA_VERSION_CODE, update.versionCode)
                putExtra(EXTRA_VERSION_NAME, update.versionName)
                putExtra(EXTRA_APK_URL, update.apkUrl)
                putExtra(EXTRA_SHA256, update.sha256)
                putExtra(EXTRA_RELEASE_NOTES, update.releaseNotes)
                addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP)
            },
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        manager.notify(
            NOTIFICATION_ID,
            Notification.Builder(context, CHANNEL)
                .setSmallIcon(R.drawable.ic_openwhisper)
                .setContentTitle(context.getString(R.string.update_available_title))
                .setContentText(
                    context.getString(R.string.update_notification_message, update.versionName),
                )
                .setContentIntent(launch)
                .setAutoCancel(true)
                .build(),
        )
    }

    private const val LOG_TAG = "OpenWhisperUpdate"
    private const val PREFERENCES = "openwhisper_updates"
    private const val LAST_CHECK = "last_check_epoch_ms"
    private const val CHANNEL = "openwhisper_updates"
    private const val NOTIFICATION_ID = 7002
    private const val EXTRA_UPDATE = "com.openwhisper.android.extra.UPDATE"
    private const val EXTRA_VERSION_CODE = "com.openwhisper.android.extra.VERSION_CODE"
    private const val EXTRA_VERSION_NAME = "com.openwhisper.android.extra.VERSION_NAME"
    private const val EXTRA_APK_URL = "com.openwhisper.android.extra.APK_URL"
    private const val EXTRA_SHA256 = "com.openwhisper.android.extra.SHA256"
    private const val EXTRA_RELEASE_NOTES = "com.openwhisper.android.extra.RELEASE_NOTES"
}
