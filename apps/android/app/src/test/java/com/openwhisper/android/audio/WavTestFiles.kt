package com.openwhisper.android.audio

/** A complete 16-bit mono PCM WAV file holding [pcm], built the way a finished recording is. */
fun pcm16MonoWav(pcm: ByteArray, sampleRate: Int): ByteArray =
    WavEncoder.header(pcm.size.toLong(), sampleRate) + pcm
