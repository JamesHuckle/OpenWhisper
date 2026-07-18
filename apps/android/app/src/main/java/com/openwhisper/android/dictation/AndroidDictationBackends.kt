package com.openwhisper.android.dictation

import android.os.Handler
import android.os.Looper
import java.util.concurrent.ExecutorService

class ExecutorBackgroundRunner(private val executor: ExecutorService) : BackgroundRunner {
    override fun run(task: () -> Unit) {
        executor.execute(task)
    }
}

class MainThreadCallbackDispatcher(
    private val handler: Handler = Handler(Looper.getMainLooper()),
) : CallbackDispatcher {
    override fun dispatch(task: () -> Unit) {
        handler.post(task)
    }
}

class DeterministicDemoBackend(
    private val handler: Handler = Handler(Looper.getMainLooper()),
) : DictationBackend {
    private var generation = 0L
    private var listener: DictationBackend.Listener? = null

    override fun start(listener: DictationBackend.Listener) {
        generation += 1
        val expected = generation
        this.listener = listener
        handler.postDelayed({
            if (expected == generation && this.listener === listener) {
                listener.onPartial("OpenWhisper")
            }
        }, 120)
    }

    override fun finish() {
        val expected = generation
        val expectedListener = listener ?: return
        handler.postDelayed({
            if (expected == generation && listener === expectedListener) {
                expectedListener.onFinal("OpenWhisper dictation works perfectly.")
            }
        }, 220)
    }

    override fun cancel() {
        generation += 1
        listener = null
    }
}

class SelectingDictationBackend(
    private val select: () -> DictationBackend,
) : DictationBackend {
    private var current: DictationBackend? = null

    override fun start(listener: DictationBackend.Listener) {
        current?.cancel()
        current = select().also { it.start(listener) }
    }

    override fun finish() {
        current?.finish()
    }

    override fun cancel() {
        current?.cancel()
        current = null
    }
}
