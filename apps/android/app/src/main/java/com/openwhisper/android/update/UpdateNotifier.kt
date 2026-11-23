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

    fun isUpdateIntent(intent: Intent?) = intent?.getBooleanExtra(EXTRA_UPDATE, false) == true

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
}
