package com.openwhisper.android

import android.content.Context

/** Converts density-independent pixels to whole screen pixels. */
internal fun Context.dp(value: Int): Int = (value * resources.displayMetrics.density).toInt()
